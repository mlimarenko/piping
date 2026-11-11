use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::permission_helpers::{ensure_access_with_resource, ensure_inventory_collection_access};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::dcim::module_type_profile::ModuleTypeProfile;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::module_type_profile_repository::MODULE_TYPE_PROFILE_COLUMNS;
use crate::contexts::shared::list_query::{SortSpec, search_pattern};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, parse_uuid, masked_custom_fields, masked_custom_fields_page, validate_and_split_custom_fields};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::module_type_profile_repository::ModuleTypeProfileChanges;
use crate::contexts::inventory::infrastructure::persistence::OwnerRepository;
use crate::contexts::shared::request_context::RequestContext;
use uuid::Uuid;

const CONTENT_TYPE: &str = "dcim.moduletypeprofile";

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleTypeProfileCreatePayload {
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    #[schema(min_length = 1, max_length = 100)]
    pub name: String,
    #[serde(default)]
    #[schema(max_length = 200)]
    pub description: String,
    #[serde(default)]
    pub comments: String,
    pub schema: Option<serde_json::Value>,
    pub owner_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleTypeProfilePatchPayload {
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, min_length = 1, max_length = 100)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, max_length = 200)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub comments: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub schema: Option<Option<serde_json::Value>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub owner_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub custom_fields: Option<serde_json::Map<String, serde_json::Value>>,
}

fn validate_profile(changes: &ModuleTypeProfileChanges) -> Result<(), AppError> {
    if let Some(name) = &changes.name {
        if name.trim().is_empty() || name.chars().count() > 100 {
            return Err(AppError::Validation(
                "name must contain 1 to 100 characters".into(),
            ));
        }
    }
    if changes
        .description
        .as_ref()
        .is_some_and(|v| v.chars().count() > 200)
    {
        return Err(AppError::Validation(
            "description must be at most 200 characters".into(),
        ));
    }
    if let Some(Some(schema)) = &changes.schema {
        crate::contexts::inventory::domain::dcim::module_type_profile::profile_validator(schema)?;
    }
    Ok(())
}

