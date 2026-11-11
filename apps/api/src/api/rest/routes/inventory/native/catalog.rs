//! Catalog REST handlers: application roles, applications, hierarchical flow
//! groups and reusable address groups.
//!
//! Persistence belongs to `SqlxCatalogRepository`; these handlers own request
//! shape, the Inventory action gate and the list envelope. Create answers 201
//! and delete 204. Nullable fields follow the inventory patch contract:
//! omission preserves, explicit null clears.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use super::owners::present;
use super::super::{
    InventoryListEnvelope, apply_custom_field_secret_intents, masked_custom_fields,
    masked_custom_fields_page, parse_optional_uuid, parse_uuid,
    validate_and_split_custom_fields,
};
use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::permission_helpers::{
    ensure_access_with_resource, ensure_inventory_collection_access,
};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::{build_request_context, project_inventory_delete_error};
use crate::api::rest::routes::validation::deserialize_nullable_patch;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::native::catalog::{
    AddressGroup, AddressGroupMember, Application, ApplicationRole, DataflowGroup,
};
use crate::contexts::inventory::domain::native::dataflow::{
    DataflowAddressMode, DataflowEndpointInput, DataflowStatus, EndpointType,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::native::catalog_repository::{
    AddressGroupChanges, AddressGroupFilter, ApplicationChanges, ApplicationFilter,
    ApplicationRoleChanges, ApplicationRoleFilter, DataflowGroupChanges, DataflowGroupFilter,
};
use crate::contexts::shared::list_query::{parse_csv_strings, parse_csv_uuids, search_pattern};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

// ---------------- shared responses ----------------

fn entity_response<T: serde::Serialize>(
    result: Result<Option<T>, AppError>,
    label: &str,
) -> Response {
    match result {
        Ok(Some(item)) => Json(item).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound(format!("{label} not found"))),
        Err(error) => handle_app_error(error),
    }
}

