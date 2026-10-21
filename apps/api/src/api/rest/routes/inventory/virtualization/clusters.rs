use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::custom_fields::CONTENT_TYPE_CLUSTER;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::Cluster;
use crate::contexts::inventory::infrastructure::persistence::{
    CLUSTER_COLUMNS, ClusterFilter, SortSpec, parse_csv_strings, parse_csv_uuids, parse_id_filter,
};
use crate::errors::AppError;

use super::super::{
    BulkCreateRequest, BulkCreateResponse, BulkCreatedBatch, ClusterListRowResponse,
    ClusterPayload, InventoryListEnvelope, apply_custom_field_secret_intents,
    apply_custom_fields_write, build_custom_field_list_filter, bump_host_config_cache_generation,
    ensure_access, ensure_create_inventory_access, finalize_bulk_created, masked_custom_fields,
    masked_custom_fields_page, parse_optional_uuid, parse_required_string, parse_uuid,
    push_bulk_error, require_bulk_items, validate_and_split_custom_fields,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};

/// Query parameters for cluster listing — pagination, free-text search,
/// optional FK/status filters, and allowlisted sorting.
///
/// `sort` is a stable logical column key (e.g. `name`, `cluster_type`,
/// `cluster_group`, `site`, `tenant`); `order` is `asc` (default) or `desc`. An
/// unknown sort key or invalid order is rejected with 400.
#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ClusterListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Comma-separated resource UUIDs (maximum 100). An empty value matches no rows.
    pub ids: Option<String>,
    /// Case-insensitive substring search on cluster name.
    pub search: Option<String>,
    /// Comma-separated cluster type ids; matches any (`IN`).
    pub cluster_type_id: Option<String>,
    /// Comma-separated cluster group ids; matches any (`IN`).
    pub cluster_group_id: Option<String>,
    /// Comma-separated site ids; matches any (`IN`).
    pub site_id: Option<String>,
    /// Comma-separated tenant ids; matches any (`IN`).
    pub tenant_id: Option<String>,
    /// Comma-separated cluster statuses; matches any (`IN`).
    pub status: Option<String>,
    /// Logical sort column key (allowlisted; unknown keys are rejected).
    pub sort: Option<String>,
    /// Sort direction: `asc` (default) or `desc`.
    pub order: Option<String>,
    #[serde(flatten)]
    #[param(ignore)]
    pub custom_field_filters: BTreeMap<String, String>,
}

