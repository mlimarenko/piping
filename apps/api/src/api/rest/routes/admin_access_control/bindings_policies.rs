use super::*;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Query, RequireAuthenticated};

macro_rules! access_control_admin {
    ($state:expr) => {
        $state.services.access_control_admin.clone()
    };
}

/// List role assignments.
#[utoipa::path(
    get,
    path = "/api/v1/admin/access-control/role-assignments",
    tag = "access-control",
    params(
        ("role_id" = Option<String>, Query, description = "Comma-separated role ids; matches any (IN)"),
        ("user_id" = Option<String>, Query, description = "Comma-separated user ids; matches any (IN)"),
        ("group_id" = Option<String>, Query, description = "Comma-separated group ids; matches any (IN)"),
        ("subject_type" = Option<String>, Query, description = "Comma-separated subject type filters: user, group"),
        ("search" = Option<String>, Query, description = "Free-text search over role name and subject (user username/email, group name), case-insensitive ILIKE"),
        ("limit" = Option<u32>, Query, description = "Maximum number of role assignments to return"),
        ("offset" = Option<u32>, Query, description = "Number of role assignments to skip"),
        ("sort" = Option<String>, Query, description = "Sort key: created, role, subject (allowlisted; unknown rejected)"),
        ("order" = Option<String>, Query, description = "Sort direction: asc (default) or desc"),
    ),
    responses(
        (status = 200, description = "Role assignments", body = AccessControlListEnvelope<RoleAssignmentResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_role_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<RoleAssignmentListQuery>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::Role).await
    {
        return handle_app_error(error);
    }

    let role_id = match parse_csv_uuids(query.role_id.as_deref(), "role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let user_id = match parse_csv_uuids(query.user_id.as_deref(), "user_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let group_id = match parse_csv_uuids(query.group_id.as_deref(), "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let subject_type = parse_csv_strings(query.subject_type.as_deref());
    if let Some(invalid) = subject_type
        .iter()
        .find(|value| value.as_str() != "user" && value.as_str() != "group")
    {
        return handle_app_error(AppError::Validation(format!(
            "Invalid subject_type {invalid:?}: expected one of: user, group"
        )));
    }
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        ROLE_ASSIGNMENT_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let (limit, offset) = normalize_page(query.limit, query.offset);
    let service = access_control_admin!(state);
    match service
        .list_role_assignments_page(
            RoleAssignmentListParams {
                role_id,
                user_id,
                group_id,
                subject_type,
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
                .map(RoleAssignmentResponse::from)
                .collect::<Vec<_>>(),
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// List the permissions granted by a role.
#[utoipa::path(
    get,
    path = "/api/v1/admin/access-control/roles/{id}/grants",
    tag = "access-control",
    responses(
        (status = 200, description = "Role grants", body = RoleGrantsResponse),
    ),
)]
pub async fn get_role_grants(
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

    let service = access_control_admin!(state);
    match service.get_role_grants(role_id).await {
        Ok(grants) => Json(RoleGrantsResponse {
            role_id: grants.role_id.to_string(),
            assignments: grants
                .assignments
                .into_iter()
                .map(RoleAssignmentResponse::from)
                .collect(),
            policies: grants
                .policies
                .into_iter()
                .map(PolicyResponse::from)
                .collect(),
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Assign a role to a subject.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/role-assignments",
    tag = "access-control",
    request_body = RoleAssignmentInput,
    responses(
        (status = 200, description = "Role assignment created", body = RoleAssignmentResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_role_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<RoleAssignmentInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Create, ResourceType::Role).await
    {
        return handle_app_error(error);
    }

    let has_user = payload.user_id.is_some();
    let has_group = payload.group_id.is_some();
    if has_user == has_group {
        return handle_app_error(AppError::Validation(
            "Exactly one of user_id or group_id must be provided".to_string(),
        ));
    }

    let user_id = match parse_optional_uuid(payload.user_id.clone(), "user_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let group_id = match parse_optional_uuid(payload.group_id.clone(), "group_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let role_id = match parse_uuid(&payload.role_id, "role_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let assignment = DomainRoleAssignment {
        id: Uuid::now_v7(),
        user_id,
        group_id,
        role_id,
    };

    let service = access_control_admin!(state);
    match service.create_role_assignment(assignment).await {
        Ok(assignment) => {
            record_access_control_mutation(
                state.services.audit.as_ref(),
                &request_context,
                "access_control.role_assignment.created",
                "role_assignment",
                assignment.id,
            )
            .await;
            Json(RoleAssignmentResponse::from(assignment)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Remove a role assignment.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/access-control/role-assignments/{id}",
    tag = "access-control",
    responses(
        (status = 200, description = "Role assignment deleted", body = DeleteResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_role_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(assignment_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Delete, ResourceType::Role).await
    {
        return handle_app_error(error);
    }

    let assignment_id = match parse_uuid(&assignment_id, "assignment_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = access_control_admin!(state);
    match service.delete_role_assignment(assignment_id).await {
        Ok(deleted) if deleted => {
            record_access_control_mutation(
                state.services.audit.as_ref(),
                &request_context,
                "access_control.role_assignment.deleted",
                "role_assignment",
                assignment_id,
            )
            .await;
            Json(DeleteResponse { deleted }).into_response()
        }
        Ok(_) => handle_app_error(AppError::NotFound("Role assignment not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// List access-control policies.
#[utoipa::path(
    get,
    path = "/api/v1/admin/access-control/policies",
    tag = "access-control",
    params(
        ("name" = Option<String>, Query, description = "Filter by policy name"),
        ("effect" = Option<String>, Query, description = "Comma-separated effects (allow/deny); matches any (IN)"),
        ("enabled" = Option<String>, Query, description = "Comma-separated enabled flags (true/false); matches any (IN)"),
        ("limit" = Option<u32>, Query, description = "Maximum number of policies to return"),
        ("offset" = Option<u32>, Query, description = "Number of policies to skip"),
        ("sort" = Option<String>, Query, description = "Sort key: name, effect, enabled (allowlisted; unknown rejected)"),
        ("order" = Option<String>, Query, description = "Sort direction: asc (default) or desc"),
    ),
    responses(
        (status = 200, description = "Policies", body = AccessControlListEnvelope<PolicyResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_policies(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<PolicyListQuery>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::Policy).await
    {
        return handle_app_error(error);
    }

    let effect = match parse_csv_policy_effects(query.effect.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let enabled = match parse_csv_bools(query.enabled.as_deref()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        POLICY_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let (limit, offset) = normalize_page(query.limit, query.offset);
    let service = access_control_admin!(state);
    match service
        .list_policies_page(
            PolicyListParams {
                name: query.name,
                effect,
                enabled,
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
                .map(PolicyResponse::from)
                .collect::<Vec<_>>(),
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get an access-control policy.
#[utoipa::path(
    get,
    path = "/api/v1/admin/access-control/policies/{id}",
    tag = "access-control",
    responses(
        (status = 200, description = "Policy", body = PolicyResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_policy(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(policy_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::Policy).await
    {
        return handle_app_error(error);
    }

    let policy_id = match parse_uuid(&policy_id, "policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = access_control_admin!(state);
    match service.get_policy(policy_id).await {
        Ok(Some(policy)) => Json(PolicyResponse::from(policy)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Policy not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create an access-control policy.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/policies",
    tag = "access-control",
    request_body = PolicyInput,
    responses(
        (status = 200, description = "Policy created", body = PolicyResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_policy(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<PolicyInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::Policy,
    )
    .await
    {
        return handle_app_error(error);
    }

    let policy = match build_policy_from_input(payload, Uuid::now_v7()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = access_control_admin!(state);
    match service.create_policy(policy).await {
        Ok(policy) => Json(PolicyResponse::from(policy)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update an access-control policy.
#[utoipa::path(
    put,
    path = "/api/v1/admin/access-control/policies/{id}",
    tag = "access-control",
    request_body = PolicyInput,
    responses(
        (status = 200, description = "Policy updated", body = PolicyResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_policy(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(policy_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<PolicyInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Policy,
    )
    .await
    {
        return handle_app_error(error);
    }

    let policy_id = match parse_uuid(&policy_id, "policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let policy = match build_policy_from_input(payload, policy_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = access_control_admin!(state);
    match service.update_policy(policy).await {
        Ok(policy) => Json(PolicyResponse::from(policy)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete an access-control policy.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/access-control/policies/{id}",
    tag = "access-control",
    responses(
        (status = 200, description = "Policy deleted", body = DeleteResponse),
    ),
)]
pub async fn delete_policy(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(policy_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::Policy,
    )
    .await
    {
        return handle_app_error(error);
    }

    let policy_id = match parse_uuid(&policy_id, "policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = access_control_admin!(state);
    match service.delete_policy(policy_id).await {
        Ok(()) => Json(DeleteResponse { deleted: true }).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Enable an access-control policy.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/policies/{id}/enable",
    tag = "access-control",
    responses(
        (status = 200, description = "Policy enabled", body = PolicyResponse),
    ),
)]
pub async fn enable_policy(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(policy_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Policy,
    )
    .await
    {
        return handle_app_error(error);
    }

    let policy_id = match parse_uuid(&policy_id, "policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = access_control_admin!(state);
    match service.set_policy_enabled(policy_id, true).await {
        Ok(policy) => Json(PolicyResponse::from(policy)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Disable an access-control policy.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/policies/{id}/disable",
    tag = "access-control",
    responses(
        (status = 200, description = "Policy disabled", body = PolicyResponse),
    ),
)]
pub async fn disable_policy(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(policy_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Policy,
    )
    .await
    {
        return handle_app_error(error);
    }

    let policy_id = match parse_uuid(&policy_id, "policy_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = access_control_admin!(state);
    match service.set_policy_enabled(policy_id, false).await {
        Ok(policy) => Json(PolicyResponse::from(policy)).into_response(),
        Err(error) => handle_app_error(error),
    }
}
