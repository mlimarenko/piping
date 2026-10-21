//! Helpers for mapping request intents to policy actions, plus the single
//! authorization entry point every route handler funnels through.

use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

/// Enforce an access-control policy for `request_context` against a resource.
///
/// This is the one place route handlers reach the policy engine, so the
/// authorization contract lives in a single spot rather than being copy-pasted
/// into every route module. It reads the cached [`AppState::services`] policy
/// evaluator directly — do not route this through the `build_*` alias getters,
/// which merely clone the same `Arc`.
///
/// Pass `resource_id` when the policy is scoped to a specific object; use
/// [`ensure_access`] when it is not.
pub(crate) async fn ensure_access_with_resource(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_type: ResourceType,
    resource_id: Option<&str>,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_access(request_context, action, resource_type, resource_id)
        .await
}

/// Enforce an access-control policy that is not scoped to a specific object id.
///
/// Thin convenience over [`ensure_access_with_resource`] with `resource_id` set
/// to `None`.
pub(crate) async fn ensure_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
    resource_type: ResourceType,
) -> Result<(), AppError> {
    ensure_access_with_resource(state, request_context, action, resource_type, None).await
}

/// Enforce collection-wide inventory access without promoting object grants.
pub(crate) async fn ensure_inventory_collection_access(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    state
        .services
        .policy_evaluation
        .ensure_inventory_collection_access(request_context, action)
        .await
}

/// Resolve secret field permission action by intent.
///
/// - Edit intents (`set`, `rotate`, `clear`, `convert_to_plain`) require `UPDATE`.
/// - Reveal intent (`reveal`) requires `SECRET`.
pub fn secret_field_action_for_intent(intent: &str) -> Option<Action> {
    let normalized = intent.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "set" | "rotate" | "clear" | "convert_to_plain" => Some(Action::Update),
        "reveal" => Some(Action::Secret),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::secret_field_action_for_intent;
    use crate::contexts::access_control::domain::Action;

    #[test]
    fn maps_secret_field_edit_intents_to_update_action() {
        for intent in ["set", "rotate", "clear", "convert_to_plain"] {
            assert_eq!(secret_field_action_for_intent(intent), Some(Action::Update));
        }
    }

    #[test]
    fn maps_secret_field_reveal_intent_to_secret_action() {
        assert_eq!(
            secret_field_action_for_intent("reveal"),
            Some(Action::Secret)
        );
    }

    #[test]
    fn rejects_unknown_secret_field_intent() {
        assert_eq!(secret_field_action_for_intent("unknown"), None);
    }
}
