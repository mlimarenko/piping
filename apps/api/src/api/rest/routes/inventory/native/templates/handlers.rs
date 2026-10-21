use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};

use super::*;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::api::rest::routes::inventory::{
    PaginationQuery, ensure_access, parse_required_string, parse_uuid,
};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::application::{
    TemplatePreview, TemplateResponse, TemplateRunResponse, TemplateRunStepResponse,
    TemplateUpsertInput,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::native::template_repository::{
    INVENTORY_TEMPLATE_COLUMNS, INVENTORY_TEMPLATE_RUN_COLUMNS, TemplateRunListFilter,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use uuid::Uuid;

fn map_template(t: TemplateResponse) -> InventoryTemplateResponse {
    InventoryTemplateResponse {
        id: t.id,
        name: t.name,
        slug: t.slug,
        description: t.description,
        status: t.status,
        is_system: t.is_system,
        owner_user_id: t.owner_user_id,
        updated_by_user_id: t.updated_by_user_id,
        created_at: t.created_at,
        updated_at: t.updated_at,
        variables: t.variables,
        steps: t.steps,
    }
}

fn map_run(r: TemplateRunResponse) -> InventoryTemplateRunResponse {
    InventoryTemplateRunResponse {
        id: r.id,
        template_id: r.template_id,
        requested_by_user_id: r.requested_by_user_id,
        status: r.status,
        input_variables: r.input_variables,
        template_snapshot: r.template_snapshot,
        summary: r.summary,
        started_at: r.started_at,
        finished_at: r.finished_at,
        created_at: r.created_at,
    }
}

fn map_run_step(s: TemplateRunStepResponse) -> InventoryTemplateRunStepResponse {
    InventoryTemplateRunStepResponse {
        id: s.id,
        run_id: s.run_id,
        step_key: s.step_key,
        status: s.status,
        created_resource_ids: s.created_resource_ids,
        resolved_payload: s.resolved_payload,
        error_code: s.error_code,
        error_message: s.error_message,
        duration_ms: s.duration_ms,
        created_at: s.created_at,
    }
}

fn parse_actor_user_id(ctx: &RequestContext) -> Result<Uuid, AppError> {
    ctx.user_id
        .ok_or_else(|| AppError::Forbidden("Authentication required".to_string()))
}

/// List inventory templates.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/templates",
    tag = "inventory",
    params(
        ("status" = Option<String>, Query, description = "Template status filter"),
        ("search" = Option<String>, Query, description = "Template search text"),
        ("sort" = Option<String>, Query, description = "Sort column"),
        ("order" = Option<String>, Query, description = "Sort order: asc or desc"),
        ("limit" = Option<u32>, Query, description = "Maximum number of templates to return"),
        ("offset" = Option<u32>, Query, description = "Number of templates to skip"),
    ),
    responses(
        (status = 200, description = "Inventory templates listed", body = InventoryTemplateListResponse),
        (status = "4XX", description = "Template list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_inventory_templates(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<TemplateListQuery>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Read, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let (limit, offset) =
        crate::contexts::shared::pagination::normalize_page(query.limit, query.offset);
    let status_filter = parse_csv_strings(query.status.as_deref());
    let search_filter = normalize(query.search);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        INVENTORY_TEMPLATE_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(e) => return handle_app_error(e),
    };

    match state
        .services
        .inventory_templates
        .list_templates(status_filter, search_filter, sort, limit, offset)
        .await
    {
        Ok((items, meta)) => {
            let response = InventoryTemplateListResponse {
                items: items.into_iter().map(map_template).collect(),
                total: meta.total,
                limit: meta.limit,
                offset: meta.offset,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => handle_app_error(e),
    }
}

/// Create an inventory template.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/templates",
    tag = "inventory",
    request_body = InventoryTemplatePayload,
    responses(
        (status = 201, description = "Inventory template created", body = InventoryTemplateResponse),
        (status = "4XX", description = "Template create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_inventory_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<InventoryTemplatePayload>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Create, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    if let Err(e) = parse_required_string(&payload.name, "name") {
        return handle_app_error(e);
    }

    let actor_id = match parse_actor_user_id(&ctx) {
        Ok(id) => Some(id),
        Err(error) => return handle_app_error(error),
    };
    let input = TemplateUpsertInput {
        name: payload.name,
        slug: payload.slug,
        description: payload.description,
        status: payload.status,
        variables: payload.variables,
        steps: payload.steps,
        is_system: payload.is_system,
    };

    match state
        .services
        .inventory_templates
        .create_template(input, actor_id)
        .await
    {
        Ok(t) => (StatusCode::CREATED, Json(map_template(t))).into_response(),
        Err(e) => handle_app_error(e),
    }
}

/// Get an inventory template.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/templates/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory template ID"),
    ),
    responses(
        (status = 200, description = "Inventory template", body = InventoryTemplateResponse),
        (status = 404, description = "Inventory template not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Template request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_inventory_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Read, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let template_id = match parse_uuid(&id, "template_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state
        .services
        .inventory_templates
        .get_template(template_id)
        .await
    {
        Ok(Some(t)) => (StatusCode::OK, Json(map_template(t))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Template not found".to_string())),
        Err(e) => handle_app_error(e),
    }
}

/// Update an inventory template.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/templates/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory template ID"),
    ),
    request_body = InventoryTemplatePayload,
    responses(
        (status = 200, description = "Inventory template updated", body = InventoryTemplateResponse),
        (status = 404, description = "Inventory template not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Template update request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template update request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_inventory_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<InventoryTemplatePayload>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Update, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let template_id = match parse_uuid(&id, "template_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    if let Err(e) = parse_required_string(&payload.name, "name") {
        return handle_app_error(e);
    }

    let actor_id = match parse_actor_user_id(&ctx) {
        Ok(id) => Some(id),
        Err(error) => return handle_app_error(error),
    };
    let input = TemplateUpsertInput {
        name: payload.name,
        slug: payload.slug,
        description: payload.description,
        status: payload.status,
        variables: payload.variables,
        steps: payload.steps,
        is_system: payload.is_system,
    };

    match state
        .services
        .inventory_templates
        .update_template(template_id, input, actor_id)
        .await
    {
        Ok(t) => (StatusCode::OK, Json(map_template(t))).into_response(),
        Err(e) => handle_app_error(e),
    }
}

/// Archive an inventory template.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/templates/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory template ID"),
    ),
    responses(
        (status = 204, description = "Inventory template deleted"),
        (status = 404, description = "Inventory template not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Template delete request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template delete request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_inventory_template(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Delete, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let template_id = match parse_uuid(&id, "template_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state
        .services
        .inventory_templates
        .delete_template(template_id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => handle_app_error(e),
    }
}

/// Preview a template run without creating resources.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/templates/{id}/preview-run",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory template ID"),
    ),
    request_body = TemplateRunPayload,
    responses(
        (status = 200, description = "Inventory template run preview", body = PreviewRunResponse),
        (status = 404, description = "Inventory template not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Template preview request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template preview request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn preview_inventory_template_run(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<TemplateRunPayload>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Read, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let template_id = match parse_uuid(&id, "template_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state
        .services
        .inventory_templates
        .preview_run(template_id, payload.variables)
        .await
    {
        Ok(TemplatePreview {
            template_id,
            variables,
            steps,
        }) => (
            StatusCode::OK,
            Json(PreviewRunResponse {
                template_id,
                variables,
                steps,
            }),
        )
            .into_response(),
        Err(e) => handle_app_error(e),
    }
}

/// Run a template — create the configured resources. Runs asynchronously; returns the accepted run.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/templates/{id}/runs",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory template ID"),
    ),
    request_body = TemplateRunPayload,
    responses(
        (status = 202, description = "Inventory template run accepted", body = InventoryTemplateRunResponse),
        (status = 404, description = "Inventory template not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Template run create request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template run create request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_inventory_template_run(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<TemplateRunPayload>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Create, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let template_id = match parse_uuid(&id, "template_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let requested_by = match parse_actor_user_id(&ctx) {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state
        .services
        .inventory_templates
        .create_run(template_id, requested_by, payload.variables)
        .await
    {
        Ok(r) => (StatusCode::ACCEPTED, Json(map_run(r))).into_response(),
        Err(e) => handle_app_error(e),
    }
}

/// List runs for an inventory template.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/templates/{id}/runs",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory template ID"),
        ("search" = Option<String>, Query, description = "Template run search text"),
        ("limit" = Option<u32>, Query, description = "Maximum number of template runs to return"),
        ("offset" = Option<u32>, Query, description = "Number of template runs to skip"),
    ),
    responses(
        (status = 200, description = "Inventory template runs listed", body = [InventoryTemplateRunResponse]),
        (status = 404, description = "Inventory template not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Template run list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template run list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_inventory_template_runs(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<PaginationQuery>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Read, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let template_id = match parse_uuid(&id, "template_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };
    let (limit, offset) =
        crate::contexts::shared::pagination::normalize_page(query.limit, query.offset);
    let search_filter = normalize(query.search);

    match state
        .services
        .inventory_templates
        .list_runs(template_id, search_filter, limit, offset)
        .await
    {
        Ok(runs) => {
            let items: Vec<InventoryTemplateRunResponse> = runs.into_iter().map(map_run).collect();
            (StatusCode::OK, Json(items)).into_response()
        }
        Err(e) => handle_app_error(e),
    }
}

/// List inventory template runs across templates.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/template-runs",
    tag = "inventory",
    params(
        ("status" = Option<String>, Query, description = "Template run status filter"),
        ("template_id" = Option<String>, Query, description = "Template ID filter"),
        ("search" = Option<String>, Query, description = "Template run search text"),
        ("sort" = Option<String>, Query, description = "Sort column"),
        ("order" = Option<String>, Query, description = "Sort order: asc or desc"),
        ("limit" = Option<u32>, Query, description = "Maximum number of template runs to return"),
        ("offset" = Option<u32>, Query, description = "Number of template runs to skip"),
    ),
    responses(
        (status = 200, description = "Inventory template runs listed", body = InventoryTemplateRunListResponse),
        (status = "4XX", description = "Template run list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template run list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_inventory_template_runs_global(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<TemplateRunListQuery>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Read, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let (limit, offset) =
        crate::contexts::shared::pagination::normalize_page(query.limit, query.offset);
    let status_filter = parse_csv_strings(query.status.as_deref());
    let template_filter = match parse_csv_uuids(query.template_id.as_deref(), "template_id") {
        Ok(values) => values,
        Err(e) => return handle_app_error(e),
    };
    let search_filter = normalize(query.search);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        INVENTORY_TEMPLATE_RUN_COLUMNS,
    ) {
        Ok(sort) => sort,
        Err(e) => return handle_app_error(e),
    };

    match state
        .services
        .inventory_templates
        .list_runs_page(
            TemplateRunListFilter {
                status_filter,
                template_filter,
                search_filter,
                sort,
            },
            limit,
            offset,
        )
        .await
    {
        Ok((runs, meta)) => {
            let response = InventoryTemplateRunListResponse {
                items: runs.into_iter().map(map_run).collect(),
                total: meta.total,
                limit: meta.limit,
                offset: meta.offset,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => handle_app_error(e),
    }
}

/// Get an inventory template run.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/template-runs/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory template run ID"),
    ),
    responses(
        (status = 200, description = "Inventory template run", body = InventoryTemplateRunResponse),
        (status = 404, description = "Inventory template run not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Template run request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template run request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_inventory_template_run(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Read, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let run_id = match parse_uuid(&run_id, "run_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state.services.inventory_templates.get_run(run_id).await {
        Ok(Some(r)) => (StatusCode::OK, Json(map_run(r))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Run not found".to_string())),
        Err(e) => handle_app_error(e),
    }
}

/// List the steps of a template run.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/template-runs/{id}/steps",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Inventory template run ID"),
    ),
    responses(
        (status = 200, description = "Inventory template run steps listed", body = [InventoryTemplateRunStepResponse]),
        (status = 404, description = "Inventory template run not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "4XX", description = "Template run step list request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Template run step list request failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_inventory_template_run_steps(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> impl IntoResponse {
    let ctx = build_request_context(&request_id, auth_context, &headers);
    if let Err(e) = ensure_access(&state, &ctx, Action::Read, ResourceType::Inventory).await {
        return handle_app_error(e);
    }
    let run_id = match parse_uuid(&run_id, "run_id") {
        Ok(v) => v,
        Err(e) => return handle_app_error(e),
    };

    match state
        .services
        .inventory_templates
        .list_run_steps(run_id)
        .await
    {
        Ok(rows) => {
            let items: Vec<InventoryTemplateRunStepResponse> =
                rows.into_iter().map(map_run_step).collect();
            (StatusCode::OK, Json(items)).into_response()
        }
        Err(e) => handle_app_error(e),
    }
}

fn normalize(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}