/// List clusters.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/clusters",
    tag = "inventory",
    params(ClusterListQuery),
    responses(
        (status = 200, description = "Clusters", body = InventoryListEnvelope<ClusterListRowResponse>),
        (status = "4XX", description = "Cluster list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Cluster list failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_clusters(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ClusterListQuery>,
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
    let ids = match parse_id_filter(query.ids.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CLUSTER_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let cluster_type_id = match parse_csv_uuids(query.cluster_type_id.as_deref(), "cluster_type_id")
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let cluster_group_id =
        match parse_csv_uuids(query.cluster_group_id.as_deref(), "cluster_group_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let site_id = match parse_csv_uuids(query.site_id.as_deref(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let status = parse_csv_strings(query.status.as_deref());
    let custom_fields = match build_custom_field_list_filter(
        &state,
        CONTENT_TYPE_CLUSTER,
        &query.custom_field_filters,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{}%", s));
    let service = state.services.inventory_admin.clone();
    match service
        .list_clusters_hydrated(
            crate::contexts::inventory::application::InventoryClusterListQuery::new(
                ClusterFilter {
                    ids,
                    cluster_type_id,
                    cluster_group_id,
                    site_id,
                    tenant_id,
                    status,
                    custom_fields,
                    sort,
                },
                query.limit,
                query.offset,
            )
            .with_search(search),
        )
        .await
    {
        Ok((rows, total)) => {
            // One masking pass for the whole page (3 queries), never per row.
            let cf_entities: Vec<(Uuid, serde_json::Value)> = rows
                .iter()
                .map(|row| {
                    (
                        row.record.id,
                        row.record
                            .custom_fields
                            .clone()
                            .unwrap_or(serde_json::Value::Null),
                    )
                })
                .collect();
            let masked_page =
                match masked_custom_fields_page(&state, CONTENT_TYPE_CLUSTER, cf_entities).await {
                    Ok(value) => value,
                    Err(error) => return handle_app_error(error),
                };
            let mut items = Vec::with_capacity(rows.len());
            for (row, custom_fields) in rows.into_iter().zip(masked_page) {
                let custom_fields = Some(custom_fields);
                items.push(ClusterListRowResponse {
                    id: row.record.id,
                    external_id: row.record.external_id,
                    name: row.record.name,
                    site_id: row.record.site_id,
                    site_name: row.site_name,
                    tenant_id: row.record.tenant_id,
                    tenant_name: row.tenant_name,
                    cluster_type_id: row.record.cluster_type_id,
                    cluster_type_name: row.cluster_type_name,
                    cluster_group_id: row.record.cluster_group_id,
                    cluster_group_name: row.cluster_group_name,
                    status: row.record.status,
                    description: row.record.description,
                    comments: row.record.comments,
                    custom_fields,
                });
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

/// Get a cluster.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/clusters/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Cluster id"),
    ),
    responses(
        (status = 200, description = "Cluster", body = Cluster),
        (status = 404, description = "Cluster not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Cluster get request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Cluster get failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_cluster(
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
    let cluster_id = match parse_uuid(&id, "cluster_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_cluster(cluster_id).await {
        Ok(Some(mut cluster)) => {
            let raw_custom_fields = cluster
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_CLUSTER, cluster.id, &raw_custom_fields)
                .await
            {
                Ok(value) => cluster.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            (StatusCode::OK, Json(cluster)).into_response()
        }
        Ok(None) => handle_app_error(AppError::NotFound("Cluster not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a cluster.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/clusters",
    tag = "inventory",
    request_body = ClusterPayload,
    responses(
        (status = 201, description = "Cluster created", body = Cluster),
        (status = "4XX", description = "Cluster create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Cluster create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_cluster(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ClusterPayload>,
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
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let site_id = match parse_optional_uuid(payload.site_id.clone(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let cluster_type_id =
        match parse_optional_uuid(payload.cluster_type_id.clone(), "cluster_type_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let cluster_group_id =
        match parse_optional_uuid(payload.cluster_group_id.clone(), "cluster_group_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let cluster_id = Uuid::now_v7();
    let custom_field_outcome = match validate_and_split_custom_fields(
        &state,
        CONTENT_TYPE_CLUSTER,
        payload.custom_fields,
        true,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let mut cluster = Cluster {
        id: cluster_id,
        external_id: None,
        name,
        site_id,
        tenant_id,
        cluster_type_id,
        cluster_group_id,
        status: payload.status,
        description: payload.description,
        comments: payload.comments,
        custom_fields: custom_field_data,
        piping_synced_at: None,
        ..Default::default()
    };
    let service = state.services.inventory_admin.clone();
    match service.create_cluster(cluster.clone()).await {
        Ok(()) => {
            if let Some(outcome) = custom_field_outcome
                && let Err(error) =
                    apply_custom_field_secret_intents(&state, cluster.id, outcome.secret_intents)
                        .await
            {
                return handle_app_error(error);
            }
            let raw_custom_fields = cluster
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_CLUSTER, cluster.id, &raw_custom_fields)
                .await
            {
                Ok(value) => cluster.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            bump_host_config_cache_generation(&state).await;
            (StatusCode::CREATED, Json(cluster)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Validate, persist, and secret-bind one bulk cluster payload. Any failure
/// (parse, custom-field validation, create, or secret intent) fails just this
/// item.
async fn create_cluster_item(
    state: &AppState,
    payload: ClusterPayload,
) -> Result<Cluster, AppError> {
    let name = parse_required_string(&payload.name, "name")?;
    let site_id = parse_optional_uuid(payload.site_id.clone(), "site_id")?;
    let tenant_id = parse_optional_uuid(payload.tenant_id.clone(), "tenant_id")?;
    let cluster_type_id = parse_optional_uuid(payload.cluster_type_id.clone(), "cluster_type_id")?;
    let cluster_group_id =
        parse_optional_uuid(payload.cluster_group_id.clone(), "cluster_group_id")?;
    let custom_field_outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE_CLUSTER, payload.custom_fields, true)
            .await?;
    let custom_field_data = custom_field_outcome
        .as_ref()
        .map(|outcome| outcome.custom_field_data.clone());
    let cluster = Cluster {
        id: Uuid::now_v7(),
        external_id: None,
        name,
        site_id,
        tenant_id,
        cluster_type_id,
        cluster_group_id,
        status: payload.status,
        description: payload.description,
        comments: payload.comments,
        custom_fields: custom_field_data,
        piping_synced_at: None,
        ..Default::default()
    };
    state
        .services
        .inventory_admin
        .create_cluster(cluster.clone())
        .await?;
    if let Some(outcome) = custom_field_outcome
        && let Err(error) =
            apply_custom_field_secret_intents(state, cluster.id, outcome.secret_intents).await
    {
        return Err(error);
    }
    Ok(cluster)
}

/// Bulk-create clusters.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/clusters/bulk",
    tag = "inventory",
    request_body = BulkCreateRequest<ClusterPayload>,
    responses(
        (status = 200, description = "Bulk cluster create result", body = BulkCreateResponse<Cluster>),
        (status = "4XX", description = "Bulk cluster create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Bulk cluster create failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_clusters(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<BulkCreateRequest<ClusterPayload>>,
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
    let mut created_indices: Vec<usize> = Vec::new();
    let mut errors = Vec::new();

    for (index, payload) in items.into_iter().enumerate() {
        match create_cluster_item(&state, payload).await {
            Ok(cluster) => {
                created.push(cluster);
                created_indices.push(index);
            }
            Err(error) => push_bulk_error(&mut errors, index, error),
        }
    }

    finalize_bulk_created(
        &state,
        CONTENT_TYPE_CLUSTER,
        BulkCreatedBatch {
            created: &mut created,
            created_indices: &created_indices,
            errors: &mut errors,
        },
        |cluster: &Cluster| cluster.id,
        |cluster: &Cluster| cluster.custom_fields.as_ref(),
        |cluster: &mut Cluster, value| cluster.custom_fields = Some(value),
    )
    .await;

    (StatusCode::OK, Json(BulkCreateResponse { created, errors })).into_response()
}

/// Update a cluster.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/clusters/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Cluster id"),
    ),
    request_body = ClusterPayload,
    responses(
        (status = 200, description = "Cluster updated", body = Cluster),
        (status = 404, description = "Cluster not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Cluster update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Cluster update failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_cluster(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ClusterPayload>,
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
    let cluster_id = match parse_uuid(&id, "cluster_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let name = match parse_required_string(&payload.name, "name") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let site_id = match parse_optional_uuid(payload.site_id.clone(), "site_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let tenant_id = match parse_optional_uuid(payload.tenant_id.clone(), "tenant_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let cluster_type_id =
        match parse_optional_uuid(payload.cluster_type_id.clone(), "cluster_type_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let cluster_group_id =
        match parse_optional_uuid(payload.cluster_group_id.clone(), "cluster_group_id") {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let service = state.services.inventory_admin.clone();
    let existing = match service.get_cluster(cluster_id).await {
        Ok(None) => return handle_app_error(AppError::NotFound("Cluster not found".to_string())),
        Ok(Some(existing)) => existing,
        Err(error) => return handle_app_error(error),
    };

    let custom_fields = match payload.custom_fields {
        Some(incoming) => match apply_custom_fields_write(
            &state,
            CONTENT_TYPE_CLUSTER,
            cluster_id,
            Some(incoming),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        },
        None => existing.custom_fields,
    };

    let mut cluster = Cluster {
        id: cluster_id,
        external_id: existing.external_id,
        name,
        site_id,
        tenant_id,
        cluster_type_id,
        cluster_group_id,
        status: payload.status.or(existing.status),
        description: payload.description,
        comments: payload.comments,
        tags: existing.tags,
        custom_fields,
        piping_synced_at: existing.piping_synced_at,
    };
    match service.update_cluster(cluster.clone()).await {
        Ok(()) => {
            let raw_custom_fields = cluster
                .custom_fields
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match masked_custom_fields(&state, CONTENT_TYPE_CLUSTER, cluster.id, &raw_custom_fields)
                .await
            {
                Ok(value) => cluster.custom_fields = Some(value),
                Err(error) => return handle_app_error(error),
            }
            bump_host_config_cache_generation(&state).await;
            (StatusCode::OK, Json(cluster)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a cluster.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/clusters/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Cluster id"),
    ),
    responses(
        (status = 204, description = "Cluster deleted"),
        (status = 404, description = "Cluster not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Cluster delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Cluster delete failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_cluster(
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
    let cluster_id = match parse_uuid(&id, "cluster_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let service = state.services.inventory_admin.clone();
    match service.get_cluster(cluster_id).await {
        Ok(None) => return handle_app_error(AppError::NotFound("Cluster not found".to_string())),
        Ok(Some(_)) => (),
        Err(error) => return handle_app_error(error),
    };

    match service.delete_cluster(cluster_id).await {
        Ok(()) => {
            bump_host_config_cache_generation(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => handle_app_error(error),
    }
}
