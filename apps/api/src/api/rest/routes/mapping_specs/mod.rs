//! HTTP handlers for canonical sync mapping configurations: immutable versions, validation, preview, reverse suggestion, sample records and the operator catalogue.
use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::permission_helpers::ensure_access;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::sync_helpers::build_netbox_registry_for_provider;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings};
use crate::contexts::sync::application::{
    MappingConfigurationDraft, MappingValidationError, ValidationSeverity,
    validate_mapping_configuration,
};
use crate::contexts::sync::domain::{
    ConflictMode, EntityMapping, FieldMapping, MappingConfiguration, MappingConfigurationStatus,
    MappingDirection, MappingTarget, Operator, Provider, TransformDirection, evaluate_expression,
};
use crate::contexts::sync::infrastructure::persistence::{
    MAPPING_CONFIGURATION_COLUMNS, MappingConfigDocument, MappingConfigurationListFilters,
    MappingConfigurationRepository,
};
use crate::contexts::sync::infrastructure::persistence::repositories::mapping_configuration_repository::MappingConfigurationScope;
use crate::contexts::sync::infrastructure::providers::{NetboxProvider, SyncProvider};
use crate::errors::AppError;
use crate::integrations::netbox::{NetboxClient, NetboxRepository};

mod operators;
mod transform;

use operators::{SYNC_OPERATOR_NAMES, sync_operator_response};
use transform::{
    create_draft_from_sync_payload, entity_drafts_from_value, parse_mapping_configuration_id,
    parse_uuid, patch_draft_from_sync_payload, sync_mapping_configuration_response,
    transform_operator_name,
};

fn parse_actor_id(auth_context: &AuthContext) -> Option<Uuid> {
    auth_context.user_id
}

async fn load_provider(state: &AppState, provider_id: Uuid) -> Result<Provider, AppError> {
    let service = state.services.sync_queries.clone();
    service
        .get_provider(provider_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Provider not found".to_string()))
}

// ---------------------------------------------------------------------------
// Canonical MappingConfiguration endpoints (immutable
// versions). The retired pre-migration `sync_mapping_spec` handler subtree
// (`/providers/{id}/mapping-specs...`, raw JSON mapping storage) is gone; the
// immutable typed configuration in `mapping_configurations` is the single
// write path.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MappingConfigurationResponse {
    pub logical_id: String,
    pub version_id: i32,
    pub name: String,
    pub description: Option<String>,
    pub source_provider_id: String,
    pub target: MappingTarget,
    pub status: String,
    pub direction: String,
    pub conflict_mode: String,
    pub created_by: Option<String>,
    pub created_at: String,
    pub retired_at: Option<String>,
}

impl From<MappingConfiguration> for MappingConfigurationResponse {
    fn from(value: MappingConfiguration) -> Self {
        Self {
            logical_id: value.logical_id.to_string(),
            version_id: value.version_id,
            name: value.name,
            description: value.description,
            source_provider_id: value.source_provider_id.to_string(),
            target: value.target,
            status: value.status.as_str().to_string(),
            direction: value.direction.as_str().to_string(),
            conflict_mode: value.conflict_mode.as_str().to_string(),
            created_by: value.created_by.map(|id| id.to_string()),
            created_at: value.created_at.to_rfc3339(),
            retired_at: value.retired_at.map(|ts| ts.to_rfc3339()),
        }
    }
}

/// HTTP 422 body shared by every mapping-configuration write endpoint: the
/// blocking validation errors plus any non-blocking warnings.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MappingValidationErrorResponse {
    pub errors: Vec<MappingValidationError>,
    pub warnings: Vec<MappingValidationError>,
}

/// Paginated list of sync mapping configurations.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SyncMappingConfigurationListResponse {
    pub items: Vec<SyncMappingConfigurationResponse>,
    pub total: usize,
    pub limit: i64,
    pub offset: i64,
}

/// Paginated list of mapping configurations for a provider.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MappingConfigurationListResponse {
    pub items: Vec<MappingConfigurationResponse>,
    pub limit: i64,
    pub offset: i64,
}

/// Result of validating a sync mapping configuration draft.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ValidateMappingResultResponse {
    pub valid: bool,
    pub diagnostics: Vec<MappingValidationError>,
}

