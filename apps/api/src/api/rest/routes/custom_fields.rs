//! REST surface for custom-field definitions and their choice sets.
//!
//! Definitions are admin-managed inventory schema, so every operation is gated
//! on `ResourceType::Inventory` (the closest existing admin resource) with the
//! matching CRUD `Action`. Per-entity custom-field VALUES are surfaced on the
//! inventory entity responses themselves; the only value endpoint here is the
//! audited per-entity secret reveal.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::api::access_control_helpers::build_access_control_service_from_repos;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::custom_fields::CustomFieldChoiceSetRepository;
use crate::contexts::custom_fields::{
    CUSTOM_FIELD_CHOICE_SET_COLUMNS, CUSTOM_FIELD_DEFINITION_COLUMNS, Choice, CustomFieldChoiceSet,
    CustomFieldChoiceSetListFilter, CustomFieldDefinition, CustomFieldDefinitionListFilter,
    FilterLogic, NewCustomFieldChoiceSet, NewCustomFieldDefinition, UiEditable, UiVisible,
    UpdateCustomFieldChoiceSet, UpdateCustomFieldDefinition, can_read_entity_secret,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::content_type_lookup::CONTENT_TYPE_COLUMNS;
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::normalize_page;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

// ── DTOs ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct CustomFieldDefinitionListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub field_type: Option<String>,
    pub group_name: Option<String>,
    pub ui_visible: Option<String>,
    pub ui_editable: Option<String>,
    pub filter_logic: Option<String>,
    pub choice_set_id: Option<String>,
    pub related_object_type_id: Option<String>,
    pub object_type_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ChoiceSetListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ContentTypeListQuery {
    pub sort: Option<String>,
    pub order: Option<String>,
    pub app_label: Option<String>,
    pub model: Option<String>,
}

/// A custom-field definition as returned to clients.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CustomFieldDefinitionResponse {
    pub id: Uuid,
    pub name: String,
    pub label: String,
    /// One of the 13 NetBox field types (`text`, `integer`, `select`, ...).
    pub field_type: String,
    pub description: String,
    pub group_name: String,
    pub required: bool,
    pub enforce_unique: bool,
    pub is_secret: bool,
    pub default_value: Option<Value>,
    pub weight: i32,
    pub validation_minimum: Option<f64>,
    pub validation_maximum: Option<f64>,
    pub validation_regex: String,
    pub validation_schema: Option<Value>,
    pub choice_set_id: Option<Uuid>,
    pub related_object_type_id: Option<Uuid>,
    pub related_object_filter: Option<Value>,
    /// `always` / `if-set` / `hidden`.
    pub ui_visible: String,
    /// `yes` / `no` / `hidden`.
    pub ui_editable: String,
    pub is_cloneable: bool,
    pub search_weight: i32,
    /// `disabled` / `loose` / `exact`.
    pub filter_logic: String,
    /// Inventory `content_type` ids this field is scoped to.
    pub object_types: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CustomFieldDefinitionListResponse {
    pub items: Vec<CustomFieldDefinitionResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

impl From<CustomFieldDefinition> for CustomFieldDefinitionResponse {
    fn from(def: CustomFieldDefinition) -> Self {
        Self {
            id: def.id,
            name: def.name,
            label: def.label,
            field_type: def.field_type.as_str().to_string(),
            description: def.description,
            group_name: def.group_name,
            required: def.required,
            enforce_unique: def.enforce_unique,
            is_secret: def.is_secret,
            default_value: def.default_value,
            weight: def.weight,
            validation_minimum: def.validation_minimum,
            validation_maximum: def.validation_maximum,
            validation_regex: def.validation_regex,
            validation_schema: def.validation_schema,
            choice_set_id: def.choice_set_id,
            related_object_type_id: def.related_object_type_id,
            related_object_filter: def.related_object_filter,
            ui_visible: def.ui_visible.as_str().to_string(),
            ui_editable: def.ui_editable.as_str().to_string(),
            is_cloneable: def.is_cloneable,
            search_weight: def.search_weight,
            filter_logic: def.filter_logic.as_str().to_string(),
            object_types: def.object_types,
            created_at: def.created_at,
            updated_at: def.updated_at,
        }
    }
}

/// Create payload for a custom-field definition.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CustomFieldDefinitionCreatePayload {
    pub name: String,
    pub label: String,
    pub field_type: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub enforce_unique: bool,
    #[serde(default)]
    pub is_secret: bool,
    pub default_value: Option<Value>,
    #[serde(default)]
    pub weight: i32,
    pub validation_minimum: Option<f64>,
    pub validation_maximum: Option<f64>,
    #[serde(default)]
    pub validation_regex: String,
    pub validation_schema: Option<Value>,
    pub choice_set_id: Option<Uuid>,
    pub related_object_type_id: Option<Uuid>,
    pub related_object_filter: Option<Value>,
    /// `always` / `if-set` / `hidden`. Defaults to `always`.
    pub ui_visible: Option<String>,
    /// `yes` / `no` / `hidden`. Defaults to `yes`.
    pub ui_editable: Option<String>,
    #[serde(default = "default_true")]
    pub is_cloneable: bool,
    #[serde(default)]
    pub search_weight: i32,
    /// `disabled` / `loose` / `exact`. Defaults to `loose`.
    pub filter_logic: Option<String>,
    #[serde(default)]
    pub object_types: Vec<Uuid>,
}

/// Patch payload for a custom-field definition. Absent fields are unchanged.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CustomFieldDefinitionUpdatePayload {
    pub label: Option<String>,
    pub field_type: Option<String>,
    pub description: Option<String>,
    pub group_name: Option<String>,
    pub required: Option<bool>,
    pub enforce_unique: Option<bool>,
    pub is_secret: Option<bool>,
    pub default_value: Option<Option<Value>>,
    pub weight: Option<i32>,
    pub validation_minimum: Option<Option<f64>>,
    pub validation_maximum: Option<Option<f64>>,
    pub validation_regex: Option<String>,
    pub validation_schema: Option<Option<Value>>,
    pub choice_set_id: Option<Option<Uuid>>,
    pub related_object_type_id: Option<Option<Uuid>>,
    pub related_object_filter: Option<Option<Value>>,
    pub ui_visible: Option<String>,
    pub ui_editable: Option<String>,
    pub is_cloneable: Option<bool>,
    pub search_weight: Option<i32>,
    pub filter_logic: Option<String>,
    pub object_types: Option<Vec<Uuid>>,
}

