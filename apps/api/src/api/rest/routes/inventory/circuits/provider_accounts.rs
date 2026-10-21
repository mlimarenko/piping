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
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::ProviderAccount;
use crate::contexts::inventory::infrastructure::persistence::{
    PROVIDER_ACCOUNT_COLUMNS, ProviderAccountListItem, ProviderAccountRepository, SortSpec,
    parse_csv_uuids,
};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope,
    bump_host_config_cache_generation, ensure_access, ensure_create_inventory_access,
    parse_required_string, parse_uuid, push_bulk_error, require_bulk_items,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ProviderAccountListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub provider_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct ProviderAccountPayload {
    pub account: String,
    pub name: Option<String>,
    pub provider_id: String,
    pub description: Option<String>,
    pub comments: Option<String>,
}

/// Hydrated provider account list item — includes the provider name for the
/// collection list endpoint.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ProviderAccountListItemResponse {
    pub id: Uuid,
    pub account: String,
    pub name: String,
    pub provider_id: Uuid,
    pub provider_name: Option<String>,
    pub description: String,
    pub comments: String,
}

fn map_provider_account_list_item(
    item: ProviderAccountListItem,
) -> ProviderAccountListItemResponse {
    let ProviderAccountListItem {
        record,
        provider_name,
    } = item;
    ProviderAccountListItemResponse {
        id: record.id,
        account: record.account,
        name: record.name,
        provider_id: record.provider_id,
        provider_name,
        description: record.description,
        comments: record.comments,
    }
}

struct ParsedProviderAccount {
    account: String,
    provider_id: Uuid,
}

fn parse_provider_account_payload(
    payload: &ProviderAccountPayload,
) -> Result<ParsedProviderAccount, AppError> {
    Ok(ParsedProviderAccount {
        account: parse_required_string(&payload.account, "account")?,
        provider_id: parse_uuid(&payload.provider_id, "provider_id")?,
    })
}

fn build_provider_account(
    id: Uuid,
    parsed: ParsedProviderAccount,
    payload: ProviderAccountPayload,
) -> ProviderAccount {
    ProviderAccount {
        id,
        account: parsed.account,
        name: payload.name.unwrap_or_default(),
        provider_id: parsed.provider_id,
        description: payload.description.unwrap_or_default(),
        comments: payload.comments.unwrap_or_default(),
        ..Default::default()
    }
}

/// List provider accounts.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/provider-accounts",
    operation_id = "list_inventory_provider_accounts",
    tag = "inventory",
    params(ProviderAccountListQuery),
    responses(
        (status = 200, description = "Provider accounts", body = InventoryListEnvelope<ProviderAccountListItemResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_provider_accounts(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ProviderAccountListQuery>,
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
    let provider_id = match parse_csv_uuids(query.provider_id.as_deref(), "provider_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        PROVIDER_ACCOUNT_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    match state
        .repositories
        .provider_accounts
        .list_paginated_hydrated(
            provider_id,
            crate::contexts::shared::list_query::search_pattern(query.search.as_deref()),
            sort,
            query.limit,
            query.offset,
        )
        .await
    {
        Ok((records, total)) => {
            let items: Vec<ProviderAccountListItemResponse> = records
                .into_iter()
                .map(map_provider_account_list_item)
                .collect();
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

/// Get a provider account.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/provider-accounts/{id}",
    operation_id = "get_inventory_provider_account",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Provider account ID"),
    ),
    responses(
        (status = 200, description = "Provider account", body = ProviderAccount),
        (status = 404, description = "Provider account not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_provider_account(
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
    let entity_id = match parse_uuid(&id, "provider_account_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .provider_accounts
        .get_by_id(entity_id)
        .await
    {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Provider account not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a provider account.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/provider-accounts",
    operation_id = "create_inventory_provider_account",
    tag = "inventory",
    request_body = ProviderAccountPayload,
    responses(
        (status = 201, description = "Provider account created", body = ProviderAccount),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_provider_account(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ProviderAccountPayload>,
) -> impl IntoResponse {
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
    let parsed = match parse_provider_account_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let item = build_provider_account(Uuid::now_v7(), parsed, payload);
    match state.repositories.provider_accounts.create(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create provider accounts.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/provider-accounts/bulk",
    operation_id = "bulk_create_inventory_provider_accounts",
    tag = "inventory",
    request_body = BulkCreateRequest<ProviderAccountPayload>,
    responses(
        (status = 200, description = "Bulk provider account create result", body = BulkCreateResponse<ProviderAccount>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_provider_accounts(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<ProviderAccountPayload>>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_create_inventory_access(&state, &request_context).await {
        return handle_app_error(error);
    }

    let items = match require_bulk_items(request.items) {
        Ok(items) => items,
        Err(error) => return handle_app_error(error),
    };

    let mut created = Vec::new();
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        let parsed = match parse_provider_account_payload(&payload) {
            Ok(value) => value,
            Err(error) => {
                push_bulk_error(&mut errors, index, error);
                continue;
            }
        };
        let item = build_provider_account(Uuid::now_v7(), parsed, payload);

        match state.repositories.provider_accounts.create(&item).await {
            Ok(()) => created.push(item),
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    if !created.is_empty() {
        bump_host_config_cache_generation(&state).await;
    }

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a provider account.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/provider-accounts/{id}",
    operation_id = "update_inventory_provider_account",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Provider account ID"),
    ),
    request_body = ProviderAccountPayload,
    responses(
        (status = 200, description = "Provider account updated", body = ProviderAccount),
        (status = 404, description = "Provider account not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_provider_account(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ProviderAccountPayload>,
) -> impl IntoResponse {
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
    let entity_id = match parse_uuid(&id, "provider_account_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let parsed = match parse_provider_account_payload(&payload) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .provider_accounts
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Provider account not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    let item = build_provider_account(entity_id, parsed, payload);
    match state.repositories.provider_accounts.update(&item).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(item)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a provider account.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/provider-accounts/{id}",
    operation_id = "delete_inventory_provider_account",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Provider account ID"),
    ),
    responses(
        (status = 204, description = "Provider account deleted"),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_provider_account(
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
    let entity_id = match parse_uuid(&id, "provider_account_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    match state
        .repositories
        .provider_accounts
        .get_by_id(entity_id)
        .await
    {
        Ok(None) => {
            return handle_app_error(AppError::NotFound("Provider account not found".to_string()));
        }
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.provider_accounts.delete(entity_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
