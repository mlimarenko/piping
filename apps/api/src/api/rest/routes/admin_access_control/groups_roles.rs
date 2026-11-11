use super::*;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Query, RequireAuthenticated};

/// List groups.
#[utoipa::path(
    get,
    path = "/api/v1/admin/access-control/groups",
    tag = "access-control",
    params(
        ("limit" = Option<u32>, Query, description = "Maximum number of groups to return"),
        ("offset" = Option<u32>, Query, description = "Number of groups to skip"),
        ("search" = Option<String>, Query, description = "Filter by group name (case-insensitive ILIKE)"),
        ("sort" = Option<String>, Query, description = "Sort key: name, external_id (allowlisted; unknown rejected)"),
        ("order" = Option<String>, Query, description = "Sort direction: asc (default) or desc"),
    ),
    responses(
        (status = 200, description = "Groups", body = AccessControlListEnvelope<GroupResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPageQuery>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::Group).await
    {
        return handle_app_error(error);
    }

    let sort =
        match SortSpec::parse_columns(query.sort.as_deref(), query.order.as_deref(), GROUP_COLUMNS)
        {
            Ok(value) => value,
            Err(error) => return handle_app_error(error),
        };
    let (limit, offset) = normalize_page(query.limit, query.offset);
    let service = state.services.access_control_admin.clone();
    match service
        .list_groups_page(
            NamedListParams {
                search: query.search,
                sort,
            },
            limit,
            offset,
        )
        .await
    {
        Ok(page) => Json(AccessControlListEnvelope {
            items: page
                .items
                .into_iter()
                .map(GroupResponse::from)
                .collect::<Vec<_>>(),
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a group.
#[utoipa::path(
    get,
    path = "/api/v1/admin/access-control/groups/{id}",
    tag = "access-control",
    responses(
        (status = 200, description = "Group", body = GroupResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::Group).await
    {
        return handle_app_error(error);
    }

    let group_id = match parse_uuid(&group_id, "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.access_control_admin.clone();
    match service.get_group(group_id).await {
        Ok(Some(group)) => Json(GroupResponse::from(group)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Group not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a group.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/groups",
    tag = "access-control",
    request_body = GroupInput,
    responses(
        (status = 200, description = "Group created", body = GroupResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<GroupInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::Group,
    )
    .await
    {
        return handle_app_error(error);
    }

    let group = DomainGroup {
        id: Uuid::now_v7(),
        name: payload.name,
        external_id: payload.external_id,
    };

    let service = state.services.access_control_admin.clone();
    match service.create_group(group).await {
        Ok(group) => Json(GroupResponse::from(group)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a group.
#[utoipa::path(
    put,
    path = "/api/v1/admin/access-control/groups/{id}",
    tag = "access-control",
    request_body = GroupInput,
    responses(
        (status = 200, description = "Group updated", body = GroupResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(group_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<GroupInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Group,
    )
    .await
    {
        return handle_app_error(error);
    }

    let group_id = match parse_uuid(&group_id, "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let group = DomainGroup {
        id: group_id,
        name: payload.name,
        external_id: payload.external_id,
    };

    let service = state.services.access_control_admin.clone();
    match service.update_group(group).await {
        Ok(group) => Json(GroupResponse::from(group)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a group.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/access-control/groups/{id}",
    tag = "access-control",
    responses(
        (status = 200, description = "Group deleted", body = DeleteResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_group(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::Group,
    )
    .await
    {
        return handle_app_error(error);
    }

    let group_id = match parse_uuid(&group_id, "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.access_control_admin.clone();
    match service.delete_group(group_id).await {
        Ok(deleted) if deleted => Json(DeleteResponse { deleted }).into_response(),
        Ok(_) => handle_app_error(AppError::NotFound("Group not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// List roles.
#[utoipa::path(
    get,
    path = "/api/v1/admin/access-control/roles",
    tag = "access-control",
    params(
        ("limit" = Option<u32>, Query, description = "Maximum number of roles to return"),
        ("offset" = Option<u32>, Query, description = "Number of roles to skip"),
        ("search" = Option<String>, Query, description = "Filter by role name (case-insensitive ILIKE)"),
        ("sort" = Option<String>, Query, description = "Sort key: name (allowlisted; unknown rejected)"),
        ("order" = Option<String>, Query, description = "Sort direction: asc (default) or desc"),
    ),
    responses(
        (status = 200, description = "Roles", body = AccessControlListEnvelope<RoleResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ListPageQuery>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::Role).await
    {
        return handle_app_error(error);
    }

    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        ROLE_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let (limit, offset) = normalize_page(query.limit, query.offset);
    let service = state.services.access_control_admin.clone();
    match service
        .list_roles_page(
            NamedListParams {
                search: query.search,
                sort,
            },
            limit,
            offset,
        )
        .await
    {
        Ok(page) => Json(AccessControlListEnvelope {
            items: page
                .items
                .into_iter()
                .map(RoleResponse::from)
                .collect::<Vec<_>>(),
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a role.
#[utoipa::path(
    get,
    path = "/api/v1/admin/access-control/roles/{id}",
    tag = "access-control",
    responses(
        (status = 200, description = "Role", body = RoleResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(role_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::Role).await
    {
        return handle_app_error(error);
    }

    let role_id = match parse_uuid(&role_id, "role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.access_control_admin.clone();
    match service.get_role(role_id).await {
        Ok(Some(role)) => Json(RoleResponse::from(role)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Role not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a role.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/roles",
    tag = "access-control",
    request_body = RoleInput,
    responses(
        (status = 200, description = "Role created", body = RoleResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RoleInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Create, ResourceType::Role).await
    {
        return handle_app_error(error);
    }

    let role = DomainRole {
        id: Uuid::now_v7(),
        name: payload.name,
        description: payload.description,
    };

    let service = state.services.access_control_admin.clone();
    match service.create_role(role).await {
        Ok(role) => {
            record_access_control_mutation(
                state.services.audit.as_ref(),
                &request_context,
                "access_control.role.created",
                "role",
                role.id,
            )
            .await;
            Json(RoleResponse::from(role)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a role.
#[utoipa::path(
    put,
    path = "/api/v1/admin/access-control/roles/{id}",
    tag = "access-control",
    request_body = RoleInput,
    responses(
        (status = 200, description = "Role updated", body = RoleResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(role_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<RoleInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Update, ResourceType::Role).await
    {
        return handle_app_error(error);
    }

    let role_id = match parse_uuid(&role_id, "role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let role = DomainRole {
        id: role_id,
        name: payload.name,
        description: payload.description,
    };

    let service = state.services.access_control_admin.clone();
    match service.update_role(role).await {
        Ok(role) => Json(RoleResponse::from(role)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a role.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/access-control/roles/{id}",
    tag = "access-control",
    responses(
        (status = 200, description = "Role deleted", body = DeleteResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_role(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(role_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Delete, ResourceType::Role).await
    {
        return handle_app_error(error);
    }

    let role_id = match parse_uuid(&role_id, "role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.access_control_admin.clone();
    match service.delete_role(role_id).await {
        Ok(deleted) if deleted => {
            record_access_control_mutation(
                state.services.audit.as_ref(),
                &request_context,
                "access_control.role.deleted",
                "role",
                role_id,
            )
            .await;
            Json(DeleteResponse { deleted }).into_response()
        }
        Ok(_) => handle_app_error(AppError::NotFound("Role not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}