/// A `{value, label}` choice option.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChoiceDto {
    pub value: String,
    pub label: String,
}

impl From<Choice> for ChoiceDto {
    fn from(choice: Choice) -> Self {
        Self {
            value: choice.value,
            label: choice.label,
        }
    }
}

impl From<ChoiceDto> for Choice {
    fn from(dto: ChoiceDto) -> Self {
        Self {
            value: dto.value,
            label: dto.label,
        }
    }
}

/// A custom-field choice set as returned to clients.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ChoiceSetResponse {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub extra_choices: Vec<ChoiceDto>,
    pub order_alphabetically: bool,
    pub choice_colors: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ChoiceSetListResponse {
    pub items: Vec<ChoiceSetResponse>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

impl From<CustomFieldChoiceSet> for ChoiceSetResponse {
    fn from(set: CustomFieldChoiceSet) -> Self {
        Self {
            id: set.id,
            name: set.name,
            description: set.description,
            extra_choices: set.extra_choices.into_iter().map(ChoiceDto::from).collect(),
            order_alphabetically: set.order_alphabetically,
            choice_colors: set.choice_colors,
            created_at: set.created_at,
            updated_at: set.updated_at,
        }
    }
}

/// Create payload for a choice set.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ChoiceSetCreatePayload {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub extra_choices: Vec<ChoiceDto>,
    #[serde(default)]
    pub order_alphabetically: bool,
    #[serde(default)]
    pub choice_colors: BTreeMap<String, String>,
}

/// Patch payload for a choice set. Absent fields are unchanged.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ChoiceSetUpdatePayload {
    pub name: Option<String>,
    pub description: Option<String>,
    pub extra_choices: Option<Vec<ChoiceDto>>,
    pub order_alphabetically: Option<bool>,
    pub choice_colors: Option<BTreeMap<String, String>>,
}

