use axum::{
    Extension,
    extract::FromRequestParts,
    http::{HeaderMap, request::Parts},
    response::Response,
};

use crate::api::client_ip::extract_client_ip;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::contexts::shared::request_context::{RequestAuth, RequestContext};
use crate::errors::AppError;

pub fn build_request_context(
    request_id: &RequestId,
    auth_context: Option<Extension<AuthContext>>,
    headers: &HeaderMap,
) -> RequestContext {
    let auth_context = auth_context
        .map(|value| value.0)
        .unwrap_or_else(AuthContext::anonymous);
    let client_ip = extract_client_ip(headers);
    let api_token_id = auth_context.api_token_id.clone();
    let mut request_context = RequestContext::new(
        auth_context.user_id,
        auth_context.auth_method,
        client_ip,
        request_id.0.clone(),
        RequestAuth {
            oidc_groups: auth_context.oidc_groups,
            api_token_scopes: auth_context.api_token_scopes,
            api_token_scope_resource_override: auth_context.api_token_scope_resource_override,
        },
    );
    request_context.api_token_id = api_token_id;
    request_context.playbook_job_binding = auth_context.playbook_job_binding;
    request_context
}

pub(crate) async fn native_inventory_authority(
    axum::extract::State(state): axum::extract::State<crate::app::AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if !request.uri().path().starts_with("/api/v1/inventory/") {
        return next.run(request).await;
    }
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .cloned()
        .unwrap_or_else(|| RequestId(uuid::Uuid::now_v7().to_string()));
    let auth = request
        .extensions()
        .get::<AuthContext>()
        .cloned()
        .map(Extension);
    let context = build_request_context(&request_id, auth, request.headers());
    crate::contexts::inventory::application::tag_authorization::NativeTagAuthority::new(
        state.services.policy_evaluation.clone(),
        state.services.user_api_tokens.clone(),
        context,
    )
    .scope(next.run(request))
    .await
}

/// Authorize the identifying projection after the repository's complete locked
/// inventory integrity scan. An incomplete sample cannot prove every counted
/// object's READ ceiling, so it is refused without identifying details.
pub(crate) async fn project_inventory_delete_error(
    state: &crate::app::AppState,
    context: &RequestContext,
    error: AppError,
) -> AppError {
    use crate::api::permission_helpers::ensure_access_with_resource;
    use crate::contexts::access_control::domain::{Action, ResourceType};
    use crate::contexts::inventory::domain::content_type::CONTENT_TYPES;
    let AppError::DeleteBlocked(details) = error else {
        return error;
    };
    let private = || {
        AppError::Conflict(
            "DELETE_BLOCKED:Cannot delete this record while related objects still exist".into(),
        )
    };
    for group in &details.blockers {
        // A group may carry the total count plus a capped example list. If the
        // examples are not the complete counted set, authorizing them cannot
        // prove the aggregate: withhold rather than expose an unproven total.
        if !group_is_fully_exampled(group) {
            return private();
        }
        for example in &group.examples {
            let Some(id) = example.id else {
                return private();
            };
            if group.table == "playbook_target_set" {
                if ensure_access_with_resource(
                    state,
                    context,
                    Action::Read,
                    ResourceType::PlaybookTargetSet,
                    Some(&id.to_string()),
                )
                .await
                .is_err()
                {
                    return private();
                }
                continue;
            }
            let inventory_blocker = [
                "circuits_",
                "dcim_",
                "ipam_",
                "tenancy_",
                "virtualization_",
                "vpn_",
                "wireless_",
                "inventory_",
            ]
            .iter()
            .any(|prefix| group.table.starts_with(*prefix))
                || CONTENT_TYPES.iter().any(|definition| {
                    definition.app_label == "users" && definition.table == group.table.as_str()
                });
            if !inventory_blocker {
                // Unknown FK blocker domains must not inherit Inventory READ.
                return private();
            }
            // Identifying blockers requires READ on the blocker's persisted
            // attributes, not a bare resource-pattern grant. This is what makes
            // an attribute DENY on a hidden template (or any other typed
            // blocker) suppress its id and label.
            let Some(attributes) = blocker_attributes(state, &group.table, id).await else {
                return private();
            };
            if state
                .services
                .policy_evaluation
                .ensure_access_with_attributes(
                    context,
                    Action::Read,
                    ResourceType::Inventory,
                    Some(&format!("inventory:{id}")),
                    Some(attributes),
                )
                .await
                .is_err()
            {
                return private();
            }
        }
    }
    AppError::DeleteBlocked(details)
}

