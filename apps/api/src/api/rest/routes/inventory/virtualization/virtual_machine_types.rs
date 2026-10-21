use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::permission_helpers::{
    ensure_access_with_resource, ensure_inventory_collection_access,
};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::VirtualMachineType;
use crate::contexts::inventory::infrastructure::persistence::{
    OwnerRepository, PlatformRepository, VIRTUAL_MACHINE_TYPE_COLUMNS, VirtualMachineTypeChanges,
    VirtualMachineTypeFilter, VirtualMachineTypeRepository,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    apply_custom_field_secret_intents, bump_host_config_cache_generation, masked_custom_fields,
    masked_custom_fields_page, parse_optional_uuid, parse_required_string, parse_uuid,
    push_bulk_error, require_bulk_items, validate_and_split_custom_fields,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

use crate::api::rest::routes::custom_fields::CONTENT_TYPE_VIRTUAL_MACHINE_TYPE as CONTENT_TYPE;

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct VirtualMachineTypeListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    /// Comma-separated default platform UUIDs; matches any (`IN`).
    pub default_platform_id: Option<String>,
    /// Comma-separated owner UUIDs; matches any (`IN`).
    pub owner_id: Option<String>,
}

/// PATCH retains omitted fields and clears explicitly null nullable defaults.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VirtualMachineTypePatchPayload {
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, min_length = 1, max_length = 100)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(
        nullable = false,
        min_length = 1,
        max_length = 100,
        pattern = "^[A-Za-z0-9_-]+$"
    )]
    pub slug: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub default_platform_id: Option<Option<Uuid>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    #[schema(minimum = 0.01, maximum = 9999.99, multiple_of = 0.01)]
    pub default_vcpus: Option<Option<f64>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    #[schema(minimum = 0)]
    pub default_memory: Option<Option<i32>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub owner_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, max_length = 200)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub comments: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub custom_fields: Option<serde_json::Map<String, serde_json::Value>>,
}

impl From<VirtualMachineTypePatchPayload> for VirtualMachineTypeChanges {
    fn from(p: VirtualMachineTypePatchPayload) -> Self {
        Self {
            name: p.name,
            slug: p.slug,
            default_platform_id: p.default_platform_id,
            default_vcpus: p.default_vcpus,
            default_memory: p.default_memory,
            owner_id: p.owner_id,
            description: p.description,
            comments: p.comments,
            custom_fields: p.custom_fields.map(serde_json::Value::Object),
        }
    }
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VirtualMachineTypePayload {
    #[schema(min_length = 1, max_length = 100)]
    pub name: String,
    #[schema(min_length = 1, max_length = 100, pattern = "^[A-Za-z0-9_-]+$")]
    pub slug: String,
    pub default_platform_id: Option<String>,
    #[schema(minimum = 0.01, maximum = 9999.99, multiple_of = 0.01)]
    pub default_vcpus: Option<f64>,
    #[schema(minimum = 0)]
    pub default_memory: Option<i32>,
    pub owner_id: Option<Uuid>,
    #[schema(max_length = 200)]
    pub description: Option<String>,
    pub comments: Option<String>,
    pub custom_fields: Option<serde_json::Value>,
}

struct ParsedVirtualMachineType {
    name: String,
    slug: String,
    default_platform_id: Option<Uuid>,
    default_vcpus: Option<f64>,
    default_memory: Option<i32>,
}

/// Validate the virtual machine type payload, including the upstream range
/// guards (`default_vcpus >= 0.01`, `default_memory >= 0`) and the
/// `default_platform_id` reference. An unknown platform is a client error, not
/// a database failure.
async fn parse_virtual_machine_type_payload(
    state: &AppState,
    payload: &VirtualMachineTypePayload,
) -> Result<ParsedVirtualMachineType, AppError> {
    let name = parse_required_string(&payload.name, "name")?;
    let slug = parse_required_string(&payload.slug, "slug")?;
    validate_type_scalars(&name, &slug, payload.default_vcpus, payload.default_memory)?;
    if payload
        .description
        .as_ref()
        .is_some_and(|value| value.chars().count() > 200)
    {
        return Err(AppError::Validation(
            "description must be at most 200 characters".into(),
        ));
    }
    let default_platform_id =
        parse_optional_uuid(payload.default_platform_id.clone(), "default_platform_id")?;
    if let Some(platform_id) = default_platform_id
        && state
            .repositories
            .platforms
            .get_by_id(platform_id)
            .await?
            .is_none()
    {
        return Err(AppError::Validation(format!(
            "default_platform_id {platform_id} does not exist"
        )));
    }
    if let Some(owner_id) = payload.owner_id
        && state
            .repositories
            .owners
            .get_by_id(owner_id)
            .await?
            .is_none()
    {
        return Err(AppError::Validation("owner_id does not exist".into()));
    }
    Ok(ParsedVirtualMachineType {
        name,
        slug,
        default_platform_id,
        default_vcpus: payload.default_vcpus,
        default_memory: payload.default_memory,
    })
}

fn validate_type_scalars(
    name: &str,
    slug: &str,
    vcpus: Option<f64>,
    memory: Option<i32>,
) -> Result<(), AppError> {
    if name.chars().count() > 100
        || slug.len() > 100
        || !slug
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(AppError::Validation("name and slug must be at most 100 characters; slug must contain only ASCII letters, digits, underscore or hyphen".into()));
    }
    validate_compute_defaults(vcpus, memory)
}

