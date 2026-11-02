use super::*;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Query, RequireAuthenticated};

/// List users.
#[utoipa::path(
    get,
    path = "/api/v1/admin/access-control/users",
    tag = "access-control",
    params(
        ("limit" = Option<u32>, Query, description = "Maximum number of users to return"),
        ("offset" = Option<u32>, Query, description = "Number of users to skip"),
        ("search" = Option<String>, Query, description = "Filter by username or email (case-insensitive ILIKE)"),
        ("status" = Option<String>, Query, description = "Comma-separated statuses; matches any (IN)"),
        ("auth_source" = Option<String>, Query, description = "Comma-separated auth sources; matches any (IN)"),
        ("sort" = Option<String>, Query, description = "Sort key: username, email, status, auth_source (allowlisted; unknown rejected)"),
        ("order" = Option<String>, Query, description = "Sort direction: asc (default) or desc"),
    ),
    responses(
        (status = 200, description = "Users", body = AccessControlListEnvelope<UserResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_users(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<UserListQuery>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::User).await
    {
        return handle_app_error(error);
    }

    let status = parse_csv_strings(query.status.as_deref());
    let auth_source = parse_csv_strings(query.auth_source.as_deref());
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        USER_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let (limit, offset) = normalize_page(query.limit, query.offset);
    let service = state.services.access_control_admin.clone();
    match service
        .list_users_page(
            UserListParams {
                search: query.search,
                status,
                auth_source,
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
                .map(UserResponse::from)
                .collect::<Vec<_>>(),
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get a user.
#[utoipa::path(
    get,
    path = "/api/v1/admin/access-control/users/{id}",
    tag = "access-control",
    responses(
        (status = 200, description = "User", body = UserResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_user(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_user_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::User).await
    {
        return handle_app_error(error);
    }

    let user_id = match parse_uuid(&target_user_id, "user_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.access_control_admin.clone();
    match service.get_user(user_id).await {
        Ok(Some(user)) => Json(UserResponse::from(user)).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("User not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a user.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/users",
    tag = "access-control",
    request_body = UserInput,
    responses(
        (status = 200, description = "User created", body = UserResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_user(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<UserInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Create, ResourceType::User).await
    {
        return handle_app_error(error);
    }

    let credential_command = match parse_optional_credential_command(
        payload.credential.as_ref(),
        &payload.auth_source,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Some(command) = credential_command.as_ref()
        && let Err(error) =
            validate_credential_command_for_policy(command, state.settings.auth.password_min_length)
    {
        return handle_app_error(error);
    }
    let user = match build_user(&payload, Uuid::now_v7()) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.access_control_admin.clone();
    match service.create_user(user).await {
        Ok(user) => {
            let generated_password = match credential_command {
                Some(command) => match service
                    .set_local_password_with_command(user.id, command)
                    .await
                {
                    Ok(result) => result.generated_password,
                    Err(error) => return handle_app_error(error),
                },
                None => None,
            };
            record_access_control_mutation(
                state.services.audit.as_ref(),
                &request_context,
                "access_control.user.created",
                "user",
                user.id,
            )
            .await;
            Json(UserResponse::from_user(user, generated_password)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a user.
#[utoipa::path(
    put,
    path = "/api/v1/admin/access-control/users/{id}",
    tag = "access-control",
    request_body = UserInput,
    responses(
        (status = 200, description = "User updated", body = UserResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_user(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_user_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<UserInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Update, ResourceType::User).await
    {
        return handle_app_error(error);
    }

    let user_id = match parse_uuid(&target_user_id, "user_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let credential_command = match parse_optional_credential_command(
        payload.credential.as_ref(),
        &payload.auth_source,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Some(command) = credential_command.as_ref()
        && let Err(error) =
            validate_credential_command_for_policy(command, state.settings.auth.password_min_length)
    {
        return handle_app_error(error);
    }
    let user = match build_user(&payload, user_id) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.access_control_admin.clone();
    match service.update_user(user).await {
        Ok(user) => {
            let generated_password = match credential_command {
                Some(command) => match service
                    .set_local_password_with_command(user.id, command)
                    .await
                {
                    Ok(result) => result.generated_password,
                    Err(error) => return handle_app_error(error),
                },
                None => None,
            };
            record_access_control_mutation(
                state.services.audit.as_ref(),
                &request_context,
                "access_control.user.updated",
                "user",
                user.id,
            )
            .await;
            Json(UserResponse::from_user(user, generated_password)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Set a new password for a user.
#[utoipa::path(
    put,
    path = "/api/v1/admin/access-control/users/{id}/password",
    tag = "access-control",
    request_body = PasswordRotationInput,
    responses(
        (status = 200, description = "User password rotated", body = PasswordRotationResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn rotate_user_password(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(target_user_id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<PasswordRotationInput>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Update, ResourceType::User).await
    {
        return handle_app_error(error);
    }

    let user_id = match parse_uuid(&target_user_id, "user_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let command = match parse_credential_command(payload.mode.as_str(), payload.password) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) =
        validate_credential_command_for_policy(&command, state.settings.auth.password_min_length)
    {
        return handle_app_error(error);
    }

    let service = state.services.access_control_admin.clone();
    match service
        .set_local_password_with_command(user_id, command)
        .await
    {
        Ok(result) => {
            record_access_control_mutation(
                state.services.audit.as_ref(),
                &request_context,
                "access_control.user.password_rotated",
                "user",
                result.credential.user_id,
            )
            .await;
            Json(PasswordRotationResponse {
                user_id: result.credential.user_id.to_string(),
                password_changed_at: result.credential.password_changed_at.to_rfc3339(),
                generated_password: result.generated_password,
            })
            .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Delete a user.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/access-control/users/{id}",
    tag = "access-control",
    responses(
        (status = 200, description = "User deleted", body = DeleteResponse),
        (status = 404, description = "Not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_user(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(target_user_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Delete, ResourceType::User).await
    {
        return handle_app_error(error);
    }

    let user_id = match parse_uuid(&target_user_id, "user_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.access_control_admin.clone();
    match service.delete_user(user_id).await {
        Ok(deleted) if deleted => {
            record_access_control_mutation(
                state.services.audit.as_ref(),
                &request_context,
                "access_control.user.deleted",
                "user",
                user_id,
            )
            .await;
            Json(DeleteResponse { deleted }).into_response()
        }
        Ok(_) => handle_app_error(AppError::NotFound("User not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}
