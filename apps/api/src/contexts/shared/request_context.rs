use crate::contexts::access_control::domain::matches_resource_pattern;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiTokenScopeContext {
    pub action: String,
    pub resource_type: String,
    pub resource_pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiTokenScopeResourceOverride {
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
}

/// Authentication material attached to a request context.
///
/// Groups the per-request authentication inputs that are not part of the
/// request identity itself: OIDC group claims and personal-API-token scopes.
#[derive(Debug, Clone, Default)]
pub struct RequestAuth {
    /// OIDC group names from the identity provider (if authenticated via OIDC).
    ///
    /// Extracted from OIDC userinfo endpoint's additional claims and retained
    /// for audit/session metadata. RBAC does not authorize directly from these
    /// request-time claims; OIDC login materializes mapped groups into durable
    /// identity group membership first.
    pub oidc_groups: Vec<String>,

    /// API-token scopes when request is authenticated by personal API token.
    ///
    /// Empty for session and anonymous flows.
    pub api_token_scopes: Vec<ApiTokenScopeContext>,

    /// Internal one-request resource id used when a legacy coarse REST handler
    /// cannot name the resource itself but the caller already validated a
    /// resource-specific token scope before dispatch.
    pub api_token_scope_resource_override: Option<ApiTokenScopeResourceOverride>,
}

/// Request context containing per-request metadata for tracing, audit, and access control.
///
/// This context is created for each HTTP request and passed through the application.
/// It contains information about the authenticated user, request origin, and correlation ID
/// for distributed tracing and audit logging.
///
/// The context is extracted from middleware (auth, request ID) and made available to
/// REST handlers, WebSocket handlers, and service layers.
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// Authenticated user ID if the request is authenticated.
    ///
    /// `None` for unauthenticated requests (public endpoints).
    /// Used for access control, audit logging, and user-specific operations.
    pub user_id: Option<Uuid>,

    /// Authentication method used for this request.
    ///
    /// Possible values:
    /// - `"session"`: Authenticated via session cookie (local or OIDC)
    /// - `"oidc"`: Authenticated via OIDC (stored in session metadata)
    /// - `"none"`: Not authenticated (public request)
    ///
    /// Used for audit logging and security analysis.
    pub auth_method: String,

    /// Client IP address extracted from request headers.
    ///
    /// Supports proxy headers: `x-forwarded-for` (first IP) or `x-real-ip`.
    /// `None` if IP cannot be determined.
    /// Used for rate limiting, security analysis, and audit logging.
    pub request_ip: Option<String>,

    /// Request correlation ID (UUIDv7) for distributed tracing.
    ///
    /// Generated once per request and included in all logs and traces.
    /// Allows correlating logs across services and request lifecycle.
    /// Format: UUIDv7 (time-ordered UUID for better database indexing).
    pub correlation_id: String,

    /// OIDC group names from the identity provider (if authenticated via OIDC).
    ///
    /// Extracted from OIDC userinfo endpoint's additional claims and retained
    /// for audit/session metadata. RBAC does not authorize directly from these
    /// request-time claims; OIDC login materializes mapped groups into durable
    /// identity group membership first.
    pub oidc_groups: Vec<String>,

    /// API-token scopes when request is authenticated by personal API token.
    ///
    /// Empty for session and anonymous flows.
    pub api_token_scopes: Vec<ApiTokenScopeContext>,

    /// Personal API token id when the request is authenticated by PAT.
    ///
    /// `None` for session, OIDC, job-token, and anonymous flows. This is an
    /// opaque id only; raw token material never enters request context.
    pub api_token_id: Option<String>,

    /// Internal one-request resource id used when a legacy coarse REST handler
    /// cannot name the resource itself but the caller already validated a
    /// resource-specific token scope before dispatch.
    pub api_token_scope_resource_override: Option<ApiTokenScopeResourceOverride>,
}

impl RequestContext {
    /// Creates a new request context with all metadata.
    ///
    /// Typically called from middleware (auth, request ID) after extracting
    /// authentication and request information.
    pub fn new(
        user_id: Option<Uuid>,
        auth_method: String,
        request_ip: Option<String>,
        correlation_id: String,
        auth: RequestAuth,
    ) -> Self {
        Self {
            user_id,
            auth_method,
            request_ip,
            correlation_id,
            oidc_groups: auth.oidc_groups,
            api_token_scopes: auth.api_token_scopes,
            api_token_id: None,
            api_token_scope_resource_override: auth.api_token_scope_resource_override,
        }
    }

    /// The authenticated user's id, or `AppError::Auth` (401) when the request
    /// carries no session/token identity. The single canonical helper every
    /// user-scoped route uses — it replaced ~10 copy-pasted private versions,
    /// one of which had drifted to a 403 (`Forbidden`) for an unauthenticated
    /// request.
    pub fn require_authenticated_user_id(&self) -> Result<Uuid, crate::errors::AppError> {
        self.user_id
            .ok_or_else(|| crate::errors::AppError::Auth("Authentication required".to_string()))
    }