pub(super) fn validate_compute_defaults(
    vcpus: Option<f64>,
    memory: Option<i32>,
) -> Result<(), AppError> {
    if let Some(cpu) = vcpus {
        let scaled = cpu * 100.0;
        if !cpu.is_finite()
            || !(0.01..=9999.99).contains(&cpu)
            || cpu.to_bits() != (scaled.round() / 100.0).to_bits()
        {
            return Err(AppError::Validation(
                "vcpus must be between 0.01 and 9999.99 with at most two decimal places".into(),
            ));
        }
    }
    if memory.is_some_and(|value| value < 0) {
        return Err(AppError::Validation(
            "memory must be zero or greater".into(),
        ));
    }
    Ok(())
}

fn build_virtual_machine_type(
    id: Uuid,
    parsed: ParsedVirtualMachineType,
    payload: VirtualMachineTypePayload,
) -> VirtualMachineType {
    VirtualMachineType {
        id,
        name: parsed.name,
        slug: parsed.slug,
        default_platform_id: parsed.default_platform_id,
        default_vcpus: parsed.default_vcpus,
        default_memory: parsed.default_memory,
        owner_id: payload.owner_id,
        virtual_machine_count: 0,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        custom_fields: payload.custom_fields,
    }
}

async fn create_type_record(
    state: &AppState,
    ctx: &RequestContext,
    payload: VirtualMachineTypePayload,
) -> Result<VirtualMachineType, AppError> {
    let parsed = parse_virtual_machine_type_payload(state, &payload).await?;
    authorize_type_references(state, ctx, parsed.default_platform_id, payload.owner_id).await?;
    let outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE, payload.custom_fields.clone(), true)
            .await?;
    let mut item = build_virtual_machine_type(Uuid::now_v7(), parsed, payload);
    let intents = if let Some(outcome) = outcome {
        item.custom_fields = Some(outcome.custom_field_data);
        outcome.secret_intents
    } else {
        Vec::new()
    };
    state
        .repositories
        .virtual_machine_types
        .create(&item)
        .await?;
    apply_custom_field_secret_intents(state, item.id, intents).await?;
    mask_type_record(state, item).await
}

async fn authorize_type_references(
    state: &AppState,
    ctx: &RequestContext,
    platform_id: Option<Uuid>,
    owner_id: Option<Uuid>,
) -> Result<(), AppError> {
    for id in [platform_id, owner_id].into_iter().flatten() {
        ensure_access_with_resource(
            state,
            ctx,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
    }
    Ok(())
}

async fn mask_type_record(
    state: &AppState,
    mut item: VirtualMachineType,
) -> Result<VirtualMachineType, AppError> {
    item.custom_fields = Some(
        masked_custom_fields(
            state,
            CONTENT_TYPE,
            item.id,
            &item
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null),
        )
        .await?,
    );
    Ok(item)
}

