//! Cable bundle REST handlers. Persistence belongs to the cable bundle repository.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::infrastructure::persistence::{NewCableBundle, UpdateCableBundle, CableBundleRecord, CableBundleRepository};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::cable_bundle_repository::CABLE_BUNDLE_COLUMNS;
use crate::contexts::shared::{list_query::SortSpec, pagination::{clamp_limit, clamp_offset}};
use crate::errors::AppError;
use super::super::{InventoryListEnvelope, parse_required_string, parse_uuid, validate_and_split_custom_fields, masked_custom_fields};
use crate::api::permission_helpers::{ensure_access_with_resource, ensure_inventory_collection_access};

const CONTENT_TYPE: &str = "dcim.cablebundle";

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct CableBundleListQuery {
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CableBundleCreatePayload {
    pub name: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    #[serde(default)]
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CableBundleUpdatePayload {
    pub name: Option<String>,
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

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CableBundleBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<CableBundleCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CableBundleResponse {
    pub id: Uuid,
    pub external_id: Option<i64>,
    pub name: String,
    pub description: String,
    pub comments: String,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: serde_json::Value,
    pub created: Option<chrono::DateTime<chrono::Utc>>,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkCableBundleResponse {
    pub created: Vec<CableBundleResponse>,
    pub errors: Vec<String>,
}

fn name(value: &str) -> Result<String, AppError> {
    let value = parse_required_string(value, "name")?;
    if value.chars().count() > 100 {
        return Err(AppError::Validation(
            "name must be at most 100 characters".into(),
        ));
    }
    Ok(value)
}

fn validate_description(value: Option<&str>) -> Result<(), AppError> {
    if value.is_some_and(|value| value.chars().count() > 200) {
        return Err(AppError::Validation(
            "description must be at most 200 characters".into(),
        ));
    }
    Ok(())
}

async fn response(
    state: &AppState,
    record: CableBundleRecord,
) -> Result<CableBundleResponse, AppError> {
    let custom_fields =
        masked_custom_fields(state, CONTENT_TYPE, record.id, &record.custom_fields).await?;
    Ok(CableBundleResponse {
        id: record.id,
        external_id: record.external_id,
        name: record.name,
        description: record.description,
        comments: record.comments,
        owner_id: record.owner_id,
        tags: record.tags,
        custom_fields,
        created: record.created,
        last_updated: record.last_updated,
        piping_synced_at: record.piping_synced_at,
    })
}

async fn prepare_create(
    state: &AppState,
    payload: CableBundleCreatePayload,
) -> Result<
    (
        NewCableBundle,
        Vec<crate::contexts::custom_fields::SecretWriteIntent>,
    ),
    AppError,
> {
    let name = name(&payload.name)?;
    validate_description(payload.description.as_deref())?;
    let outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE, payload.custom_fields, true).await?;
    let (custom_fields, intents) = match outcome {
        Some(outcome) => (Some(outcome.custom_field_data), outcome.secret_intents),
        None => (None, Vec::new()),
    };
    Ok((
        NewCableBundle {
            id: Uuid::now_v7(),
            name,
            description: payload.description,
            comments: payload.comments,
            owner_id: payload.owner_id,
            tags: payload.tags,
            custom_fields,
        },
        intents,
    ))
}

#[utoipa::path(get, path = "/api/v1/inventory/cable-bundles", tag = "inventory", params(CableBundleListQuery), responses((status = 200, description = "Successful response", body = InventoryListEnvelope<CableBundleResponse>), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_cable_bundles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<CableBundleListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Read).await {
        return handle_app_error(error);
    }
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CABLE_BUNDLE_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .cable_bundles
        .list(
            query.limit,
            query.offset,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
        )
        .await
    {
        Ok((records, total)) => {
            let mut items = Vec::with_capacity(records.len());
            for record in records {
                match response(&state, record).await {
                    Ok(item) => items.push(item),
                    Err(error) => return handle_app_error(error),
                }
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

#[utoipa::path(get, path = "/api/v1/inventory/cable-bundles/{id}", tag = "inventory", params(("id" = String, Path)), responses((status = 200, description = "Successful response", body = CableBundleResponse), (status = 404, description = "Resource not found", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_cable_bundle(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, auth_context, &headers);
    let id = match parse_uuid(&id, "id") {
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
    match state.repositories.cable_bundles.get_by_id(id).await {
        Ok(Some(record)) => match response(&state, record).await {
            Ok(item) => (StatusCode::OK, Json(item)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Cable bundle not found".into())),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(post, path = "/api/v1/inventory/cable-bundles", tag = "inventory", request_body = CableBundleCreatePayload, responses((status = 201, description = "Resource created", body = CableBundleResponse), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_cable_bundle(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CableBundleCreatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    let (item, intents) = match prepare_create(&state, payload).await {
        Ok(item) => item,
        Err(error) => return handle_app_error(error),
    };
    let inputs = match state
        .services
        .custom_field_values
        .prepare_secret_intents(item.id, intents)
    {
        Ok(inputs) => inputs,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .cable_bundles
        .create_with_secret_intents(&item, inputs)
        .await
    {
        Ok(record) => match response(&state, record).await {
            Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(put, path = "/api/v1/inventory/cable-bundles/{id}", tag = "inventory", params(("id" = String, Path)), request_body = CableBundleUpdatePayload, responses((status = 200, description = "Successful response", body = CableBundleResponse), (status = 404, description = "Resource not found", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn update_cable_bundle(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<CableBundleUpdatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Update,
        ResourceType::Inventory,
        Some(&format!("inventory:{id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    let name = match payload.name.as_deref().map(name).transpose() {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = validate_description(payload.description.as_deref()) {
        return handle_app_error(error);
    }
    let outcome =
        match validate_and_split_custom_fields(&state, CONTENT_TYPE, payload.custom_fields, false)
            .await
        {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let (custom_fields, intents) = match outcome {
        Some(outcome) => (Some(outcome.custom_field_data), outcome.secret_intents),
        None => (None, Vec::new()),
    };
    let changes = UpdateCableBundle {
        name,
        description: payload.description,
        comments: payload.comments,
        owner_id: payload.owner_id,
        tags: payload.tags,
        custom_fields,
    };
    let inputs = match state
        .services
        .custom_field_values
        .prepare_secret_intents(id, intents)
    {
        Ok(inputs) => inputs,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .cable_bundles
        .update_with_secret_intents(id, &changes, inputs)
        .await
    {
        Ok(Some(record)) => match response(&state, record).await {
            Ok(item) => (StatusCode::OK, Json(item)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound("Cable bundle not found".into())),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(delete, path = "/api/v1/inventory/cable-bundles/{id}", tag = "inventory", params(("id" = String, Path)), responses((status = 204, description = "Cable bundle deleted"), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_cable_bundle(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    let id = match parse_uuid(&id, "id") {
        Ok(id) => id,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_access_with_resource(
        &state,
        &context,
        Action::Delete,
        ResourceType::Inventory,
        Some(&format!("inventory:{id}")),
    )
    .await
    {
        return handle_app_error(error);
    }
    match state.repositories.cable_bundles.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(post, path = "/api/v1/inventory/cable-bundles/bulk", tag = "inventory", request_body = CableBundleBulkCreatePayload, responses((status = 200, description = "Successful response", body = BulkCableBundleResponse), (status = "4XX", description = "Client error", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", description = "Server error", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn bulk_create_cable_bundles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CableBundleBulkCreatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_inventory_collection_access(&state, &context, Action::Create).await {
        return handle_app_error(error);
    }
    let mut items = Vec::with_capacity(payload.items.len());
    let mut intents = std::collections::HashMap::new();
    for payload in payload.items {
        match prepare_create(&state, payload).await {
            Ok((item, item_intents)) => {
                let inputs = match state
                    .services
                    .custom_field_values
                    .prepare_secret_intents(item.id, item_intents)
                {
                    Ok(inputs) => inputs,
                    Err(error) => return handle_app_error(error),
                };
                intents.insert(item.id, inputs);
                items.push(item);
            }
            Err(error) => return handle_app_error(error),
        }
    }
    match state
        .repositories
        .cable_bundles
        .bulk_create_with_secret_intents(&items, intents)
        .await
    {
        Ok(result) => {
            let mut created = Vec::with_capacity(result.created.len());
            for record in result.created {
                match response(&state, record).await {
                    Ok(item) => created.push(item),
                    Err(error) => return handle_app_error(error),
                }
            }
            (
                StatusCode::OK,
                Json(BulkCableBundleResponse {
                    created,
                    errors: result.errors,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