    /// Best-effort audit actor channel derived from the authentication method.
    ///
    /// Maps the request's channel to the canonical `actor_kind` recorded on
    /// audit events: a personal API token is `api_token`, a playbook job token
    /// is `playbook_job_token`, and everything else (session, OIDC, anonymous)
    /// is an interactive `user`.
    ///
    /// This is only for request-scoped emit sites that would otherwise hardcode
    /// `"user"`. Emit sites that already know a more specific kind (for example
    /// `mcp_agent` on the MCP dispatch anchor) must keep their explicit value —
    /// do not overwrite it with this default. A downstream effect reached via an
    /// MCP re-dispatch carries `auth_method = "api_token"`, so it is correctly
    /// attributed to `api_token`; only the top-level MCP event stays `mcp_agent`.
    #[must_use]
    pub fn audit_actor_kind(&self) -> &'static str {
        if self.api_token_id.is_some() {
            return "api_token";
        }
        match self.auth_method.as_str() {
            "api_token" => "api_token",
            "playbook_job_token" => "playbook_job_token",
            _ => "user",
        }
    }

    pub fn allows_api_token_scope(&self, action: &str, resource_type: &str) -> bool {
        self.allows_api_token_scope_for_resource(action, resource_type, None)
    }

    pub fn allows_api_token_scope_for_resource(
        &self,
        action: &str,
        resource_type: &str,
        resource_id: Option<&str>,
    ) -> bool {
        let effective_resource_id = resource_id.or_else(|| {
            self.api_token_scope_resource_override
                .as_ref()
                .filter(|value| value.action == action && value.resource_type == resource_type)
                .map(|value| value.resource_id.as_str())
        });
        self.api_token_scopes.iter().any(|scope| {
            scope.action == action
                && scope.resource_type == resource_type
                && scope_pattern_allows_resource(
                    scope.resource_pattern.as_str(),
                    effective_resource_id,
                )
        })
    }

    pub fn has_api_token_scope(&self, action: &str, resource_type: &str) -> bool {
        self.api_token_scopes
            .iter()
            .any(|scope| scope.action == action && scope.resource_type == resource_type)
    }
}

fn scope_pattern_allows_resource(pattern: &str, resource_id: Option<&str>) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == "*" {
        return true;
    }

    let Some(resource_id) = resource_id else {
        return false;
    };
    matches_resource_pattern(pattern, resource_id)
}

#[cfg(test)]
mod tests {
    use super::{ApiTokenScopeContext, ApiTokenScopeResourceOverride, RequestAuth, RequestContext};
    use uuid::Uuid;

    fn context(scope: ApiTokenScopeContext) -> RequestContext {
        RequestContext::new(
            Some(Uuid::nil()),
            "api_token".to_string(),
            None,
            "request".to_string(),
            RequestAuth {
                api_token_scopes: vec![scope],
                ..RequestAuth::default()
            },
        )
    }

    #[test]
    fn wildcard_api_token_scope_allows_broad_and_specific_access() {
        let request_context = context(ApiTokenScopeContext {
            action: "read".to_string(),
            resource_type: "inventory".to_string(),
            resource_pattern: "*".to_string(),
        });

        assert!(request_context.allows_api_token_scope("read", "inventory"));
        assert!(request_context.allows_api_token_scope_for_resource(
            "read",
            "inventory",
            Some("device:dev-1")
        ));
    }

    #[test]
    fn resource_scoped_api_token_scope_rejects_broad_access() {
        let request_context = context(ApiTokenScopeContext {
            action: "read".to_string(),
            resource_type: "inventory".to_string(),
            resource_pattern: "device:dev-1".to_string(),
        });

        assert!(!request_context.allows_api_token_scope("read", "inventory"));
        assert!(request_context.allows_api_token_scope_for_resource(
            "read",
            "inventory",
            Some("device:dev-1")
        ));
        assert!(!request_context.allows_api_token_scope_for_resource(
            "read",
            "inventory",
            Some("device:dev-2")
        ));
    }

    #[test]
    fn resource_scoped_api_token_scope_matches_unprefixed_patterns() {
        let request_context = context(ApiTokenScopeContext {
            action: "connect".to_string(),
            resource_type: "connection_target".to_string(),
            resource_pattern: "target-42".to_string(),
        });

        assert!(request_context.allows_api_token_scope_for_resource(
            "connect",
            "connection_target",
            Some("connection_target:target-42")
        ));
    }

    #[test]
    fn resource_scoped_api_token_scope_matches_prefixed_connection_target_patterns() {
        let request_context = context(ApiTokenScopeContext {
            action: "read".to_string(),
            resource_type: "connection_target".to_string(),
            resource_pattern: "connection_target:target-42".to_string(),
        });

        assert!(request_context.allows_api_token_scope_for_resource(
            "read",
            "connection_target",
            Some("connection_target:target-42")
        ));
        assert!(!request_context.allows_api_token_scope_for_resource(
            "read",
            "connection_target",
            Some("connection_target:connection_target-43")
        ));
    }

    #[test]
    fn internal_resource_override_allows_broad_checks_without_wildcard_scope() {
        let request_context = RequestContext::new(
            Some(Uuid::nil()),
            "api_token".to_string(),
            None,
            "request".to_string(),
            RequestAuth {
                api_token_scopes: vec![ApiTokenScopeContext {
                    action: "create".to_string(),
                    resource_type: "inventory".to_string(),
                    resource_pattern: "inventory_entity:devices".to_string(),
                }],
                api_token_scope_resource_override: Some(ApiTokenScopeResourceOverride {
                    action: "create".to_string(),
                    resource_type: "inventory".to_string(),
                    resource_id: "inventory_entity:devices".to_string(),
                }),
                ..RequestAuth::default()
            },
        );

        assert!(request_context.allows_api_token_scope("create", "inventory"));
        assert!(!request_context.allows_api_token_scope("update", "inventory"));
        assert!(!request_context.allows_api_token_scope_for_resource(
            "create",
            "inventory",
            Some("inventory_entity:sites")
        ));
    }
}