/// List virtual machine types.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-machine-types",
    operation_id = "list_inventory_virtual_machine_types",
    tag = "inventory",
    params(VirtualMachineTypeListQuery),
    responses(
        (status = 200, description = "Virtual machine types", body = InventoryListEnvelope<VirtualMachineType>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_virtual_machine_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VirtualMachineTypeListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_inventory_collection_access(&state, &request_context, Action::Read).await
    {
        return handle_app_error(error);
    }
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        VIRTUAL_MACHINE_TYPE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let default_platform_id =
        match parse_csv_uuids(query.default_platform_id.as_deref(), "default_platform_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let owner_id = match parse_csv_uuids(query.owner_id.as_deref(), "owner_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .virtual_machine_types
        .list_paginated(
            query.limit,
            query.offset,
            VirtualMachineTypeFilter {
                search: crate::contexts::shared::list_query::search_pattern(
                    query.search.as_deref(),
                ),
                sort,
                default_platform_id,
                owner_id,
            },
        )
        .await
    {
        Ok((mut items, total)) => {
            let inputs = items
                .iter()
                .map(|item| {
                    (
                        item.id,
                        item.custom_fields
                            .clone()
                            .unwrap_or(serde_json::Value::Null),
                    )
                })
                .collect();
            let masked = match masked_custom_fields_page(&state, CONTENT_TYPE, inputs).await {
                Ok(values) => values,
                Err(error) => return handle_app_error(error),
            };
            for (item, fields) in items.iter_mut().zip(masked) {
                item.custom_fields = Some(fields);
            }
            (
                StatusCode::OK,
                Json(InventoryListEnvelope {
                    items,
                    total,
                    limit: resolved_limit,
                    offset: resolved_offset,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a virtual machine type.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/virtual-machine-types/{id}",
    operation_id = "get_inventory_virtual_machine_type",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual machine type ID"),
    ),
    responses(
        (status = 200, description = "Virtual machine type", body = VirtualMachineType),
        (status = 404, description = "Virtual machine type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_virtual_machine_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let entity_id = match parse_uuid(&id, "virtual_machine_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
        Some(&format!("inventory:{entity_id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state
        .repositories
        .virtual_machine_types
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => match mask_type_record(&state, item).await {
            Ok(item) => (StatusCode::OK, Json(item)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound(
            "Virtual machine type not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Create a virtual machine type.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-machine-types",
    operation_id = "create_inventory_virtual_machine_type",
    tag = "inventory",
    request_body = VirtualMachineTypePayload,
    responses(
        (status = 201, description = "Virtual machine type created", body = VirtualMachineType),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_virtual_machine_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VirtualMachineTypePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_inventory_collection_access(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }
    match create_type_record(&state, &request_context, payload).await {
        Ok(item) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create virtual machine types.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/virtual-machine-types/bulk",
    operation_id = "bulk_create_inventory_virtual_machine_types",
    tag = "inventory",
    request_body = BulkCreateRequest<VirtualMachineTypePayload>,
    responses(
        (status = 200, description = "Bulk virtual machine type create result", body = BulkCreateResponse<VirtualMachineType>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_virtual_machine_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<VirtualMachineTypePayload>>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_inventory_collection_access(&state, &request_context, Action::Create).await
    {
        return handle_app_error(error);
    }

    let items = match require_bulk_items(request.items) {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };

    let mut created = Vec::new();
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        match create_type_record(&state, &request_context, payload).await {
            Ok(item) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a virtual machine type.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/virtual-machine-types/{id}",
    operation_id = "update_inventory_virtual_machine_type",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual machine type ID"),
    ),
    request_body = VirtualMachineTypePayload,
    responses(
        (status = 200, description = "Virtual machine type updated", body = VirtualMachineType),
        (status = 404, description = "Virtual machine type not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_virtual_machine_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VirtualMachineTypePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let entity_id = match parse_uuid(&id, "virtual_machine_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
        Some(&format!("inventory:{entity_id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    let default_platform_id =
        match parse_optional_uuid(payload.default_platform_id, "default_platform_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let changes = VirtualMachineTypeChanges {
        name: Some(payload.name),
        slug: Some(payload.slug),
        default_platform_id: Some(default_platform_id),
        default_vcpus: Some(payload.default_vcpus),
        default_memory: Some(payload.default_memory),
        owner_id: Some(payload.owner_id),
        description: Some(payload.description.unwrap_or_default()),
        comments: Some(payload.comments.unwrap_or_default()),
        custom_fields: payload.custom_fields,
    };
    match patch_type_record(&state, &request_context, entity_id, changes).await {
        Ok(item) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a virtual machine type's fields.
#[utoipa::path(
    patch, path = "/api/v1/inventory/virtual-machine-types/{id}",
    operation_id = "patch_inventory_virtual_machine_type", tag = "inventory",
    params(("id" = Uuid, Path, description = "Virtual machine type ID")),
    request_body = VirtualMachineTypePatchPayload,
    responses(
        (status = 200, description = "Virtual machine type updated", body = VirtualMachineType),
        (status = "4XX", description = "Rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Failed", body = crate::api::rest::error_mapping::ApiErrorResponse)
    )
)]
pub async fn patch_virtual_machine_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<VirtualMachineTypePatchPayload>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access_with_resource(
        &state,
        &ctx,
        Action::Update,
        ResourceType::Inventory,
        Some(&format!("inventory:{id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    match patch_type_record(&state, &ctx, id, payload.into()).await {
        Ok(item) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

async fn patch_type_record(
    state: &AppState,
    ctx: &RequestContext,
    id: Uuid,
    mut changes: VirtualMachineTypeChanges,
) -> Result<VirtualMachineType, AppError> {
    authorize_type_references(
        state,
        ctx,
        changes.default_platform_id.flatten(),
        changes.owner_id.flatten(),
    )
    .await?;
    let existing = state
        .repositories
        .virtual_machine_types
        .get_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("Virtual machine type not found".into()))?;
    let candidate = VirtualMachineTypePayload {
        name: changes.name.clone().unwrap_or(existing.name),
        slug: changes.slug.clone().unwrap_or(existing.slug),
        default_platform_id: changes
            .default_platform_id
            .unwrap_or(existing.default_platform_id)
            .map(|id| id.to_string()),
        default_vcpus: changes.default_vcpus.unwrap_or(existing.default_vcpus),
        default_memory: changes.default_memory.unwrap_or(existing.default_memory),
        owner_id: changes.owner_id.unwrap_or(existing.owner_id),
        description: Some(changes.description.clone().unwrap_or(existing.description)),
        comments: changes.comments.clone(),
        custom_fields: changes.custom_fields.clone(),
    };
    let parsed = parse_virtual_machine_type_payload(state, &candidate).await?;
    if changes.name.is_some() {
        changes.name = Some(parsed.name);
    }
    if changes.slug.is_some() {
        changes.slug = Some(parsed.slug);
    }
    let outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE, changes.custom_fields.clone(), false)
            .await?;
    let intents = if let Some(outcome) = outcome {
        changes.custom_fields = Some(outcome.custom_field_data);
        outcome.secret_intents
    } else {
        Vec::new()
    };
    if !state
        .repositories
        .virtual_machine_types
        .update(id, &changes)
        .await?
    {
        return Err(AppError::NotFound("Virtual machine type not found".into()));
    }
    apply_custom_field_secret_intents(state, id, intents).await?;
    let item = state
        .repositories
        .virtual_machine_types
        .get_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("Virtual machine type not found".into()))?;
    mask_type_record(state, item).await
}

/// Delete a virtual machine type.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/virtual-machine-types/{id}",
    operation_id = "delete_inventory_virtual_machine_type",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Virtual machine type ID"),
    ),
    responses(
        (status = 204, description = "Virtual machine type deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Virtual machine type is in use", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_virtual_machine_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let entity_id = match parse_uuid(&id, "virtual_machine_type_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::Inventory,
        Some(&format!("inventory:{entity_id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state
        .repositories
        .virtual_machine_types
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound(
                "Virtual machine type not found".to_string(),
            ));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .virtual_machine_types
        .delete(entity_id)
        .await
    {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_reject_numeric_loss_and_invalid_slugs() {
        for value in [
            0.0,
            -1.0,
            0.015,
            1.2300000001,
            10000.0,
            f64::INFINITY,
            f64::NAN,
        ] {
            assert!(validate_type_scalars("Type", "type", Some(value), None).is_err());
        }
        for value in [0.01, 0.25, 0.29, 1.23, 9999.99] {
            assert!(validate_type_scalars("Type", "type", Some(value), Some(0)).is_ok());
        }
        assert!(validate_type_scalars("Type", "bad slug", None, None).is_err());
        assert!(validate_type_scalars("Type", "type", None, Some(-1)).is_err());
        assert!(
            serde_json::from_value::<VirtualMachineTypePayload>(
                serde_json::json!({"name":"Type","slug":"type","unknown":1})
            )
            .is_err()
        );
    }

    #[test]
    fn patches_distinguish_omitted_nullable_fields_and_reject_null_text()
    -> Result<(), serde_json::Error> {
        let omitted: VirtualMachineTypePatchPayload =
            serde_json::from_value(serde_json::json!({}))?;
        assert_eq!(omitted.default_vcpus, None);
        let clear: VirtualMachineTypePatchPayload =
            serde_json::from_value(serde_json::json!({"default_vcpus":null,"owner_id":null}))?;
        assert_eq!(clear.default_vcpus, Some(None));
        assert_eq!(clear.owner_id, Some(None));
        let value: VirtualMachineTypePatchPayload =
            serde_json::from_value(serde_json::json!({"default_vcpus":0.25}))?;
        assert_eq!(value.default_vcpus, Some(Some(0.25)));
        for field in ["name", "slug", "description", "comments", "custom_fields"] {
            assert!(
                serde_json::from_value::<VirtualMachineTypePatchPayload>(
                    serde_json::json!({field:null})
                )
                .is_err()
            );
        }
        assert!(
            serde_json::from_value::<VirtualMachineTypePatchPayload>(
                serde_json::json!({"unknown":true})
            )
            .is_err()
        );
        Ok(())
    }
}