/// Load the persisted attributes of a blocker row for attribute-aware READ
/// evaluation. Returns `None` when the table is not a registered canonical
/// content type, is not a closed-world identifier, or the row cannot be read;
/// blocker identity is then withheld.
async fn blocker_attributes(
    state: &crate::app::AppState,
    table: &str,
    id: uuid::Uuid,
) -> Option<std::collections::HashMap<String, serde_json::Value>> {
    if !table
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    let definition = crate::contexts::inventory::domain::content_type::CONTENT_TYPES
        .iter()
        .find(|definition| definition.table == table)?;
    let object_type = definition.token();
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT to_jsonb(t) FROM ");
    query
        .push(definition.table)
        .push(" t WHERE t.id=")
        .push_bind(id);
    let value: Option<serde_json::Value> = query
        .build_query_scalar()
        .fetch_optional(&state.db_pool)
        .await
        .ok()
        .flatten();
    let mut value = value?;
    value["object_type"] = serde_json::Value::String(object_type);
    serde_json::from_value(value).ok()
}

fn blocker_object_type(table: &str) -> Option<String> {
    use crate::contexts::inventory::domain::content_type::CONTENT_TYPES;
    CONTENT_TYPES
        .iter()
        .find(|definition| definition.table == table)
        .map(|definition| definition.token())
}

/// A blocker group may be projected only when its examples are the complete
/// counted set. Counts above the example cap, duplicate-collapsed examples, or
/// an empty group are all incomplete projections.
fn group_is_fully_exampled(group: &crate::errors::DeleteBlockedGroup) -> bool {
    !group.examples.is_empty() && group.count == group.examples.len() as i64
}

/// Extracts the request-scoped [`RequestContext`] as a single handler argument.
///
/// Bundles the request id, auth context and headers so a handler that also needs
/// a body extractor stays within the argument budget. `#[utoipa::path]` does not
/// treat it as a parameter, so the OpenAPI parameter surface is unchanged.
pub struct RequestContextExtractor(pub RequestContext);

impl<S> FromRequestParts<S> for RequestContextExtractor
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let request_id = parts
            .extensions
            .get::<RequestId>()
            .cloned()
            .ok_or_else(|| {
                handle_app_error(AppError::Internal(
                    "RequestId extension missing".to_string(),
                ))
            })?;
        let auth_context = parts
            .extensions
            .get::<AuthContext>()
            .cloned()
            .map(Extension);
        let headers = parts.headers.clone();
        Ok(Self(build_request_context(
            &request_id,
            auth_context,
            &headers,
        )))
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{blocker_object_type, build_request_context, group_is_fully_exampled};
    use crate::api::middleware::request_id::RequestId;

    fn blocker_group(count: i64, examples: usize) -> crate::errors::DeleteBlockedGroup {
        crate::errors::DeleteBlockedGroup {
            table: "dcim_consoleporttemplate".to_string(),
            entity_key: None,
            count,
            examples: (0..examples)
                .map(|_| crate::errors::DeleteBlockedExample {
                    id: Some(uuid::Uuid::now_v7()),
                    label: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn blocker_group_requires_the_complete_counted_example_set() {
        assert!(
            !group_is_fully_exampled(&blocker_group(4, 3)),
            "a capped example list must not authorize a larger aggregate"
        );
        assert!(
            group_is_fully_exampled(&blocker_group(3, 3)),
            "a complete small group remains useful"
        );
        assert!(!group_is_fully_exampled(&blocker_group(0, 0)));
        assert!(!group_is_fully_exampled(&blocker_group(2, 3)));
    }

    #[test]
    fn blocker_object_type_requires_a_registered_canonical_token() {
        assert_eq!(
            blocker_object_type("dcim_poweroutlettemplate").as_deref(),
            Some("dcim.poweroutlettemplate")
        );
        assert_eq!(
            blocker_object_type("dcim_inventoryitemtemplate").as_deref(),
            Some("dcim.inventoryitemtemplate")
        );
        assert_eq!(
            blocker_object_type("users_owner").as_deref(),
            Some("users.owner")
        );
        // No seeded canonical content-type token exists for the nested mapping
        // relation, so its blockers are withheld rather than given an invented
        // `object_type`.
        assert_eq!(blocker_object_type("dcim_porttemplatemapping"), None);
        assert_eq!(blocker_object_type("dcim_unregistered_relation"), None);
    }

    #[test]
    fn build_request_context_uses_proxy_headers_for_request_ip() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.25, 172.18.0.6"),
        );

        let request_context = build_request_context(&RequestId("rid".to_string()), None, &headers);

        assert_eq!(request_context.request_ip.as_deref(), Some("198.51.100.25"));
    }

    #[test]
    fn build_request_context_prefers_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.8"));
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.25, 172.18.0.6"),
        );

        let request_context = build_request_context(&RequestId("rid".to_string()), None, &headers);

        assert_eq!(request_context.request_ip.as_deref(), Some("203.0.113.8"));
    }
}