/// A seeded inventory content type, as offered to the custom-field scope
/// picker and the object/multiobject selectors.
///
/// The `id` is minted per-install (uuidv7), so it is NOT portable across
/// installs; `token` (`app_label.model`) is the stable cross-install handle.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContentTypeResponse {
    /// Per-install id (uuidv7).
    pub id: Uuid,
    /// NetBox app label, e.g. `dcim`.
    pub app_label: String,
    /// NetBox model, e.g. `site`.
    pub model: String,
    /// Stable `app_label.model` token, e.g. `dcim.site`.
    pub token: String,
    /// Human-readable label, e.g. `Site (dcim)`.
    pub label: String,
    /// Whether this object type is fully wired into the typed custom-field
    /// read/write/reveal pipeline and can be selected as a field scope.
    pub supported_for_custom_fields: bool,
    /// Feature capabilities for models with a pinned native contract; absent
    /// for the rest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<crate::contexts::inventory::domain::content_type::ModelCapabilities>,
}

/// Build a human label from `(app_label, model)`, e.g.
/// `("dcim", "virtualmachine")` → `"Virtualmachine (dcim)"`. Deterministic and
/// source-free so it stays install-portable.
fn content_type_label(app_label: &str, model: &str) -> String {
    let mut model_title = String::with_capacity(model.len());
    let mut chars = model.chars();
    if let Some(first) = chars.next() {
        model_title.extend(first.to_uppercase());
        model_title.extend(chars);
    }
    format!("{model_title} ({app_label})")
}

fn supports_custom_fields_scope(token: &str) -> bool {
    matches!(
        token,
        CONTENT_TYPE_SITE
            | CONTENT_TYPE_DEVICE
            | CONTENT_TYPE_REGION
            | CONTENT_TYPE_CLUSTER
            | CONTENT_TYPE_TENANT
            | CONTENT_TYPE_VIRTUAL_MACHINE
            | CONTENT_TYPE_VIRTUAL_MACHINE_TYPE
    )
}

/// Cleartext of an entity's secret custom fields, returned by the reveal
/// endpoint after the per-entity `Action::Secret` check passes.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CustomFieldRevealResponse {
    /// Per-object custom field `name -> value` map with secret values resolved
    /// to cleartext.
    // dynamic: per-object custom field name->value map (definition+value, secrets revealed)
    pub custom_fields: Value,
}

const fn default_true() -> bool {
    true
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn ensure_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    crate::api::permission_helpers::ensure_access(
        state,
        request_context,
        action,
        ResourceType::Inventory,
    )
    .await
}

fn parse_field_type(
    raw: &str,
) -> Result<crate::contexts::custom_fields::CustomFieldType, AppError> {
    crate::contexts::custom_fields::CustomFieldType::parse(raw)
        .ok_or_else(|| AppError::Validation(format!("unknown custom field type '{raw}'")))
}

fn parse_ui_visible(raw: &str) -> Result<UiVisible, AppError> {
    UiVisible::parse(raw).ok_or_else(|| AppError::Validation(format!("invalid ui_visible '{raw}'")))
}

fn parse_ui_editable(raw: &str) -> Result<UiEditable, AppError> {
    UiEditable::parse(raw)
        .ok_or_else(|| AppError::Validation(format!("invalid ui_editable '{raw}'")))
}

fn parse_filter_logic(raw: &str) -> Result<FilterLogic, AppError> {
    FilterLogic::parse(raw)
        .ok_or_else(|| AppError::Validation(format!("invalid filter_logic '{raw}'")))
}

fn build_definition_list_filter(
    query: &CustomFieldDefinitionListQuery,
) -> Result<CustomFieldDefinitionListFilter, AppError> {
    let field_type = parse_csv_strings(query.field_type.as_deref());
    for value in &field_type {
        parse_field_type(value)?;
    }
    let group_name = parse_csv_strings(query.group_name.as_deref());
    let ui_visible = parse_csv_strings(query.ui_visible.as_deref());
    for value in &ui_visible {
        parse_ui_visible(value)?;
    }
    let ui_editable = parse_csv_strings(query.ui_editable.as_deref());
    for value in &ui_editable {
        parse_ui_editable(value)?;
    }
    let filter_logic = parse_csv_strings(query.filter_logic.as_deref());
    for value in &filter_logic {
        parse_filter_logic(value)?;
    }

    Ok(CustomFieldDefinitionListFilter {
        search: query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        field_type,
        group_name,
        ui_visible,
        ui_editable,
        filter_logic,
        choice_set_id: parse_csv_uuids(query.choice_set_id.as_deref(), "choice_set_id")?,
        related_object_type_id: parse_csv_uuids(
            query.related_object_type_id.as_deref(),
            "related_object_type_id",
        )?,
        object_type_id: parse_csv_uuids(query.object_type_id.as_deref(), "object_type_id")?,
        sort: SortSpec::parse_columns(
            query.sort.as_deref(),
            query.order.as_deref(),
            CUSTOM_FIELD_DEFINITION_COLUMNS,
        )?,
    })
}