fn created_response<T: serde::Serialize>(result: Result<T, AppError>) -> Response {
    match result {
        Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

async fn delete_response(
    state: &AppState,
    context: &RequestContext,
    result: Result<(), AppError>,
) -> Response {
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(project_inventory_delete_error(state, context, error).await),
    }
}

fn item_resource(id: Uuid) -> String {
    format!("inventory:{id}")
}

const APPLICATION_CONTENT_TYPE: &str = "inventory.application";
const DATAFLOW_GROUP_CONTENT_TYPE: &str = "inventory.dataflowgroup";
const ADDRESS_GROUP_CONTENT_TYPE: &str = "inventory.addressgroup";

/// Validate/split a catalog custom-field write through the canonical inventory
/// custom-field owner. `None` means the field was omitted (update preserves it).
async fn split_custom_fields(
    state: &AppState,
    content_type: &str,
    incoming: Option<serde_json::Value>,
    require_complete: bool,
) -> Result<
    (
        Option<serde_json::Value>,
        Vec<crate::contexts::custom_fields::SecretWriteIntent>,
    ),
    AppError,
> {
    Ok(
        match validate_and_split_custom_fields(state, content_type, incoming, require_complete)
            .await?
        {
            Some(outcome) => (Some(outcome.custom_field_data), outcome.secret_intents),
            None => (None, Vec::new()),
        },
    )
}

/// Mask one catalog row's custom fields for a response.
async fn masked_entity_custom_fields(
    state: &AppState,
    content_type: &str,
    id: Uuid,
    raw: &serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let masked = masked_custom_fields(state, content_type, id, raw).await?;
    Ok(if masked.is_null() {
        empty_object()
    } else {
        masked
    })
}

/// Mask a catalog list page in one batched read.
async fn mask_catalog_page<T>(
    state: &AppState,
    content_type: &str,
    items: &mut [T],
    id: impl Fn(&T) -> Uuid,
    raw: impl Fn(&T) -> serde_json::Value,
    set: impl Fn(&mut T, serde_json::Value),
) -> Result<(), AppError> {
    let entities: Vec<_> = items.iter().map(|item| (id(item), raw(item))).collect();
    let masked = masked_custom_fields_page(state, content_type, entities).await?;
    for (item, value) in items.iter_mut().zip(masked) {
        set(
            item,
            if value.is_null() {
                empty_object()
            } else {
                value
            },
        );
    }
    Ok(())
}

fn parse_status_filter(raw: Option<&str>) -> Result<Vec<String>, AppError> {
    let mut values: Vec<String> = Vec::new();
    for value in parse_csv_strings(raw) {
        let normalized = value.to_ascii_lowercase();
        if !matches!(normalized.as_str(), "draft" | "active" | "disabled") {
            return Err(AppError::Validation(format!(
                "Invalid status value {value:?}: expected one of: draft, active, disabled"
            )));
        }
        if !values.contains(&normalized) {
            values.push(normalized);
        }
    }
    Ok(values)
}

// ---------------- application roles ----------------

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct ApplicationRoleListQuery {
    pub search: Option<String>,
    pub owner_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplicationRoleCreatePayload {
    #[schema(min_length = 1, max_length = 200)]
    pub name: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplicationRoleUpdatePayload {
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false, min_length = 1, max_length = 200)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub comments: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub owner_id: Option<Option<Uuid>>,
}

/// List application roles.
#[utoipa::path(get, path = "/api/v1/inventory/application-roles", tag = "inventory", params(ApplicationRoleListQuery), responses((status = 200, description = "Application roles", body = InventoryListEnvelope<ApplicationRole>), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_application_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ApplicationRoleListQuery>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Read).await {
        return handle_app_error(error);
    }
    let owner_id = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(values) => values,
        Err(error) => return handle_app_error(error),
    };
    let filter = ApplicationRoleFilter {
        search: search_pattern(query.search.as_deref()),
        owner_id,
    };
    match state
        .repositories
        .catalog
        .list_application_roles(&filter, query.limit, query.offset)
        .await
    {
        Ok((items, total)) => Json(InventoryListEnvelope {
            items,
            total,
            limit: clamp_limit(query.limit),
            offset: clamp_offset(query.offset),
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get an application role.
#[utoipa::path(get, path = "/api/v1/inventory/application-roles/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 200, description = "Application role", body = ApplicationRole), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_application_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Read,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    entity_response(
        state.repositories.catalog.get_application_role(id).await,
        "Application role",
    )
}

/// Create an application role.
#[utoipa::path(post, path = "/api/v1/inventory/application-roles", tag = "inventory", request_body = ApplicationRoleCreatePayload, responses((status = 201, description = "Created", body = ApplicationRole), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_application_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ApplicationRoleCreatePayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    let mut role = ApplicationRole {
        id: Uuid::now_v7(),
        name: payload.name,
        description: payload.description,
        comments: payload.comments,
        owner_id: payload.owner_id,
    };
    if let Err(error) = role.normalize() {
        return handle_app_error(error);
    }
    created_response(
        state
            .repositories
            .catalog
            .create_application_role(&role)
            .await,
    )
}

/// Update an application role.
#[utoipa::path(put, path = "/api/v1/inventory/application-roles/{id}", tag = "inventory", params(("id" = Uuid, Path)), request_body = ApplicationRoleUpdatePayload, responses((status = 200, description = "Updated", body = ApplicationRole), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_application_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ApplicationRoleUpdatePayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Update,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    let changes = ApplicationRoleChanges {
        name: payload.name,
        description: payload.description,
        comments: payload.comments,
        owner_id: payload.owner_id,
    };
    entity_response(
        state
            .repositories
            .catalog
            .update_application_role(id, &changes)
            .await,
        "Application role",
    )
}

/// Delete an application role.
#[utoipa::path(delete, path = "/api/v1/inventory/application-roles/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 204, description = "Deleted"), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_application_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Delete,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    delete_response(
        &state,
        &context,
        state.repositories.catalog.delete_application_role(id).await,
    )
    .await
}

