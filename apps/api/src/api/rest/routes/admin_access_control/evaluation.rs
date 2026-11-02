use super::*;
use crate::api::rest::envelope_extractors::{EnvelopeJson, RequireAuthenticated};
use crate::contexts::shared::request_context::RequestAuth;

/// Evaluate whether a subject may perform an action on a resource.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/access-evaluations",
    tag = "access-control",
    request_body = AccessEvaluationRequest,
    responses(
        (status = 200, description = "Access evaluation", body = AccessEvaluationResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn evaluate_access(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<AccessEvaluationRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::Policy).await
    {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::User).await
    {
        return handle_app_error(error);
    }

    let target_user_id = match parse_uuid(&payload.user_id, "user_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let action = match parse_action(&payload.action) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let resource_type = match parse_resource_type(&payload.resource_type) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let attributes = match parse_attributes(payload.attributes) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let target_request_context = RequestContext::new(
        Some(target_user_id),
        request_context.auth_method.clone(),
        request_context.request_ip.clone(),
        request_context.correlation_id.clone(),
        RequestAuth {
            oidc_groups: Vec::new(),
            api_token_scopes: request_context.api_token_scopes.clone(),
            api_token_scope_resource_override: request_context
                .api_token_scope_resource_override
                .clone(),
        },
    );

    let access_service = state.services.access_control.clone();
    let metadata = match access_service
        .check_access_with_trace(
            &target_request_context,
            action,
            resource_type,
            payload.resource_id.as_deref(),
            attributes,
        )
        .await
    {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let principal = Principal::user(target_user_id);
    let resolved_subjects = match access_service.resolve_subjects(&principal).await {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let policy_ids: HashSet<Uuid> = metadata
        .matched_allow
        .iter()
        .chain(metadata.matched_deny.iter())
        .copied()
        .collect();
    let service = state.services.access_control_admin.clone();
    let mut policies = HashMap::new();
    for policy_id in &policy_ids {
        match service.get_policy(*policy_id).await {
            Ok(Some(policy)) => {
                policies.insert(*policy_id, policy);
            }
            Ok(None) => {}
            Err(error) => return handle_app_error(error),
        }
    }

    // Reconstruct trace provenance from the resolved subject set alone (the
    // `rbac_policy_binding` re-query is gone). A matched `role:{uuid}` subject is
    // resolved to its role name for `via_roles`; an unresolvable role uuid and any
    // other kind (`user:`/`group:`/`service_principal:`) goes to
    // `via_subjects`. Build the role-name lookup once for the role uuids that
    // actually appear in the resolved subjects.
    let subject_set: HashSet<String> = resolved_subjects.into_iter().collect();
    let mut role_names: HashMap<Uuid, String> = HashMap::new();
    for subject in &subject_set {
        if let Some(raw_uuid) = subject.strip_prefix("role:")
            && let Ok(role_id) = Uuid::parse_str(raw_uuid)
            && let std::collections::hash_map::Entry::Vacant(entry) = role_names.entry(role_id)
        {
            match service.get_role(role_id).await {
                Ok(Some(role)) => {
                    entry.insert(role.name);
                }
                Ok(None) => {}
                Err(error) => return handle_app_error(error),
            }
        }
    }

    let matched_allow = metadata
        .matched_allow
        .iter()
        .map(|policy_id| {
            map_policy_trace(
                policy_id,
                &policies,
                &subject_set,
                &role_names,
                PolicyEffect::Allow,
            )
        })
        .collect();
    let matched_deny = metadata
        .matched_deny
        .iter()
        .map(|policy_id| {
            map_policy_trace(
                policy_id,
                &policies,
                &subject_set,
                &role_names,
                PolicyEffect::Deny,
            )
        })
        .collect();

    Json(AccessEvaluationResponse {
        decision: policy_decision_to_string(metadata.decision).to_string(),
        reasons: metadata.reasons,
        matched_allow,
        matched_deny,
    })
    .into_response()
}

/// Invalidate the entire policy-evaluation cache.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/cache/invalidate",
    tag = "access-control",
    responses(
        (status = 200, description = "Policy cache invalidated", body = CacheInvalidationResponse),
    ),
)]
pub async fn invalidate_policy_cache(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::ManagePolicies,
        ResourceType::Policy,
    )
    .await
    {
        return handle_app_error(error);
    }

    let service = state.services.access_control_admin.clone();
    match service.invalidate_policy_cache().await {
        Ok(generation) => Json(CacheInvalidationResponse {
            invalidated: true,
            scope: "global".to_string(),
            generation,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Invalidate the policy-evaluation cache for one user.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/cache/invalidate/{user_id}",
    tag = "access-control",
    responses(
        (status = 200, description = "User policy cache invalidated", body = CacheInvalidationResponse),
    ),
)]
pub async fn invalidate_policy_cache_for_user(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::ManagePolicies,
        ResourceType::Policy,
    )
    .await
    {
        return handle_app_error(error);
    }

    let user_id = match parse_uuid(&user_id, "user_id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    let service = state.services.access_control_admin.clone();
    match service.invalidate_policy_cache_for_user(user_id).await {
        Ok(generation) => Json(CacheInvalidationResponse {
            invalidated: true,
            scope: format!("user:{}", user_id),
            generation,
        })
        .into_response(),
        Err(error) => handle_app_error(error),
    }
}
