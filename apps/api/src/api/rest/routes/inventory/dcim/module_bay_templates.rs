use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use super::super::InventoryListEnvelope;
use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::permission_helpers::{
    ensure_access_with_resource, ensure_inventory_collection_access,
};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::{build_request_context, project_inventory_delete_error};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::dcim::module_bay_template::{
    ModuleBayTemplate, ModuleBayTemplateChanges,
};
use crate::contexts::shared::{
    list_query::{parse_csv_uuids, search_pattern},
    pagination::{clamp_limit, clamp_offset},
};
use crate::errors::AppError;

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in=Query)]
#[serde(deny_unknown_fields)]
pub struct ModuleBayTemplateListQuery {
    pub device_type_id: Option<String>,
    pub module_type_id: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[utoipa::path(get,path="/api/v1/inventory/module-bay-templates",tag="inventory",params(ModuleBayTemplateListQuery),responses((status=200,body=InventoryListEnvelope<ModuleBayTemplate>),(status="4XX",body=crate::api::rest::error_mapping::ApiErrorResponse),(status="5XX",body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_module_bay_templates(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ModuleBayTemplateListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &context, Action::Read).await?;
        let devices = parse_csv_uuids(query.device_type_id.as_deref(), "device_type_id")?;
        let modules = parse_csv_uuids(query.module_type_id.as_deref(), "module_type_id")?;
        let (items, total) = state
            .repositories
            .module_bay_templates
            .list(
                &devices,
                &modules,
                search_pattern(query.search.as_deref()),
                query.limit,
                query.offset,
            )
            .await?;
        Ok::<_, AppError>(InventoryListEnvelope {
            items,
            total,
            limit: clamp_limit(query.limit),
            offset: clamp_offset(query.offset),
        })
    }
    .await;
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(get,path="/api/v1/inventory/module-bay-templates/{id}",tag="inventory",params(("id"=Uuid,Path)),responses((status=200,body=ModuleBayTemplate),(status="4XX",body=crate::api::rest::error_mapping::ApiErrorResponse),(status="5XX",body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_module_bay_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    let result = async {
        ensure_access_with_resource(
            &state,
            &context,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        state
            .repositories
            .module_bay_templates
            .get(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Module bay template not found".into()))
    }
    .await;
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(post,path="/api/v1/inventory/module-bay-templates",tag="inventory",request_body=ModuleBayTemplateChanges,responses((status=201,body=ModuleBayTemplate),(status="4XX",body=crate::api::rest::error_mapping::ApiErrorResponse),(status="5XX",body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_module_bay_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ModuleBayTemplateChanges>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &context, Action::Create).await?;
        state
            .repositories
            .module_bay_templates
            .save(Uuid::now_v7(), payload, true)
            .await?
            .ok_or_else(|| AppError::Internal("Created module bay template not found".into()))
    }
    .await;
    match result {
        Ok(value) => (StatusCode::CREATED, Json(value)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(put,path="/api/v1/inventory/module-bay-templates/{id}",tag="inventory",params(("id"=Uuid,Path)),request_body=ModuleBayTemplateChanges,responses((status=200,body=ModuleBayTemplate),(status="4XX",body=crate::api::rest::error_mapping::ApiErrorResponse),(status="5XX",body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_module_bay_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<ModuleBayTemplateChanges>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let result = async {
        ensure_access_with_resource(
            &state,
            &context,
            Action::Update,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        state
            .repositories
            .module_bay_templates
            .save(id, payload, false)
            .await?
            .ok_or_else(|| AppError::NotFound("Module bay template not found".into()))
    }
    .await;
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(delete,path="/api/v1/inventory/module-bay-templates/{id}",tag="inventory",params(("id"=Uuid,Path)),responses((status=204),(status="4XX",body=crate::api::rest::error_mapping::ApiErrorResponse),(status="5XX",body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_module_bay_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let result = async {
        ensure_access_with_resource(
            &state,
            &context,
            Action::Delete,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        state.repositories.module_bay_templates.delete(id).await
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            handle_app_error(project_inventory_delete_error(&state, &context, error).await)
        }
    }
}