// ---------------- applications ----------------

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct ApplicationListQuery {
    pub search: Option<String>,
    pub role_id: Option<String>,
    pub tenant_id: Option<String>,
    pub owner_id: Option<String>,
    pub tag_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplicationCreatePayload {
    #[schema(min_length = 1, max_length = 200)]
    pub name: String,
    pub role_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    #[serde(default)]
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub custom_field_data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplicationUpdatePayload {
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false, min_length = 1, max_length = 200)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub role_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub comments: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub owner_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub tenant_id: Option<Option<Uuid>>,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    pub custom_field_data: Option<serde_json::Value>,
}

/// List applications.
#[utoipa::path(get, path = "/api/v1/inventory/applications", tag = "inventory", params(ApplicationListQuery), responses((status = 200, description = "Applications", body = InventoryListEnvelope<Application>), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_applications(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ApplicationListQuery>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Read).await {
        return handle_app_error(error);
    }
    let filter = match application_filter(&query) {
        Ok(filter) => filter,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .catalog
        .list_applications(&filter, query.limit, query.offset)
        .await
    {
        Ok((mut items, total)) => {
            if let Err(error) = mask_catalog_page(
                &state,
                APPLICATION_CONTENT_TYPE,
                &mut items,
                |item| item.id,
                |item| item.custom_field_data.clone(),
                |item, value| item.custom_field_data = value,
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(InventoryListEnvelope {
                items,
                total,
                limit: clamp_limit(query.limit),
                offset: clamp_offset(query.offset),
            })
            .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

fn application_filter(query: &ApplicationListQuery) -> Result<ApplicationFilter, AppError> {
    Ok(ApplicationFilter {
        search: search_pattern(query.search.as_deref()),
        role_id: parse_csv_uuids(query.role_id.as_deref(), "role_id")?,
        tenant_id: parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id")?,
        owner_id: parse_csv_uuids(query.owner_id.as_deref(), "owner_id")?,
        tag_id: parse_csv_uuids(query.tag_id.as_deref(), "tag_id")?,
    })
}

/// Get an application.
#[utoipa::path(get, path = "/api/v1/inventory/applications/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 200, description = "Application", body = Application), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_application(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Read,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.repositories.catalog.get_application(id).await {
        Ok(Some(mut item)) => match masked_entity_custom_fields(
            &state,
            APPLICATION_CONTENT_TYPE,
            item.id,
            &item.custom_field_data,
        )
        .await
        {
            Ok(value) => {
                item.custom_field_data = value;
                Json(item).into_response()
            }
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Application not found".into())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an application.
#[utoipa::path(post, path = "/api/v1/inventory/applications", tag = "inventory", request_body = ApplicationCreatePayload, responses((status = 201, description = "Created", body = Application), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_application(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ApplicationCreatePayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    let (custom_field_data, intents) = match split_custom_fields(
        &state,
        APPLICATION_CONTENT_TYPE,
        Some(payload.custom_field_data.unwrap_or_else(empty_object)),
        true,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let mut application = Application {
        id: Uuid::now_v7(),
        name: payload.name,
        role_id: payload.role_id,
        description: payload.description,
        comments: payload.comments,
        owner_id: payload.owner_id,
        tenant_id: payload.tenant_id,
        tags: payload.tags,
        custom_field_data: custom_field_data.unwrap_or_else(empty_object),
    };
    if let Err(error) = application.normalize() {
        return handle_app_error(error);
    }
    let created = match state
        .repositories
        .catalog
        .create_application(&application)
        .await
    {
        Ok(item) => item,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = apply_custom_field_secret_intents(&state, created.id, intents).await {
        return handle_app_error(error);
    }
    match masked_entity_custom_fields(
        &state,
        APPLICATION_CONTENT_TYPE,
        created.id,
        &created.custom_field_data,
    )
    .await
    {
        Ok(value) => {
            let mut response = created;
            response.custom_field_data = value;
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update an application.
#[utoipa::path(put, path = "/api/v1/inventory/applications/{id}", tag = "inventory", params(("id" = Uuid, Path)), request_body = ApplicationUpdatePayload, responses((status = 200, description = "Updated", body = Application), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_application(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ApplicationUpdatePayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Update,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    let (custom_field_data, intents) = match split_custom_fields(
        &state,
        APPLICATION_CONTENT_TYPE,
        payload.custom_field_data,
        false,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let changes = ApplicationChanges {
        name: payload.name,
        role_id: payload.role_id,
        description: payload.description,
        comments: payload.comments,
        owner_id: payload.owner_id,
        tenant_id: payload.tenant_id,
        tags: payload.tags,
        custom_field_data,
    };
    let updated = match state
        .repositories
        .catalog
        .update_application(id, &changes)
        .await
    {
        Ok(Some(item)) => item,
        Ok(None) => return handle_app_error(AppError::NotFound("Application not found".into())),
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = apply_custom_field_secret_intents(&state, updated.id, intents).await {
        return handle_app_error(error);
    }
    match masked_entity_custom_fields(
        &state,
        APPLICATION_CONTENT_TYPE,
        updated.id,
        &updated.custom_field_data,
    )
    .await
    {
        Ok(value) => {
            let mut response = updated;
            response.custom_field_data = value;
            Json(response).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete an application.
#[utoipa::path(delete, path = "/api/v1/inventory/applications/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 204, description = "Deleted"), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_application(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Delete,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    delete_response(
        &state,
        &context,
        state.repositories.catalog.delete_application(id).await,
    )
    .await
}

// ---------------- flow groups ----------------

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct DataflowGroupListQuery {
    pub search: Option<String>,
    /// Stored status (`draft`, `active`, `disabled`); the effective status is a
    /// read projection, not a filter.
    pub status: Option<String>,
    pub application_id: Option<String>,
    /// Direct parent only.
    pub parent_id: Option<String>,
    /// `root=true` selects rows without a parent; it cannot be combined with
    /// `parent_id`.
    pub root: Option<bool>,
    pub tenant_id: Option<String>,
    pub owner_id: Option<String>,
    pub tag_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DataflowGroupCreatePayload {
    #[schema(min_length = 1, max_length = 200)]
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
    /// New groups default to `active` so they do not pause existing flows.
    pub status: Option<DataflowStatus>,
    pub owner_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    #[serde(default)]
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub custom_field_data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DataflowGroupUpdatePayload {
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false, min_length = 1, max_length = 200)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub parent_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub application_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub comments: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub status: Option<DataflowStatus>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub owner_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub tenant_id: Option<Option<Uuid>>,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    pub custom_field_data: Option<serde_json::Value>,
}

/// List flow groups.
#[utoipa::path(get, path = "/api/v1/inventory/dataflow-groups", tag = "inventory", params(DataflowGroupListQuery), responses((status = 200, description = "Flow groups", body = InventoryListEnvelope<DataflowGroup>), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_dataflow_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<DataflowGroupListQuery>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Read).await {
        return handle_app_error(error);
    }
    let filter = match dataflow_group_filter(&query) {
        Ok(filter) => filter,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .catalog
        .list_dataflow_groups(&filter, query.limit, query.offset)
        .await
    {
        Ok((mut items, total)) => {
            if let Err(error) = mask_catalog_page(
                &state,
                DATAFLOW_GROUP_CONTENT_TYPE,
                &mut items,
                |item| item.id,
                |item| item.custom_field_data.clone(),
                |item, value| item.custom_field_data = value,
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(InventoryListEnvelope {
                items,
                total,
                limit: clamp_limit(query.limit),
                offset: clamp_offset(query.offset),
            })
            .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

fn dataflow_group_filter(query: &DataflowGroupListQuery) -> Result<DataflowGroupFilter, AppError> {
    let parent_id = parse_optional_uuid(query.parent_id.clone(), "parent_id")?;
    let root = query.root.unwrap_or(false);
    if root && parent_id.is_some() {
        return Err(AppError::Validation(
            "root=true and parent_id are mutually exclusive".into(),
        ));
    }
    Ok(DataflowGroupFilter {
        search: search_pattern(query.search.as_deref()),
        status: parse_status_filter(query.status.as_deref())?,
        application_id: parse_csv_uuids(query.application_id.as_deref(), "application_id")?,
        parent_id,
        root,
        tenant_id: parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id")?,
        owner_id: parse_csv_uuids(query.owner_id.as_deref(), "owner_id")?,
        tag_id: parse_csv_uuids(query.tag_id.as_deref(), "tag_id")?,
    })
}

/// Get a flow group.
#[utoipa::path(get, path = "/api/v1/inventory/dataflow-groups/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 200, description = "Flow group", body = DataflowGroup), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_dataflow_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Read,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.repositories.catalog.get_dataflow_group(id).await {
        Ok(Some(mut item)) => match masked_entity_custom_fields(
            &state,
            DATAFLOW_GROUP_CONTENT_TYPE,
            item.id,
            &item.custom_field_data,
        )
        .await
        {
            Ok(value) => {
                item.custom_field_data = value;
                Json(item).into_response()
            }
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Flow group not found".into())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a flow group.
#[utoipa::path(post, path = "/api/v1/inventory/dataflow-groups", tag = "inventory", request_body = DataflowGroupCreatePayload, responses((status = 201, description = "Created", body = DataflowGroup), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_dataflow_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<DataflowGroupCreatePayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    let (custom_field_data, intents) = match split_custom_fields(
        &state,
        DATAFLOW_GROUP_CONTENT_TYPE,
        Some(payload.custom_field_data.unwrap_or_else(empty_object)),
        true,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let mut group = DataflowGroup {
        id: Uuid::now_v7(),
        parent_id: payload.parent_id,
        application_id: payload.application_id,
        name: payload.name,
        description: payload.description,
        comments: payload.comments,
        status: payload.status.unwrap_or(DataflowStatus::Active),
        owner_id: payload.owner_id,
        tenant_id: payload.tenant_id,
        tags: payload.tags,
        custom_field_data: custom_field_data.unwrap_or_else(empty_object),
        effective_status: DataflowStatus::Active,
        blocked_by_group_id: None,
        inherited_tags: Vec::new(),
        effective_tags: Vec::new(),
    };
    if let Err(error) = group.normalize() {
        return handle_app_error(error);
    }
    let created = match state
        .repositories
        .catalog
        .create_dataflow_group(&group)
        .await
    {
        Ok(item) => item,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = apply_custom_field_secret_intents(&state, created.id, intents).await {
        return handle_app_error(error);
    }
    match masked_entity_custom_fields(
        &state,
        DATAFLOW_GROUP_CONTENT_TYPE,
        created.id,
        &created.custom_field_data,
    )
    .await
    {
        Ok(value) => {
            let mut response = created;
            response.custom_field_data = value;
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a flow group.
#[utoipa::path(put, path = "/api/v1/inventory/dataflow-groups/{id}", tag = "inventory", params(("id" = Uuid, Path)), request_body = DataflowGroupUpdatePayload, responses((status = 200, description = "Updated", body = DataflowGroup), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_dataflow_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<DataflowGroupUpdatePayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Update,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    let (custom_field_data, intents) = match split_custom_fields(
        &state,
        DATAFLOW_GROUP_CONTENT_TYPE,
        payload.custom_field_data,
        false,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let changes = DataflowGroupChanges {
        name: payload.name,
        parent_id: payload.parent_id,
        application_id: payload.application_id,
        description: payload.description,
        comments: payload.comments,
        status: payload.status,
        owner_id: payload.owner_id,
        tenant_id: payload.tenant_id,
        tags: payload.tags,
        custom_field_data,
    };
    let updated = match state
        .repositories
        .catalog
        .update_dataflow_group(id, &changes)
        .await
    {
        Ok(Some(item)) => item,
        Ok(None) => return handle_app_error(AppError::NotFound("Flow group not found".into())),
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = apply_custom_field_secret_intents(&state, updated.id, intents).await {
        return handle_app_error(error);
    }
    match masked_entity_custom_fields(
        &state,
        DATAFLOW_GROUP_CONTENT_TYPE,
        updated.id,
        &updated.custom_field_data,
    )
    .await
    {
        Ok(value) => {
            let mut response = updated;
            response.custom_field_data = value;
            Json(response).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a flow group.
#[utoipa::path(delete, path = "/api/v1/inventory/dataflow-groups/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 204, description = "Deleted"), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_dataflow_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Delete,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    delete_response(
        &state,
        &context,
        state.repositories.catalog.delete_dataflow_group(id).await,
    )
    .await
}

// ---------------- address groups ----------------

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct AddressGroupListQuery {
    pub search: Option<String>,
    pub tenant_id: Option<String>,
    pub owner_id: Option<String>,
    pub tag_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// One ordered member of an address group. Positions are assigned from the
/// array order, so a client never sends one.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddressGroupMemberPayload {
    pub object_type: EndpointType,
    pub object_id: Option<Uuid>,
    #[serde(default)]
    pub address_mode: DataflowAddressMode,
    #[serde(default)]
    pub ipaddresses: Vec<Uuid>,
    #[serde(default)]
    pub tag_object_type: Option<EndpointType>,
}

impl From<AddressGroupMemberPayload> for AddressGroupMember {
    fn from(payload: AddressGroupMemberPayload) -> Self {
        Self {
            selector: DataflowEndpointInput {
                object_type: payload.object_type,
                object_id: payload.object_id,
                address_mode: payload.address_mode,
                ipaddresses: payload.ipaddresses,
                tag_object_type: payload.tag_object_type,
            },
            position: 0,
            label: None,
        }
    }
}

fn members_from_payload(members: Vec<AddressGroupMemberPayload>) -> Vec<AddressGroupMember> {
    members.into_iter().map(AddressGroupMember::from).collect()
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddressGroupCreatePayload {
    #[schema(min_length = 1, max_length = 200)]
    pub name: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    #[serde(default)]
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub custom_field_data: Option<serde_json::Value>,
    #[schema(min_items = 1, max_items = 64)]
    pub members: Vec<AddressGroupMemberPayload>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddressGroupUpdatePayload {
    #[serde(default, deserialize_with = "present")]
    #[schema(nullable = false, min_length = 1, max_length = 200)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub comments: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub owner_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_nullable_patch")]
    pub tenant_id: Option<Option<Uuid>>,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    pub custom_field_data: Option<serde_json::Value>,
    /// Supplying members replaces the whole ordered set; `[]` is rejected.
    #[serde(default, deserialize_with = "present")]
    #[schema(min_items = 1, max_items = 64)]
    pub members: Option<Vec<AddressGroupMemberPayload>>,
}

/// List address groups.
#[utoipa::path(get, path = "/api/v1/inventory/address-groups", tag = "inventory", params(AddressGroupListQuery), responses((status = 200, description = "Address groups", body = InventoryListEnvelope<AddressGroup>), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_address_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<AddressGroupListQuery>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Read).await {
        return handle_app_error(error);
    }
    let filter = match address_group_filter(&query) {
        Ok(filter) => filter,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .catalog
        .list_address_groups(&filter, query.limit, query.offset)
        .await
    {
        Ok((mut items, total)) => {
            if let Err(error) = mask_catalog_page(
                &state,
                ADDRESS_GROUP_CONTENT_TYPE,
                &mut items,
                |item| item.id,
                |item| item.custom_field_data.clone(),
                |item, value| item.custom_field_data = value,
            )
            .await
            {
                return handle_app_error(error);
            }
            Json(InventoryListEnvelope {
                items,
                total,
                limit: clamp_limit(query.limit),
                offset: clamp_offset(query.offset),
            })
            .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

fn address_group_filter(query: &AddressGroupListQuery) -> Result<AddressGroupFilter, AppError> {
    Ok(AddressGroupFilter {
        search: search_pattern(query.search.as_deref()),
        tenant_id: parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id")?,
        owner_id: parse_csv_uuids(query.owner_id.as_deref(), "owner_id")?,
        tag_id: parse_csv_uuids(query.tag_id.as_deref(), "tag_id")?,
    })
}

/// Get an address group.
#[utoipa::path(get, path = "/api/v1/inventory/address-groups/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 200, description = "Address group", body = AddressGroup), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_address_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let context = build_request_context(&request_id, auth, &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Read,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.repositories.catalog.get_address_group(id).await {
        Ok(Some(mut item)) => match masked_entity_custom_fields(
            &state,
            ADDRESS_GROUP_CONTENT_TYPE,
            item.id,
            &item.custom_field_data,
        )
        .await
        {
            Ok(value) => {
                item.custom_field_data = value;
                Json(item).into_response()
            }
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Address group not found".into())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an address group.
#[utoipa::path(post, path = "/api/v1/inventory/address-groups", tag = "inventory", request_body = AddressGroupCreatePayload, responses((status = 201, description = "Created", body = AddressGroup), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_address_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<AddressGroupCreatePayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    let (custom_field_data, intents) = match split_custom_fields(
        &state,
        ADDRESS_GROUP_CONTENT_TYPE,
        Some(payload.custom_field_data.unwrap_or_else(empty_object)),
        true,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let mut group = AddressGroup {
        id: Uuid::now_v7(),
        name: payload.name,
        description: payload.description,
        comments: payload.comments,
        owner_id: payload.owner_id,
        tenant_id: payload.tenant_id,
        tags: payload.tags,
        custom_field_data: custom_field_data.unwrap_or_else(empty_object),
        members: members_from_payload(payload.members),
    };
    if let Err(error) = group.normalize() {
        return handle_app_error(error);
    }
    let created = match state
        .repositories
        .catalog
        .create_address_group(&group)
        .await
    {
        Ok(item) => item,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = apply_custom_field_secret_intents(&state, created.id, intents).await {
        return handle_app_error(error);
    }
    match masked_entity_custom_fields(
        &state,
        ADDRESS_GROUP_CONTENT_TYPE,
        created.id,
        &created.custom_field_data,
    )
    .await
    {
        Ok(value) => {
            let mut response = created;
            response.custom_field_data = value;
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update an address group.
#[utoipa::path(put, path = "/api/v1/inventory/address-groups/{id}", tag = "inventory", params(("id" = Uuid, Path)), request_body = AddressGroupUpdatePayload, responses((status = 200, description = "Updated", body = AddressGroup), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_address_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<AddressGroupUpdatePayload>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Update,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    let (custom_field_data, intents) = match split_custom_fields(
        &state,
        ADDRESS_GROUP_CONTENT_TYPE,
        payload.custom_field_data,
        false,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let changes = AddressGroupChanges {
        name: payload.name,
        description: payload.description,
        comments: payload.comments,
        owner_id: payload.owner_id,
        tenant_id: payload.tenant_id,
        tags: payload.tags,
        custom_field_data,
        members: payload.members.map(members_from_payload),
    };
    let updated = match state
        .repositories
        .catalog
        .update_address_group(id, &changes)
        .await
    {
        Ok(Some(item)) => item,
        Ok(None) => return handle_app_error(AppError::NotFound("Address group not found".into())),
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = apply_custom_field_secret_intents(&state, updated.id, intents).await {
        return handle_app_error(error);
    }
    match masked_entity_custom_fields(
        &state,
        ADDRESS_GROUP_CONTENT_TYPE,
        updated.id,
        &updated.custom_field_data,
    )
    .await
    {
        Ok(value) => {
            let mut response = updated;
            response.custom_field_data = value;
            Json(response).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete an address group.
#[utoipa::path(delete, path = "/api/v1/inventory/address-groups/{id}", tag = "inventory", params(("id" = Uuid, Path)), responses((status = 204, description = "Deleted"), (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_address_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Delete,
        ResourceType::Inventory,
        Some(&item_resource(id)),
    )
    .await
    {
        return handle_app_error(error);
    }
    delete_response(
        &state,
        &context,
        state.repositories.catalog.delete_address_group(id).await,
    )
    .await
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}
