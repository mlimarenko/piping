use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::permission_helpers::{ensure_access_with_resource, ensure_inventory_collection_access};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::{build_request_context, project_inventory_delete_error};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::dcim::module_bay_type::ModuleBayType;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::module_bay_type_repository::{MODULE_BAY_TYPE_COLUMNS, NewModuleBayType, UpdateModuleBayType};
use crate::contexts::inventory::infrastructure::persistence::repositories::derive_slug;
use crate::contexts::shared::{list_query::{SortSpec, parse_csv_uuids, search_pattern}, pagination::{clamp_limit, clamp_offset}};
use crate::errors::AppError;
use super::super::{InventoryListEnvelope, BulkCreateResponse, parse_uuid, parse_required_string, masked_custom_fields, validate_and_split_custom_fields, push_bulk_error};

const CONTENT_TYPE: &str = "dcim.modulebaytype";

impl super::super::InventoryCustomFieldProjection for ModuleBayType {
    const CONTENT_TYPE: &'static str = CONTENT_TYPE;
    fn inventory_id(&self) -> Uuid {
        self.id
    }
    fn assign_custom_fields(&mut self, value: serde_json::Value) {
        self.custom_fields = value;
    }
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ModuleBayTypeListQuery {
    pub manufacturer_id: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleBayTypeCreatePayload {
    pub name: String,
    pub slug: Option<String>,
    pub manufacturer_id: Option<Uuid>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    #[serde(default)]
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleBayTypeUpdatePayload {
    pub name: Option<String>,
    pub slug: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub manufacturer_id: Option<Option<Uuid>>,
    pub color: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub owner_id: Option<Option<Uuid>>,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleBayTypeBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<ModuleBayTypeCreatePayload>,
}

async fn masked(state: &AppState, mut record: ModuleBayType) -> Result<ModuleBayType, AppError> {
    record.custom_fields =
        masked_custom_fields(state, CONTENT_TYPE, record.id, &record.custom_fields).await?;
    Ok(record)
}

async fn prepare_create(
    state: &AppState,
    payload: ModuleBayTypeCreatePayload,
) -> Result<
    (
        NewModuleBayType,
        Vec<crate::contexts::secrets::application::ApplySecretFieldIntentInput>,
    ),
    AppError,
> {
    let name = parse_required_string(&payload.name, "name")?;
    let outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE, payload.custom_fields, true).await?;
    let id = Uuid::now_v7();
    let custom_fields = outcome.as_ref().map(|v| v.custom_field_data.clone());
    let inputs = state
        .services
        .custom_field_values
        .prepare_secret_intents(id, outcome.map(|v| v.secret_intents).unwrap_or_default())?;
    let item = NewModuleBayType {
        id,
        slug: payload.slug.unwrap_or_else(|| derive_slug(&name)),
        name,
        manufacturer_id: payload.manufacturer_id,
        color: payload.color.unwrap_or_else(|| "9e9e9e".into()),
        description: payload.description,
        comments: payload.comments,
        owner_id: payload.owner_id,
        tags: payload.tags,
        custom_fields,
    };
    Ok((item, inputs))
}

#[utoipa::path(get, path = "/api/v1/inventory/module-bay-types", tag = "inventory", params(ModuleBayTypeListQuery), responses((status = 200, description = "Successful response", body = InventoryListEnvelope<ModuleBayType>), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_module_bay_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ModuleBayTypeListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_inventory_collection_access(&state, &context, Action::Read).await {
        return handle_app_error(e);
    }
    let result = async {
        let manufacturers = parse_csv_uuids(query.manufacturer_id.as_deref(), "manufacturer_id")?;
        let sort = SortSpec::parse_columns(
            query.sort.as_deref(),
            query.order.as_deref(),
            MODULE_BAY_TYPE_COLUMNS,
        )?;
        let (mut items, total) = state
            .repositories
            .module_bay_types
            .list(
                &manufacturers,
                search_pattern(query.search.as_deref()),
                query.limit,
                query.offset,
                sort,
            )
            .await?;
        super::super::mask_inventory_projection_page(&state, &mut items).await?;
        Ok::<_, AppError>(InventoryListEnvelope {
            items,
            total,
            limit: clamp_limit(query.limit),
            offset: clamp_offset(query.offset),
        })
    }
    .await;
    match result {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => handle_app_error(e),
    }
}