// ── Definition endpoints ────────────────────────────────────────────────────

/// List custom-field definitions.
#[utoipa::path(
    get,
    path = "/api/v1/custom-fields",
    tag = "custom-fields",
    params(CustomFieldDefinitionListQuery),
    responses(
        (status = 200, description = "Custom field definitions", body = CustomFieldDefinitionListResponse),
        (status = "4XX", description = "Custom field list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Custom field list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_custom_fields(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<CustomFieldDefinitionListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }
    let (limit, offset) = normalize_page(query.limit, query.offset);
    let filter = match build_definition_list_filter(&query) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let items = match state
        .services
        .custom_field_definitions
        .list_page(&filter, limit, offset)
        .await
    {
        Ok(definitions) => definitions
            .into_iter()
            .map(CustomFieldDefinitionResponse::from)
            .collect(),
        Err(error) => return handle_app_error(error),
    };
    let total = match state.services.custom_field_definitions.count(&filter).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    (
        StatusCode::OK,
        Json(CustomFieldDefinitionListResponse {
            items,
            total,
            limit,
            offset,
        }),
    )
        .into_response()
}

/// Get a custom-field definition.
#[utoipa::path(
    get,
    path = "/api/v1/custom-fields/{id}",
    tag = "custom-fields",
    params(("id" = String, Path, description = "Custom field id")),
    responses(
        (status = 200, description = "Custom field definition", body = CustomFieldDefinitionResponse),
        (status = 404, description = "Custom field not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Custom field request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Custom field request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_custom_field(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }
    match state.services.custom_field_definitions.get(id).await {
        Ok(Some(def)) => (
            StatusCode::OK,
            Json(CustomFieldDefinitionResponse::from(def)),
        )
            .into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Custom field not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a custom-field definition.
#[utoipa::path(
    post,
    path = "/api/v1/custom-fields",
    tag = "custom-fields",
    request_body = CustomFieldDefinitionCreatePayload,
    responses(
        (status = 201, description = "Custom field created", body = CustomFieldDefinitionResponse),
        (status = "4XX", description = "Custom field create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Custom field create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_custom_field(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CustomFieldDefinitionCreatePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Create).await {
        return handle_app_error(error);
    }
    let input = match build_new_definition(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state.services.custom_field_definitions.create(input).await {
        Ok(def) => (
            StatusCode::CREATED,
            Json(CustomFieldDefinitionResponse::from(def)),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a custom-field definition.
#[utoipa::path(
    patch,
    path = "/api/v1/custom-fields/{id}",
    tag = "custom-fields",
    params(("id" = String, Path, description = "Custom field id")),
    request_body = CustomFieldDefinitionUpdatePayload,
    responses(
        (status = 200, description = "Custom field updated", body = CustomFieldDefinitionResponse),
        (status = 404, description = "Custom field not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Custom field update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Custom field update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_custom_field(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<CustomFieldDefinitionUpdatePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Update).await {
        return handle_app_error(error);
    }
    let input = match build_update_definition(payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .services
        .custom_field_definitions
        .update(id, input)
        .await
    {
        Ok(Some(def)) => (
            StatusCode::OK,
            Json(CustomFieldDefinitionResponse::from(def)),
        )
            .into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Custom field not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a custom-field definition.
#[utoipa::path(
    delete,
    path = "/api/v1/custom-fields/{id}",
    tag = "custom-fields",
    params(("id" = String, Path, description = "Custom field id")),
    responses(
        (status = 204, description = "Custom field deleted"),
        (status = 404, description = "Custom field not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Custom field delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Custom field delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_custom_field(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Delete).await {
        return handle_app_error(error);
    }
    match state.services.custom_field_definitions.delete(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound("Custom field not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

// ── Choice-set endpoints ────────────────────────────────────────────────────

/// List custom-field choice sets.
#[utoipa::path(
    get,
    path = "/api/v1/custom-field-choice-sets",
    tag = "custom-fields",
    params(ChoiceSetListQuery),
    responses(
        (status = 200, description = "Custom field choice sets", body = ChoiceSetListResponse),
        (status = "4XX", description = "Choice set list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Choice set list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_custom_field_choice_sets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ChoiceSetListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }
    let (limit, offset) = normalize_page(query.limit, query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CUSTOM_FIELD_CHOICE_SET_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let filter = CustomFieldChoiceSetListFilter {
        search: query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        sort,
    };
    let items = match state
        .services
        .custom_field_choice_sets
        .list_page(&filter, limit, offset)
        .await
    {
        Ok(sets) => sets.into_iter().map(ChoiceSetResponse::from).collect(),
        Err(error) => return handle_app_error(error),
    };
    let total = match state.services.custom_field_choice_sets.count(&filter).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    (
        StatusCode::OK,
        Json(ChoiceSetListResponse {
            items,
            total,
            limit,
            offset,
        }),
    )
        .into_response()
}

/// Get a custom-field choice set.
#[utoipa::path(
    get,
    path = "/api/v1/custom-field-choice-sets/{id}",
    tag = "custom-fields",
    params(("id" = String, Path, description = "Choice set id")),
    responses(
        (status = 200, description = "Custom field choice set", body = ChoiceSetResponse),
        (status = 404, description = "Choice set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Choice set request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Choice set request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_custom_field_choice_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }
    match state.services.custom_field_choice_sets.get(id).await {
        Ok(Some(set)) => (StatusCode::OK, Json(ChoiceSetResponse::from(set))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Choice set not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a custom-field choice set.
#[utoipa::path(
    post,
    path = "/api/v1/custom-field-choice-sets",
    tag = "custom-fields",
    request_body = ChoiceSetCreatePayload,
    responses(
        (status = 201, description = "Choice set created", body = ChoiceSetResponse),
        (status = "4XX", description = "Choice set create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Choice set create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_custom_field_choice_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ChoiceSetCreatePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Create).await {
        return handle_app_error(error);
    }
    let input = NewCustomFieldChoiceSet {
        name: payload.name,
        description: payload.description,
        extra_choices: payload
            .extra_choices
            .into_iter()
            .map(Choice::from)
            .collect(),
        order_alphabetically: payload.order_alphabetically,
        choice_colors: payload.choice_colors,
    };
    match state.services.custom_field_choice_sets.create(&input).await {
        Ok(set) => (StatusCode::CREATED, Json(ChoiceSetResponse::from(set))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a custom-field choice set.
#[utoipa::path(
    patch,
    path = "/api/v1/custom-field-choice-sets/{id}",
    tag = "custom-fields",
    params(("id" = String, Path, description = "Choice set id")),
    request_body = ChoiceSetUpdatePayload,
    responses(
        (status = 200, description = "Choice set updated", body = ChoiceSetResponse),
        (status = 404, description = "Choice set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Choice set update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Choice set update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_custom_field_choice_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<ChoiceSetUpdatePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Update).await {
        return handle_app_error(error);
    }
    let input = UpdateCustomFieldChoiceSet {
        name: payload.name,
        description: payload.description,
        extra_choices: payload
            .extra_choices
            .map(|choices| choices.into_iter().map(Choice::from).collect()),
        order_alphabetically: payload.order_alphabetically,
        choice_colors: payload.choice_colors,
    };
    match state
        .services
        .custom_field_choice_sets
        .update(id, &input)
        .await
    {
        Ok(Some(set)) => (StatusCode::OK, Json(ChoiceSetResponse::from(set))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Choice set not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a custom-field choice set.
#[utoipa::path(
    delete,
    path = "/api/v1/custom-field-choice-sets/{id}",
    tag = "custom-fields",
    params(("id" = String, Path, description = "Choice set id")),
    responses(
        (status = 204, description = "Choice set deleted"),
        (status = 404, description = "Choice set not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Choice set delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Choice set delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_custom_field_choice_set(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Delete).await {
        return handle_app_error(error);
    }
    match state.services.custom_field_choice_sets.delete(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound("Choice set not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

// ── Content-type listing ─────────────────────────────────────────────────────

/// List the seeded inventory content types for the custom-field scope picker.
///
/// Content-type ids are minted per-install, so the picker reads them here
/// (labeled, with the install-portable `app_label.model` token) instead of
/// hardcoding uuids.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/content-types",
    tag = "custom-fields",
    params(ContentTypeListQuery),
    responses(
        (status = 200, description = "Inventory content types", body = [ContentTypeResponse]),
        (status = "4XX", description = "Content type list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Content type list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_content_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ContentTypeListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(&state, &request_context, Action::Read).await {
        return handle_app_error(error);
    }
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CONTENT_TYPE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let app_labels = parse_csv_strings(query.app_label.as_deref());
    let models = parse_csv_strings(query.model.as_deref());
    match state
        .services
        .custom_field_definitions
        .list_content_types(sort, &app_labels, &models)
        .await
    {
        Ok(rows) => {
            let items: Vec<ContentTypeResponse> =
                rows.into_iter()
                    .map(|row| {
                        let label = content_type_label(&row.app_label, &row.model);
                        let token = format!("{}.{}", row.app_label, row.model);
                        ContentTypeResponse {
                            id: row.id,
                            app_label: row.app_label,
                            model: row.model,
                            supported_for_custom_fields: supports_custom_fields_scope(&token),
                            capabilities:
                                crate::contexts::inventory::domain::content_type::by_token(&token)
                                    .and_then(|def| def.capabilities()),
                            token,
                            label,
                        }
                    })
                    .collect();
            (StatusCode::OK, Json(items)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

// ── Per-entity secret reveal ─────────────────────────────────────────────────

/// Reveal the cleartext secret custom-field values of one inventory entity.
///
/// The `{entity}` path segment is one of `sites`, `devices`, `regions`,
/// `clusters`, `tenants`, `virtual-machines`, `virtual-machine-types`. The reveal is gated per entity by the attribute-based
/// `Action::Secret` check against the fetched entity's attributes, and the
/// access is audit-logged.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/{entity}/{id}/custom-fields/reveal",
    tag = "custom-fields",
    params(
        ("entity" = String, Path, description = "Entity kind: sites | devices | regions | clusters | tenants | virtual-machines | virtual-machine-types"),
        ("id" = String, Path, description = "Entity id"),
    ),
    responses(
        (status = 200, description = "Revealed secret custom fields", body = CustomFieldRevealResponse),
        (status = 403, description = "Secret reveal denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Entity not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Reveal request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Reveal request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn reveal_entity_custom_fields(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path((entity, id)): Path<(String, Uuid)>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let kind = match InventoryEntityKind::parse(&entity) {
        Some(kind) => kind,
        None => {
            return handle_app_error(AppError::NotFound(format!(
                "unknown inventory entity '{entity}'"
            )));
        }
    };

    let read_access = if matches!(kind, InventoryEntityKind::VirtualMachineType) {
        crate::api::permission_helpers::ensure_access_with_resource(
            &state,
            &request_context,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await
    } else {
        ensure_access(&state, &request_context, Action::Read).await
    };
    if let Err(error) = read_access {
        return handle_app_error(error);
    }

    // Fetch the entity to obtain its stored custom-field data and the attributes
    // that drive the per-entity secret authz decision.
    let (raw_custom_fields, attributes) = match fetch_entity_custom_fields(&state, kind, id).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let access_control = build_access_control_service_from_repos(&state.repositories);
    let can_read =
        match can_read_entity_secret(&access_control, &request_context, id, attributes).await {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    if !can_read {
        return handle_app_error(AppError::Forbidden(
            "Secret custom-field reveal denied for this entity".to_string(),
        ));
    }
    let actor_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => Some(user_id),
        Err(error) => return handle_app_error(error),
    };

    let view = match state
        .services
        .custom_field_values
        .reveal_for_entity_token(kind.content_type_token(), id, &raw_custom_fields)
        .await
    {
        Ok(view) => view,
        Err(error) => return handle_app_error(error),
    };

    let revealed_keys: Vec<String> = view
        .fields
        .iter()
        .filter(|field| field.is_secret)
        .map(|field| field.name.clone())
        .collect();
    let event = AuditEvent {
        id: Uuid::now_v7(),
        event_type: "inventory.custom_fields.reveal".to_string(),
        actor_id,
        actor_kind: request_context.audit_actor_kind().to_string(),
        target_type: Some(format!("inventory:{}", kind.content_type_token())),
        target_id: Some(id),
        metadata: serde_json::json!({ "keys": revealed_keys }),
        created_at: Utc::now(),
    };
    if let Err(error) = state.services.audit.record_event(event).await {
        return handle_app_error(error);
    }

    (
        StatusCode::OK,
        Json(CustomFieldRevealResponse {
            custom_fields: view.to_value_map(),
        }),
    )
        .into_response()
}

/// Fetch an inventory entity's stored custom-field data and the attributes that
/// drive the per-entity secret authz decision.
async fn fetch_entity_custom_fields(
    state: &AppState,
    kind: InventoryEntityKind,
    id: Uuid,
) -> Result<(Value, Value), AppError> {
    let inventory = state.services.inventory_admin.clone();
    match kind {
        InventoryEntityKind::VirtualMachineType => {
            use crate::contexts::inventory::infrastructure::persistence::VirtualMachineTypeRepository;
            let item = state
                .repositories
                .virtual_machine_types
                .get_by_id(id)
                .await?
                .ok_or_else(|| AppError::NotFound("Virtual machine type not found".into()))?;
            // Types are global reference objects: they have no site or tenant.
            // Secret policies may constrain their id or default platform; site/
            // tenant-scoped grants remain fail-closed for this entity kind.
            Ok((
                item.custom_fields.unwrap_or(Value::Null),
                serde_json::json!({"platform_id": item.default_platform_id}),
            ))
        }
        InventoryEntityKind::Site => match inventory.get_site(id).await {
            Ok(Some(site)) => Ok((
                site.custom_fields.clone().unwrap_or(Value::Null),
                serde_json::json!({
                    "site_id": site.id,
                    "tenant_id": site.tenant_id,
                    "status": site.status,
                }),
            )),
            Ok(None) => Err(AppError::NotFound("Site not found".to_string())),
            Err(error) => Err(error),
        },
        InventoryEntityKind::Device => match inventory.get_device(id).await {
            Ok(Some(device)) => Ok((
                device.custom_fields.clone().unwrap_or(Value::Null),
                serde_json::json!({
                    "site_id": device.site_id,
                    "role_id": device.role_id,
                    "tenant_id": device.tenant_id,
                    "status": device.status,
                }),
            )),
            Ok(None) => Err(AppError::NotFound("Device not found".to_string())),
            Err(error) => Err(error),
        },
        InventoryEntityKind::Region => match inventory.get_region(id).await {
            Ok(Some(region)) => Ok((
                region.custom_fields.clone().unwrap_or(Value::Null),
                serde_json::json!({
                    "region_id": region.id,
                }),
            )),
            Ok(None) => Err(AppError::NotFound("Region not found".to_string())),
            Err(error) => Err(error),
        },
        InventoryEntityKind::Cluster => match inventory.get_cluster(id).await {
            Ok(Some(cluster)) => Ok((
                cluster.custom_fields.clone().unwrap_or(Value::Null),
                serde_json::json!({
                    "site_id": cluster.site_id,
                    "tenant_id": cluster.tenant_id,
                    "status": cluster.status,
                }),
            )),
            Ok(None) => Err(AppError::NotFound("Cluster not found".to_string())),
            Err(error) => Err(error),
        },
        InventoryEntityKind::Tenant => match inventory.get_tenant(id).await {
            Ok(Some(tenant)) => Ok((
                tenant.custom_fields.clone().unwrap_or(Value::Null),
                serde_json::json!({
                    "tenant_id": tenant.id,
                }),
            )),
            Ok(None) => Err(AppError::NotFound("Tenant not found".to_string())),
            Err(error) => Err(error),
        },
        InventoryEntityKind::VirtualMachine => match inventory.get_virtual_machine(id).await {
            Ok(Some(vm)) => Ok((
                vm.custom_fields.clone().unwrap_or(Value::Null),
                serde_json::json!({
                    "site_id": vm.site_id,
                    "role_id": vm.role_id,
                    "tenant_id": vm.tenant_id,
                    "status": vm.status,
                }),
            )),
            Ok(None) => Err(AppError::NotFound("Virtual machine not found".to_string())),
            Err(error) => Err(error),
        },
    }
}

/// The inventory entity kinds that carry per-entity custom fields, with their
/// NetBox content-type token.
#[derive(Debug, Clone, Copy)]
enum InventoryEntityKind {
    Site,
    Device,
    Region,
    Cluster,
    Tenant,
    VirtualMachine,
    VirtualMachineType,
}

impl InventoryEntityKind {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "sites" => Some(Self::Site),
            "devices" => Some(Self::Device),
            "regions" => Some(Self::Region),
            "clusters" => Some(Self::Cluster),
            "tenants" => Some(Self::Tenant),
            "virtual-machines" => Some(Self::VirtualMachine),
            "virtual-machine-types" => Some(Self::VirtualMachineType),
            _ => None,
        }
    }

    fn content_type_token(self) -> &'static str {
        match self {
            Self::Site => CONTENT_TYPE_SITE,
            Self::Device => CONTENT_TYPE_DEVICE,
            Self::Region => CONTENT_TYPE_REGION,
            Self::Cluster => CONTENT_TYPE_CLUSTER,
            Self::Tenant => CONTENT_TYPE_TENANT,
            Self::VirtualMachine => CONTENT_TYPE_VIRTUAL_MACHINE,
            Self::VirtualMachineType => CONTENT_TYPE_VIRTUAL_MACHINE_TYPE,
        }
    }
}

pub const CONTENT_TYPE_SITE: &str = "dcim.site";
pub const CONTENT_TYPE_DEVICE: &str = "dcim.device";
pub const CONTENT_TYPE_REGION: &str = "dcim.region";
pub const CONTENT_TYPE_CLUSTER: &str = "virtualization.cluster";
pub const CONTENT_TYPE_TENANT: &str = "tenancy.tenant";
pub const CONTENT_TYPE_VIRTUAL_MACHINE: &str = "virtualization.virtualmachine";
pub const CONTENT_TYPE_VIRTUAL_MACHINE_TYPE: &str = "virtualization.virtualmachinetype";

// ── Payload → domain ────────────────────────────────────────────────────────

fn build_new_definition(
    payload: CustomFieldDefinitionCreatePayload,
) -> Result<NewCustomFieldDefinition, AppError> {
    let field_type = parse_field_type(&payload.field_type)?;
    let ui_visible = match payload.ui_visible.as_deref() {
        Some(raw) => parse_ui_visible(raw)?,
        None => UiVisible::Always,
    };
    let ui_editable = match payload.ui_editable.as_deref() {
        Some(raw) => parse_ui_editable(raw)?,
        None => UiEditable::Yes,
    };
    let filter_logic = match payload.filter_logic.as_deref() {
        Some(raw) => parse_filter_logic(raw)?,
        None => FilterLogic::Loose,
    };
    Ok(NewCustomFieldDefinition {
        name: payload.name,
        label: payload.label,
        field_type,
        description: payload.description,
        group_name: payload.group_name,
        required: payload.required,
        enforce_unique: payload.enforce_unique,
        is_secret: payload.is_secret,
        default_value: payload.default_value,
        weight: payload.weight,
        validation_minimum: payload.validation_minimum,
        validation_maximum: payload.validation_maximum,
        validation_regex: payload.validation_regex,
        validation_schema: payload.validation_schema,
        choice_set_id: payload.choice_set_id,
        related_object_type_id: payload.related_object_type_id,
        related_object_filter: payload.related_object_filter,
        ui_visible,
        ui_editable,
        is_cloneable: payload.is_cloneable,
        search_weight: payload.search_weight,
        filter_logic,
        object_types: payload.object_types,
    })
}

fn build_update_definition(
    payload: CustomFieldDefinitionUpdatePayload,
) -> Result<UpdateCustomFieldDefinition, AppError> {
    let field_type = match payload.field_type.as_deref() {
        Some(raw) => Some(parse_field_type(raw)?),
        None => None,
    };
    let ui_visible = match payload.ui_visible.as_deref() {
        Some(raw) => Some(parse_ui_visible(raw)?),
        None => None,
    };
    let ui_editable = match payload.ui_editable.as_deref() {
        Some(raw) => Some(parse_ui_editable(raw)?),
        None => None,
    };
    let filter_logic = match payload.filter_logic.as_deref() {
        Some(raw) => Some(parse_filter_logic(raw)?),
        None => None,
    };
    Ok(UpdateCustomFieldDefinition {
        label: payload.label,
        field_type,
        description: payload.description,
        group_name: payload.group_name,
        required: payload.required,
        enforce_unique: payload.enforce_unique,
        is_secret: payload.is_secret,
        default_value: payload.default_value,
        weight: payload.weight,
        validation_minimum: payload.validation_minimum,
        validation_maximum: payload.validation_maximum,
        validation_regex: payload.validation_regex,
        validation_schema: payload.validation_schema,
        choice_set_id: payload.choice_set_id,
        related_object_type_id: payload.related_object_type_id,
        related_object_filter: payload.related_object_filter,
        ui_visible,
        ui_editable,
        is_cloneable: payload.is_cloneable,
        search_weight: payload.search_weight,
        filter_logic,
        object_types: payload.object_types,
    })
}