/// Suggested reverse transform for a forward mapping expression.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SuggestReverseResponse {
    #[schema(value_type = Option<Object>)]
    pub reverse_transform: Option<Value>,
    pub note: Option<String>,
}

/// Result of evaluating a mapping draft against a sample record.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PreviewMappingResultResponse {
    #[schema(value_type = Object)]
    pub result: Value,
    pub errors: Vec<PreviewError>,
    pub warnings: Vec<PreviewError>,
}

/// A representative source record for a provider model.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SampleRecordResponse {
    pub model: String,
    // dynamic: passthrough of an external NetBox record — fields are
    // model-defined and not under our control, so the record stays arbitrary JSON.
    #[schema(value_type = Object)]
    pub record: Value,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListMappingConfigurationsQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListSyncMappingConfigurationsQuery {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub logical_id: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SyncMappingConfigurationResponse {
    pub id: String,
    pub logical_id: String,
    pub name: String,
    pub version: i32,
    pub version_id: i32,
    pub is_latest: bool,
    pub provider_id: String,
    pub source_provider_id: String,
    pub target: MappingTarget,
    pub status: String,
    pub direction: String,
    pub conflict_mode: String,
    #[schema(value_type = Vec<Object>)]
    pub entity_mappings: Vec<Value>,
    pub created_at: String,
    pub created_by: Option<String>,
    pub retired_at: Option<String>,
    pub in_flight_job_count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SyncOperatorResponse {
    pub name: String,
    pub label: String,
    pub description: String,
    pub category: String,
    pub invertibility_hint: String,
    #[schema(value_type = Object)]
    pub args_schema: Value,
    #[schema(value_type = Option<Object>)]
    pub suggested_reverse: Option<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ValidateSyncMappingConfigurationRequest {
    #[schema(value_type = Vec<Object>)]
    pub entity_mappings: Vec<Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SuggestReverseRequest {
    #[schema(value_type = Object)]
    pub forward_transform: Value,
    pub source_entity: String,
    pub source_field: String,
}

/// List sync mapping configurations.
#[utoipa::path(
    get,
    path = "/api/v1/sync/mapping-configurations",
    tag = "mapping-specs",
    params(
        ("provider_id" = Option<String>, Query, description = "Optional provider id scope"),
        ("logical_id" = Option<String>, Query, description = "Logical mapping configuration id"),
        ("search" = Option<String>, Query, description = "Free-text search over mapping names, descriptions and logical ids"),
        ("direction" = Option<String>, Query, description = "Comma-separated direction filter: pull,push,bidirectional"),
        ("sort" = Option<String>, Query, description = "Sort key: name, versions, direction, provider_id"),
        ("order" = Option<String>, Query, description = "Sort order: asc or desc"),
        ("limit" = Option<i64>, Query, description = "Maximum number of mapping configurations to return"),
        ("offset" = Option<i64>, Query, description = "Number of mapping configurations to skip"),
    ),
    responses(
        (status = 200, description = "Sync mapping configurations", body = SyncMappingConfigurationListResponse),
        (status = "4XX", description = "Sync mapping configuration list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync mapping configuration list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_sync_mapping_configurations(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListSyncMappingConfigurationsQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let provider_id = match query
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => match parse_uuid(value, "provider_id") {
            Ok(value) => Some(value),
            Err(error) => return handle_app_error(error),
        },
        None => None,
    };
    let filters = match parse_sync_mapping_configuration_filters(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        MAPPING_CONFIGURATION_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let limit = query.limit.clamp(1, 500);
    let offset = query.offset.max(0);

    let logical_id = match query
        .logical_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => match parse_uuid(value, "logical_id") {
            Ok(value) => Some(value),
            Err(error) => return handle_app_error(error),
        },
        None => None,
    };

    let rows = match state
        .repositories
        .sync_mapping_configurations
        .list(
            MappingConfigurationScope {
                provider_id,
                logical_id,
            },
            &filters,
            sort.as_ref(),
            limit,
            offset,
        )
        .await
    {
        Ok(rows) => rows,
        Err(error) => return handle_app_error(error),
    };
    let total = match state
        .repositories
        .sync_mapping_configurations
        .count(provider_id, logical_id, &filters)
        .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let total = match usize::try_from(total) {
        Ok(value) => value,
        Err(_) => {
            return handle_app_error(AppError::Internal(format!(
                "sync mapping configuration count overflowed usize: {total}"
            )));
        }
    };

    let mut items = Vec::with_capacity(rows.len());
    for configuration in rows {
        let document = match load_mapping_configuration_document(
            &state,
            configuration.logical_id,
            configuration.version_id,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
        let is_latest = match mapping_configuration_is_latest(
            &state,
            document.reference.logical_id,
            document.reference.version_id,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
        items.push(sync_mapping_configuration_response(&document, is_latest));
    }

    (
        StatusCode::OK,
        Json(json!({ "items": items, "total": total, "limit": limit, "offset": offset })),
    )
        .into_response()
}

fn parse_sync_mapping_configuration_filters(
    query: &ListSyncMappingConfigurationsQuery,
) -> Result<MappingConfigurationListFilters, AppError> {
    Ok(MappingConfigurationListFilters {
        search: query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        direction: parse_csv_strings(query.direction.as_deref()),
    })
}

/// Create a sync mapping configuration.
// dynamic request body: ad-hoc FE-sync wire envelope (provider_id + draft
// fields with operator/args transforms) normalized into a
// MappingConfigurationDraft by create_draft_from_sync_payload; no single
// concrete type mirrors it.
#[utoipa::path(
    post,
    path = "/api/v1/sync/mapping-configurations",
    tag = "mapping-specs",
    request_body = serde_json::Value,
    responses(
        (status = 201, description = "Sync mapping configuration created", body = SyncMappingConfigurationResponse),
        (status = 422, description = "Sync mapping configuration validation failed", body = MappingValidationErrorResponse),
        (status = "4XX", description = "Sync mapping configuration create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync mapping configuration create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_sync_mapping_configuration(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<Value>,
) -> impl IntoResponse {
    let actor_id = parse_actor_id(&auth_context);
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let (logical_id, draft) = match create_draft_from_sync_payload(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let _provider_id = draft.source_provider_id;

    let configuration = match create_new_mapping_configuration_version(
        &state, logical_id, 1, draft, actor_id,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let document = match load_mapping_configuration_document(
        &state,
        configuration.logical_id,
        configuration.version_id,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    (
        StatusCode::CREATED,
        Json(sync_mapping_configuration_response(&document, true)),
    )
        .into_response()
}

/// Get a sync mapping configuration.
#[utoipa::path(
    get,
    path = "/api/v1/sync/mapping-configurations/{id}",
    tag = "mapping-specs",
    params(
        ("id" = String, Path, description = "Mapping configuration id formatted as '<logical_id>:<version_id>'"),
    ),
    responses(
        (status = 200, description = "Sync mapping configuration", body = SyncMappingConfigurationResponse),
        (status = 404, description = "Sync mapping configuration not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Sync mapping configuration request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync mapping configuration request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_sync_mapping_configuration(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let (logical_id, version_id) = match parse_mapping_configuration_id(&id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let document = match load_mapping_configuration_document(&state, logical_id, version_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let is_latest = match mapping_configuration_is_latest(&state, logical_id, version_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    (
        StatusCode::OK,
        Json(sync_mapping_configuration_response(&document, is_latest)),
    )
        .into_response()
}

/// Update a sync mapping configuration.
// dynamic request body: ad-hoc FE-sync patch envelope normalized into a
// MappingConfigurationDraft by patch_draft_from_sync_payload; no single
// concrete type mirrors the partial-update wire shape.
#[utoipa::path(
    patch,
    path = "/api/v1/sync/mapping-configurations/{id}",
    tag = "mapping-specs",
    params(
        ("id" = String, Path, description = "Mapping configuration id formatted as '<logical_id>:<version_id>'"),
    ),
    request_body = serde_json::Value,
    responses(
        (status = 201, description = "New sync mapping configuration version created", body = SyncMappingConfigurationResponse),
        (status = 404, description = "Sync mapping configuration not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 422, description = "Sync mapping configuration validation failed", body = MappingValidationErrorResponse),
        (status = "4XX", description = "Sync mapping configuration update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync mapping configuration update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_sync_mapping_configuration(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<Value>,
) -> impl IntoResponse {
    let actor_id = parse_actor_id(&auth_context);
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let (logical_id, version_id) = match parse_mapping_configuration_id(&id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let document = match load_mapping_configuration_document(&state, logical_id, version_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let draft = match patch_draft_from_sync_payload(payload, &document) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let existing = match state
        .repositories
        .sync_mapping_configurations
        .list_versions_for_logical_id(logical_id)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return handle_app_error(error),
    };
    let next_version = existing
        .iter()
        .map(|configuration| configuration.version_id)
        .max()
        .unwrap_or(version_id)
        .saturating_add(1);

    let configuration = match create_new_mapping_configuration_version(
        &state,
        logical_id,
        next_version,
        draft,
        actor_id,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let document = match load_mapping_configuration_document(
        &state,
        configuration.logical_id,
        configuration.version_id,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    (
        StatusCode::CREATED,
        Json(sync_mapping_configuration_response(&document, true)),
    )
        .into_response()
}

/// Delete a sync mapping configuration.
#[utoipa::path(
    delete,
    path = "/api/v1/sync/mapping-configurations/{id}",
    tag = "mapping-specs",
    params(
        ("id" = String, Path, description = "Mapping configuration id formatted as '<logical_id>:<version_id>'"),
    ),
    responses(
        (status = 204, description = "Sync mapping configuration retired"),
        (status = 404, description = "Sync mapping configuration not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Sync mapping configuration delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync mapping configuration delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_sync_mapping_configuration(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let (logical_id, version_id) = match parse_mapping_configuration_id(&id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = load_mapping_configuration_document(&state, logical_id, version_id).await {
        return handle_app_error(error);
    }

    match state
        .repositories
        .sync_mapping_configurations
        .retire(logical_id, version_id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Validate a sync mapping configuration.
#[utoipa::path(
    post,
    path = "/api/v1/sync/mapping-configurations/validate",
    tag = "mapping-specs",
    request_body = ValidateSyncMappingConfigurationRequest,
    responses(
        (status = 200, description = "Sync mapping validation result", body = ValidateMappingResultResponse),
        (status = "4XX", description = "Sync mapping validation request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync mapping validation failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn validate_sync_mapping_configuration(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ValidateSyncMappingConfigurationRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let entity_mappings = match entity_drafts_from_value(Value::Array(payload.entity_mappings)) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let draft = MappingConfigurationDraft {
        name: "validation".to_string(),
        description: None,
        source_provider_id: Uuid::nil(),
        target: MappingTarget::LocalInventory,
        direction: "pull".to_string(),
        conflict_mode: "netbox_wins".to_string(),
        entity_mappings,
    };
    let diagnostics = validate_mapping_configuration(&draft);
    let valid = diagnostics.iter().all(|diagnostic| !diagnostic.is_error());

    (
        StatusCode::OK,
        Json(json!({ "valid": valid, "diagnostics": diagnostics })),
    )
        .into_response()
}

/// Suggest a reverse mapping configuration.
#[utoipa::path(
    post,
    path = "/api/v1/sync/mapping-configurations/suggest-reverse",
    tag = "mapping-specs",
    request_body = SuggestReverseRequest,
    responses(
        (status = 200, description = "Suggested reverse transform", body = SuggestReverseResponse),
        (status = "4XX", description = "Suggest reverse request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Suggest reverse failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn suggest_reverse_mapping_configuration(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<SuggestReverseRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let Some(op_name) = transform_operator_name(&payload.forward_transform) else {
        return handle_app_error(AppError::Validation(
            "forward_transform must include an operator".to_string(),
        ));
    };
    let Some(operator) = Operator::from_name(op_name) else {
        return handle_app_error(AppError::Validation(format!(
            "unknown operator '{op_name}'"
        )));
    };

    let _source_context = (&payload.source_entity, &payload.source_field);
    let (reverse_transform, note) = if operator.invertibility_hint() {
        (
            Some(payload.forward_transform),
            Some("Operator is invertible; reuse the forward transform arguments".to_string()),
        )
    } else {
        (
            None,
            Some("Operator is not invertible; author a reverse transform manually or leave it read-only".to_string()),
        )
    };

    (
        StatusCode::OK,
        Json(json!({
            "reverse_transform": reverse_transform,
            "note": note,
        })),
    )
        .into_response()
}

/// List the operators available for mapping expressions.
#[utoipa::path(
    get,
    path = "/api/v1/sync/operators",
    tag = "mapping-specs",
    responses(
        (status = 200, description = "Sync transform operators", body = [SyncOperatorResponse]),
        (status = "4XX", description = "Sync operator request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sync operator request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_sync_operators(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let operators = match SYNC_OPERATOR_NAMES
        .iter()
        .copied()
        .map(sync_operator_response)
        .collect::<Result<Vec<_>, AppError>>()
    {
        Ok(operators) => operators,
        Err(error) => return handle_app_error(error),
    };
    (StatusCode::OK, Json(operators)).into_response()
}

/// List mapping configurations.
#[utoipa::path(
    get,
    path = "/api/v1/providers/{id}/mapping-configurations",
    tag = "mapping-specs",
    params(
        ("id" = String, Path, description = "Provider id"),
        ("limit" = Option<i64>, Query, description = "Maximum number of mapping configurations to return"),
        ("offset" = Option<i64>, Query, description = "Number of mapping configurations to skip"),
    ),
    responses(
        (status = 200, description = "Mapping configurations", body = MappingConfigurationListResponse),
        (status = "4XX", description = "Mapping configuration list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Mapping configuration list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_mapping_configurations(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    Query(query): Query<ListMappingConfigurationsQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let limit = query.limit.clamp(1, 500);
    let offset = query.offset.max(0);

    match state
        .repositories
        .sync_mapping_configurations
        .list_for_provider(
            provider_id,
            &MappingConfigurationListFilters::default(),
            None,
            limit,
            offset,
        )
        .await
    {
        Ok(rows) => {
            let items: Vec<MappingConfigurationResponse> = rows
                .into_iter()
                .map(MappingConfigurationResponse::from)
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "items": items, "limit": limit, "offset": offset })),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create a mapping configuration for a provider.
///
/// Starts a new logical mapping at `version_id` 1.
#[utoipa::path(
    post,
    path = "/api/v1/providers/{id}/mapping-configurations",
    tag = "mapping-specs",
    params(
        ("id" = String, Path, description = "Provider id"),
    ),
    request_body = MappingConfigurationDraft,
    responses(
        (status = 201, description = "Mapping configuration created", body = MappingConfigurationResponse),
        (status = 422, description = "Mapping configuration validation failed", body = MappingValidationErrorResponse),
        (status = "4XX", description = "Mapping configuration create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Mapping configuration create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_mapping_configuration(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<MappingConfigurationDraft>,
) -> impl IntoResponse {
    let actor_id = parse_actor_id(&auth_context);
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    if let Err(error) = parse_uuid(&provider_id, "provider_id") {
        return handle_app_error(error);
    }

    let logical_id = Uuid::now_v7();
    save_new_version(
        &state, logical_id, /*version_id=*/ 1, payload, actor_id,
    )
    .await
}

/// Create a new version of a mapping configuration.
///
/// Adds a version; the existing versions are kept.
#[utoipa::path(
    post,
    path = "/api/v1/providers/{id}/mapping-configurations/{logical_id}/versions",
    tag = "mapping-specs",
    params(
        ("id" = String, Path, description = "Provider id"),
        ("logical_id" = String, Path, description = "Logical mapping configuration id"),
    ),
    request_body = MappingConfigurationDraft,
    responses(
        (status = 201, description = "Mapping configuration version created", body = MappingConfigurationResponse),
        (status = 404, description = "Mapping configuration not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 422, description = "Mapping configuration validation failed", body = MappingValidationErrorResponse),
        (status = "4XX", description = "Mapping configuration version request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Mapping configuration version create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_mapping_configuration_version(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path((provider_id, logical_id)): Path<(String, String)>,
    EnvelopeJson(payload): EnvelopeJson<MappingConfigurationDraft>,
) -> impl IntoResponse {
    let actor_id = parse_actor_id(&auth_context);
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    if let Err(error) = parse_uuid(&provider_id, "provider_id") {
        return handle_app_error(error);
    }
    let logical_id = match parse_uuid(&logical_id, "logical_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let existing = match state
        .repositories
        .sync_mapping_configurations
        .list_versions_for_logical_id(logical_id)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return handle_app_error(error),
    };

    if existing.is_empty() {
        return handle_app_error(AppError::NotFound(format!(
            "no mapping configuration with logical_id={logical_id}"
        )));
    }
    let next_version = existing
        .iter()
        .map(|c| c.version_id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);

    save_new_version(&state, logical_id, next_version, payload, actor_id).await
}

/// Get a mapping configuration.
#[utoipa::path(
    get,
    path = "/api/v1/providers/{id}/mapping-configurations/{logical_id}/{version_id}",
    tag = "mapping-specs",
    params(
        ("id" = String, Path, description = "Provider id"),
        ("logical_id" = String, Path, description = "Logical mapping configuration id"),
        ("version_id" = i32, Path, description = "Mapping configuration version id"),
    ),
    responses(
        (status = 200, description = "Mapping configuration version", body = MappingConfigurationResponse),
        (status = 404, description = "Mapping configuration version not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Mapping configuration request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Mapping configuration request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_mapping_configuration(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path((provider_id, logical_id, version_id)): Path<(String, String, i32)>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    if let Err(error) = parse_uuid(&provider_id, "provider_id") {
        return handle_app_error(error);
    }
    let logical_id = match parse_uuid(&logical_id, "logical_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .sync_mapping_configurations
        .get(logical_id, version_id)
        .await
    {
        Ok(Some(config)) => (
            StatusCode::OK,
            Json(MappingConfigurationResponse::from(config)),
        )
            .into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(
            "Mapping configuration version not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Retire a mapping configuration version.
///
/// Marks the version `retired`; the version is kept, not deleted.
#[utoipa::path(
    delete,
    path = "/api/v1/providers/{id}/mapping-configurations/{logical_id}/{version_id}",
    tag = "mapping-specs",
    params(
        ("id" = String, Path, description = "Provider id"),
        ("logical_id" = String, Path, description = "Logical mapping configuration id"),
        ("version_id" = i32, Path, description = "Mapping configuration version id"),
    ),
    responses(
        (status = 204, description = "Mapping configuration version retired"),
        (status = "4XX", description = "Mapping configuration retire request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Mapping configuration retire failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn retire_mapping_configuration(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path((provider_id, logical_id, version_id)): Path<(String, String, i32)>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    if let Err(error) = parse_uuid(&provider_id, "provider_id") {
        return handle_app_error(error);
    }
    let logical_id = match parse_uuid(&logical_id, "logical_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .sync_mapping_configurations
        .retire(logical_id, version_id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SampleRecordQuery {
    pub model: String,
}

/// Get a sample source record for a provider model.
///
/// Fetches the model from the live NetBox provider and returns the first record
/// as-is.
#[utoipa::path(
    get,
    path = "/api/v1/providers/{id}/sample-record",
    tag = "mapping-specs",
    params(
        ("id" = String, Path, description = "Provider id"),
        ("model" = String, Query, description = "Provider model name"),
    ),
    responses(
        (status = 200, description = "Sample provider record", body = SampleRecordResponse),
        (status = 404, description = "No sample record found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Sample record request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Sample record request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_sample_record(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    Query(query): Query<SampleRecordQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let provider_id = match parse_uuid(&provider_id, "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let provider = match load_provider(&state, provider_id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    if query.model.trim().is_empty() {
        return handle_app_error(AppError::Validation(
            "model query parameter must not be empty".to_string(),
        ));
    }
    let model = query.model.trim().to_string();

    // Cache hit short-circuits the round-trip to NetBox. The cache is shared
    // across all sessions in this process; entries TTL out at
    // `PIPING_SYNC__SAMPLE_CACHE_TTL_SECONDS` (default 600s) and the
    // `provider_registry/refresh` endpoint invalidates per-provider on
    // schema reload.
    if let Some(record) = state
        .services
        .sample_record_cache
        .get(provider_id, &model)
        .await
    {
        return (
            StatusCode::OK,
            Json(json!({ "model": model, "record": record })),
        )
            .into_response();
    }

    let netbox_config = match state
        .services
        .source_service
        .resolve_netbox_config_for_provider(&provider)
        .await
    {
        Ok(config) => config,
        Err(error) => return handle_app_error(error),
    };
    let registry = match build_netbox_registry_for_provider(&state, &provider).await {
        Ok(registry) => registry,
        Err(error) => return handle_app_error(error),
    };
    let client = match NetboxClient::from_config(&netbox_config) {
        Ok(client) => client,
        Err(error) => return handle_app_error(error),
    };
    let repository = NetboxRepository::new(client, registry);
    let sync_provider = NetboxProvider::new(repository, netbox_config.page_size);

    match sync_provider.fetch_inventory(&model).await {
        Ok(mut records) if !records.is_empty() => {
            let record = records.remove(0);
            state
                .services
                .sample_record_cache
                .insert(provider_id, model.clone(), record.clone())
                .await;
            (
                StatusCode::OK,
                Json(json!({ "model": model, "record": record })),
            )
                .into_response()
        }
        Ok(_) => handle_app_error(AppError::NotFound(format!(
            "No records returned by provider for model {model}"
        ))),
        Err(error) => handle_app_error(error),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PreviewMappingRequest {
    #[schema(value_type = Object)]
    pub mapping_config: MappingConfigurationDraft,
    pub sample_record: Value,
    /// "forward" or "reverse". Defaults to "forward".
    #[serde(default = "default_direction")]
    pub direction: String,
}

fn default_direction() -> String {
    "forward".to_string()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PreviewError {
    pub path: String,
    pub code: String,
    pub message: String,
}

/// Preview a mapping-configuration draft against a sample record.
///
/// Evaluates the draft against the sample record and returns per-field results
/// with per-field errors and warnings.
#[utoipa::path(
    post,
    path = "/api/v1/sync/mapping-configurations/preview",
    tag = "mapping-specs",
    request_body = PreviewMappingRequest,
    responses(
        (status = 200, description = "Mapping preview result", body = PreviewMappingResultResponse),
        (status = 422, description = "Mapping preview validation failed", body = MappingValidationErrorResponse),
        (status = "4XX", description = "Mapping preview request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Mapping preview failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn preview_mapping_configuration(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PreviewMappingRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let direction = match payload.direction.as_str() {
        "forward" => TransformDirection::Forward,
        "reverse" => TransformDirection::Reverse,
        other => {
            return handle_app_error(AppError::Validation(format!(
                "direction must be 'forward' or 'reverse', got '{other}'"
            )));
        }
    };

    let validation_diagnostics = validate_mapping_configuration(&payload.mapping_config);
    let (validation_errors, validation_warnings): (Vec<_>, Vec<_>) = validation_diagnostics
        .into_iter()
        .partition(|d| matches!(d.severity, ValidationSeverity::Error));
    if !validation_errors.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "errors": validation_errors,
                "warnings": validation_warnings,
            })),
        )
            .into_response();
    }

    let mut result = serde_json::Map::new();
    let mut errors: Vec<PreviewError> = Vec::new();
    let mut warnings: Vec<PreviewError> = validation_warnings
        .into_iter()
        .map(|d| PreviewError {
            path: d.path,
            code: d.code,
            message: d.message,
        })
        .collect();

    for entity in &payload.mapping_config.entity_mappings {
        let mut entity_out = serde_json::Map::new();
        for field in &entity.field_mappings {
            let expr = match direction {
                TransformDirection::Forward => &field.forward_transform,
                TransformDirection::Reverse => match &field.reverse_transform {
                    Some(value) => value,
                    None => {
                        warnings.push(PreviewError {
                            path: format!(
                                "{}.{}.{}",
                                entity.target_entity, field.target_field, "reverse_transform"
                            ),
                            code: "reverse_null".to_string(),
                            message:
                                "reverse_transform is null; field is read-only in this direction"
                                    .to_string(),
                        });
                        continue;
                    }
                },
            };

            match evaluate_expression(expr, &payload.sample_record, direction) {
                Ok(value) => {
                    entity_out.insert(field.target_field.clone(), value);
                }
                Err(AppError::Internal(message)) => errors.push(PreviewError {
                    path: format!("{}.{}", entity.target_entity, field.target_field),
                    code: "not_implemented".to_string(),
                    message,
                }),
                Err(other) => errors.push(PreviewError {
                    path: format!("{}.{}", entity.target_entity, field.target_field),
                    code: "evaluation_error".to_string(),
                    message: other.to_string(),
                }),
            }
        }
        result.insert(entity.target_entity.clone(), Value::Object(entity_out));
    }

    (
        StatusCode::OK,
        Json(json!({
            "result": Value::Object(result),
            "errors": errors,
            "warnings": warnings,
        })),
    )
        .into_response()
}

async fn save_new_version(
    state: &AppState,
    logical_id: Uuid,
    version_id: i32,
    draft: MappingConfigurationDraft,
    actor_id: Option<Uuid>,
) -> axum::response::Response {
    match create_new_mapping_configuration_version(state, logical_id, version_id, draft, actor_id)
        .await
    {
        Ok(configuration) => (
            StatusCode::CREATED,
            Json(MappingConfigurationResponse::from(configuration)),
        )
            .into_response(),
        Err(response) => *response,
    }
}

async fn create_new_mapping_configuration_version(
    state: &AppState,
    logical_id: Uuid,
    version_id: i32,
    draft: MappingConfigurationDraft,
    actor_id: Option<Uuid>,
) -> Result<MappingConfiguration, Box<axum::response::Response>> {
    let validation_diagnostics = validate_mapping_configuration(&draft);
    let (validation_errors, validation_warnings): (Vec<_>, Vec<_>) = validation_diagnostics
        .into_iter()
        .partition(|d| matches!(d.severity, ValidationSeverity::Error));
    if !validation_errors.is_empty() {
        return Err(Box::new(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "errors": validation_errors,
                    "warnings": validation_warnings,
                })),
            )
                .into_response(),
        ));
    }
    let _ = validation_warnings;

    let direction = match MappingDirection::from_wire(&draft.direction) {
        Some(value) => value,
        None => {
            return Err(Box::new(handle_app_error(AppError::Validation(format!(
                "unknown direction '{}'",
                draft.direction
            )))));
        }
    };
    let conflict_mode = match ConflictMode::from_wire(&draft.conflict_mode) {
        Some(value) => value,
        None => {
            return Err(Box::new(handle_app_error(AppError::Validation(format!(
                "unknown conflict_mode '{}'",
                draft.conflict_mode
            )))));
        }
    };

    let now = Utc::now();
    let configuration = MappingConfiguration {
        logical_id,
        version_id,
        name: draft.name,
        description: draft.description,
        source_provider_id: draft.source_provider_id,
        target: draft.target,
        status: MappingConfigurationStatus::Active,
        direction,
        conflict_mode,
        created_by: actor_id,
        created_at: now,
        retired_at: None,
    };

    let mut entity_rows: Vec<EntityMapping> = Vec::with_capacity(draft.entity_mappings.len());
    let mut field_rows: Vec<FieldMapping> = Vec::new();

    for entity in draft.entity_mappings {
        let entity_id = Uuid::now_v7();
        entity_rows.push(EntityMapping {
            id: entity_id,
            mapping_config_logical_id: logical_id,
            mapping_config_version_id: version_id,
            source_entity: entity.source_entity,
            target_entity: entity.target_entity,
            enabled: entity.enabled,
            created_at: now,
        });
        for field in entity.field_mappings {
            field_rows.push(FieldMapping {
                id: Uuid::now_v7(),
                entity_mapping_id: entity_id,
                source_field: field.source_field,
                target_field: field.target_field,
                forward_transform_json: field.forward_transform,
                reverse_transform_json: field.reverse_transform,
                round_trip_critical: field.round_trip_critical,
                created_at: now,
            });
        }
    }

    match state
        .repositories
        .sync_mapping_configurations
        .insert_new_version(&configuration, &entity_rows, &field_rows)
        .await
    {
        Ok(()) => Ok(configuration),
        Err(error) => Err(Box::new(handle_app_error(error))),
    }
}

async fn load_mapping_configuration_document(
    state: &AppState,
    logical_id: Uuid,
    version_id: i32,
) -> Result<MappingConfigDocument, AppError> {
    state
        .repositories
        .sync_mapping_configurations
        .get_document(logical_id, version_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Mapping configuration version not found".to_string()))
}

async fn mapping_configuration_is_latest(
    state: &AppState,
    logical_id: Uuid,
    version_id: i32,
) -> Result<bool, AppError> {
    let versions = state
        .repositories
        .sync_mapping_configurations
        .list_versions_for_logical_id(logical_id)
        .await?;
    Ok(versions
        .iter()
        .map(|configuration| configuration.version_id)
        .max()
        .map(|latest| latest == version_id)
        .unwrap_or(false))
}