#[utoipa::path(get, path = "/api/v1/inventory/module-bay-types/{id}", tag = "inventory", params(("id" = String, Path)), responses((status = 200, description = "Successful response", body = ModuleBayType), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_module_bay_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    let result = async {
        let id = parse_uuid(&id, "id")?;
        ensure_access_with_resource(
            &state,
            &context,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        let record = state
            .repositories
            .module_bay_types
            .get_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Module bay type not found".into()))?;
        masked(&state, record).await
    }
    .await;
    match result {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => handle_app_error(e),
    }
}

#[utoipa::path(post, path = "/api/v1/inventory/module-bay-types", tag = "inventory", request_body = ModuleBayTypeCreatePayload, responses((status = 201, description = "Resource created", body = ModuleBayType), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_module_bay_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ModuleBayTypeCreatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(e) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(e);
    }
    let result = async {
        let (item, inputs) = prepare_create(&state, payload).await?;
        masked(
            &state,
            state
                .repositories
                .module_bay_types
                .create(&item, inputs)
                .await?,
        )
        .await
    }
    .await;
    match result {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => handle_app_error(e),
    }
}

#[utoipa::path(put, path = "/api/v1/inventory/module-bay-types/{id}", tag = "inventory", params(("id" = String, Path)), request_body = ModuleBayTypeUpdatePayload, responses((status = 200, description = "Successful response", body = ModuleBayType), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_module_bay_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ModuleBayTypeUpdatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let result = async {
        let id = parse_uuid(&id, "id")?;
        ensure_access_with_resource(
            &state,
            &context,
            Action::Update,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        let outcome =
            validate_and_split_custom_fields(&state, CONTENT_TYPE, payload.custom_fields, false)
                .await?;
        let custom_fields = outcome.as_ref().map(|v| v.custom_field_data.clone());
        let inputs = state
            .services
            .custom_field_values
            .prepare_secret_intents(id, outcome.map(|v| v.secret_intents).unwrap_or_default())?;
        let changes = UpdateModuleBayType {
            name: payload.name,
            slug: payload.slug,
            manufacturer_id: payload.manufacturer_id,
            color: payload.color,
            description: payload.description,
            comments: payload.comments,
            owner_id: payload.owner_id,
            tags: payload.tags,
            custom_fields,
        };
        let record = state
            .repositories
            .module_bay_types
            .update(id, &changes, inputs)
            .await?
            .ok_or_else(|| AppError::NotFound("Module bay type not found".into()))?;
        masked(&state, record).await
    }
    .await;
    match result {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => handle_app_error(e),
    }
}

#[utoipa::path(post, path = "/api/v1/inventory/module-bay-types/bulk", tag = "inventory", request_body = ModuleBayTypeBulkCreatePayload, responses((status = 200, description = "Successful response", body = BulkCreateResponse<ModuleBayType>), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn bulk_create_module_bay_types(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ModuleBayTypeBulkCreatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(e) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(e);
    }
    let mut created = Vec::new();
    let mut errors = Vec::new();
    let mut prepared = Vec::new();
    let mut indices = Vec::new();
    for (index, payload) in payload.items.into_iter().enumerate() {
        match prepare_create(&state, payload).await {
            Ok(v) => {
                prepared.push(v);
                indices.push(index);
            }
            Err(e) => push_bulk_error(&mut errors, index, e),
        }
    }
    let results = match state
        .repositories
        .module_bay_types
        .bulk_create(prepared)
        .await
    {
        Ok(results) => results,
        Err(error) => return handle_app_error(error),
    };
    for (index, result) in indices.into_iter().zip(results) {
        match result {
            Ok(v) => created.push(v),
            Err(e) => push_bulk_error(&mut errors, index, e),
        }
    }
    if let Err(error) = super::super::mask_inventory_projection_page(&state, &mut created).await {
        return handle_app_error(error);
    }
    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

#[utoipa::path(delete, path = "/api/v1/inventory/module-bay-types/{id}", tag = "inventory", params(("id" = String, Path)), responses((status = 204, description = "Resource deleted"), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_module_bay_type(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let result = async {
        let id = parse_uuid(&id, "id")?;
        ensure_access_with_resource(
            &state,
            &context,
            Action::Delete,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        state.repositories.module_bay_types.delete(id).await
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            handle_app_error(project_inventory_delete_error(&state, &context, error).await)
        }
    }
}
