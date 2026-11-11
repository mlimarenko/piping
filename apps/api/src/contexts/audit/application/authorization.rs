use serde_json::Value;
use uuid::Uuid;

use crate::app::AppState;
use crate::contexts::access_control::domain::conditions::{
    Condition, ConditionExpression, evaluate_constraints, parse_constraints,
};
use crate::contexts::access_control::domain::{
    Action, PolicyEffect, ResourceType, evaluate_policies_with_metadata,
};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

fn global_pattern(pattern: &str) -> bool {
    matches!(pattern, "*" | "audit_event:*")
}

// Missing event attributes are unknown, not false. Request-only conditions are
// evaluated by the canonical evaluator; AND/OR propagate possible matches.
fn deny_may_match(expression: &ConditionExpression, context: &Value) -> bool {
    match expression {
        ConditionExpression::And(items) => items.iter().all(|item| deny_may_match(item, context)),
        ConditionExpression::Or(items) => items.iter().any(|item| deny_may_match(item, context)),
        ConditionExpression::Condition(Condition::Attributes { attributes }) => {
            attributes.iter().all(|(key, expected)| {
                context
                    .get("attributes")
                    .and_then(|value| value.get(key))
                    .is_none_or(|actual| actual == expected)
            })
        }
        ConditionExpression::Condition(Condition::Unknown(_)) => true,
        ConditionExpression::Condition(_) => evaluate_constraints(expression, context),
    }
}

/// Authorize an unfiltered audit collection, including its counts and lookups.
/// Scoped denies that may match any event block the collection. Request-only
/// constraints retain their native semantics; unavailable object attributes in
/// denies are conservatively treated as potentially matching.
///
/// # Errors
/// Returns an authorization or policy-resolution error when access is denied.
pub async fn ensure_audit_collection_access(
    state: &AppState,
    request_context: &RequestContext,
) -> Result<(), AppError> {
    if request_context.auth_method == "playbook_job_token"
        || (request_context.auth_method == "api_token"
            && !request_context.api_token_scopes.iter().any(|scope| {
                scope.action == Action::ViewAudit.as_str()
                    && scope.resource_type == ResourceType::AuditEvent.as_str()
                    && (scope.resource_pattern.trim().is_empty()
                        || global_pattern(scope.resource_pattern.trim()))
            }))
    {
        return Err(AppError::Forbidden(
            "Global audit token scope required".into(),
        ));
    }

    let service = &state.services.access_control;
    let policies = service
        .resolve_policies_for_request(request_context)
        .await?;
    let context =
        service.constraint_context_for_request(request_context, ResourceType::AuditEvent, None)?;
    let trace = evaluate_policies_with_metadata(
        &policies,
        Action::ViewAudit,
        ResourceType::AuditEvent,
        None,
        &context,
    );
    let relevant = |policy: &&crate::contexts::access_control::domain::Policy| {
        policy.enabled
            && policy.actions.contains(&Action::ViewAudit)
            && policy.resource_types.contains(&ResourceType::AuditEvent)
    };
    if policies.iter().filter(relevant).any(|policy| {
        policy.effect == PolicyEffect::Deny
            && policy
                .constraints
                .as_ref()
                .is_none_or(|constraints| deny_may_match(&parse_constraints(constraints), &context))
    }) {
        return Err(AppError::Forbidden(
            "Audit collection denied by policy".into(),
        ));
    }
    if policies.iter().filter(relevant).any(|policy| {
        policy.effect == PolicyEffect::Allow
            && trace.matched_allow.contains(&policy.id)
            && (policy.resources.is_empty() || policy.resources.iter().any(|r| global_pattern(r)))
    }) {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "Global audit permission required".into(),
        ))
    }
}

/// Authorize one actual audit event before fetching it.
///
/// # Errors
/// Returns the native policy or token-scope denial for this event.
pub async fn ensure_audit_event_access(
    state: &AppState,
    request_context: &RequestContext,
    event_id: Uuid,
) -> Result<(), AppError> {
    let resource = format!("audit_event:{event_id}");
    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            Action::ViewAudit,
            ResourceType::AuditEvent,
            Some(&resource),
        )
        .await
}