async fn authorize_owner(
    state: &AppState,
    ctx: &RequestContext,
    id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(id) = id {
        ensure_access_with_resource(
            state,
            ctx,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        if state.repositories.owners.get_by_id(id).await?.is_none() {
            return Err(AppError::Validation("owner_id does not exist".into()));
        }
    }
    Ok(())
}

async fn mask_profile(
    state: &AppState,
    mut item: ModuleTypeProfile,
) -> Result<ModuleTypeProfile, AppError> {
    item.custom_fields =
        masked_custom_fields(state, CONTENT_TYPE, item.id, &item.custom_fields).await?;
    Ok(item)
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct ModuleTypeProfileListQuery {
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub owner_id: Option<String>,
}

/// List module type profiles.
#[utoipa::path(
    get, path = "/api/v1/inventory/module-type-profiles", tag = "inventory",
    params(ModuleTypeProfileListQuery),
    responses(
        (status = 200, description = "Module type profiles", body = InventoryListEnvelope<ModuleTypeProfile>),
        (status = "4XX", description = "Request rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_module_type_profiles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ModuleTypeProfileListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Read).await {
        return handle_app_error(error);
    }
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        MODULE_TYPE_PROFILE_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(error) => return handle_app_error(error),
    };
    let owner_ids = match crate::contexts::shared::list_query::parse_csv_uuids(
        query.owner_id.as_deref(),
        "owner_id",
    ) {
        Ok(ids) => ids,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .module_type_profiles
        .list_paginated(
            search_pattern(query.search.as_deref()),
            query.limit,
            query.offset,
            sort,
            &owner_ids,
        )
        .await
    {
        Ok((mut items, total)) => {
            let inputs = items
                .iter()
                .map(|item| (item.id, item.custom_fields.clone()))
                .collect();
            let masked = match masked_custom_fields_page(&state, CONTENT_TYPE, inputs).await {
                Ok(values) => values,
                Err(error) => return handle_app_error(error),
            };
            for (item, fields) in items.iter_mut().zip(masked) {
                item.custom_fields = fields;
            }
            (
                StatusCode::OK,
                Json(InventoryListEnvelope {
                    items,
                    total,
                    limit: clamp_limit(query.limit),
                    offset: clamp_offset(query.offset),
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a module type profile.
#[utoipa::path(
    get, path = "/api/v1/inventory/module-type-profiles/{id}", tag = "inventory",
    params(("id" = String, Path, description = "Module type profile id")),
    responses(
        (status = 200, description = "Module type profile", body = ModuleTypeProfile),
        (status = 404, description = "Profile not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Request rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_module_type_profile(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    let id = match parse_uuid(&id, "module_type_profile_id") {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Read,
        ResourceType::Inventory,
        Some(&format!("inventory:{id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.repositories.module_type_profiles.get_by_id(id).await {
        Ok(Some(profile)) => match mask_profile(&state, profile).await {
            Ok(item) => Json(item).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Module type profile not found".into())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a module type attribute profile.
#[utoipa::path(post, path="/api/v1/inventory/module-type-profiles", tag="inventory", request_body=ModuleTypeProfileCreatePayload, responses((status=201, description="Created", body=ModuleTypeProfile), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_module_type_profile(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ModuleTypeProfileCreatePayload>,
) -> impl IntoResponse {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &ctx, Action::Create).await?;
        let changes = ModuleTypeProfileChanges {
            name: Some(payload.name.clone()),
            description: Some(payload.description.clone()),
            schema: Some(payload.schema.clone()),
            ..Default::default()
        };
        validate_profile(&changes)?;
        authorize_owner(&state, &ctx, payload.owner_id).await?;
        let outcome = validate_and_split_custom_fields(
            &state,
            CONTENT_TYPE,
            payload.custom_fields.map(serde_json::Value::Object),
            true,
        )
        .await?;
        let mut item = ModuleTypeProfile {
            id: Uuid::now_v7(),
            name: payload.name,
            description: payload.description,
            comments: payload.comments,
            schema: payload.schema,
            owner_id: payload.owner_id,
            custom_fields: serde_json::json!({}),
            tags: payload.tags.unwrap_or_default(),
            created: None,
            last_updated: None,
        };
        let intents = if let Some(outcome) = outcome {
            item.custom_fields = outcome.custom_field_data;
            outcome.secret_intents
        } else {
            Vec::new()
        };
        let inputs = state
            .services
            .custom_field_values
            .prepare_secret_intents(item.id, intents)?;
        let item = state
            .repositories
            .module_type_profiles
            .create_with_secret_intents(&item, inputs)
            .await?;
        mask_profile(&state, item).await
    }
    .await;
    match result {
        Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Partially update a module type attribute profile; null clears nullable fields.
#[utoipa::path(patch, path="/api/v1/inventory/module-type-profiles/{id}", tag="inventory", params(("id"=Uuid, Path)), request_body=ModuleTypeProfilePatchPayload, responses((status=200, description="Updated", body=ModuleTypeProfile), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn patch_module_type_profile(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ModuleTypeProfilePatchPayload>,
) -> impl IntoResponse {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "module_type_profile_id")?;
        ensure_access_with_resource(
            &state,
            &ctx,
            Action::Update,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        let mut changes = ModuleTypeProfileChanges {
            name: payload.name,
            description: payload.description,
            comments: payload.comments,
            schema: payload.schema,
            owner_id: payload.owner_id,
            custom_fields: payload.custom_fields.map(serde_json::Value::Object),
            tags: payload.tags,
        };
        validate_profile(&changes)?;
        authorize_owner(&state, &ctx, changes.owner_id.flatten()).await?;
        let outcome = validate_and_split_custom_fields(
            &state,
            CONTENT_TYPE,
            changes.custom_fields.take(),
            false,
        )
        .await?;
        let intents = if let Some(outcome) = outcome {
            changes.custom_fields = Some(outcome.custom_field_data);
            outcome.secret_intents
        } else {
            Vec::new()
        };
        let inputs = state
            .services
            .custom_field_values
            .prepare_secret_intents(id, intents)?;
        let item = state
            .repositories
            .module_type_profiles
            .update_with_secret_intents(id, &changes, inputs)
            .await?
            .ok_or_else(|| AppError::NotFound("Module type profile not found".into()))?;
        mask_profile(&state, item).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an unreferenced module type attribute profile.
#[utoipa::path(delete, path="/api/v1/inventory/module-type-profiles/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=204, description="Deleted"), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_module_type_profile(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "module_type_profile_id")?;
        ensure_access_with_resource(
            &state,
            &ctx,
            Action::Delete,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        state.repositories.module_type_profiles.delete(id).await
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(
            crate::api::rest::request_context::project_inventory_delete_error(&state, &ctx, error)
                .await,
        ),
    }
}
