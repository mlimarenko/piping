use serde_json::Value;
use uuid::Uuid;

use crate::contexts::access_control::domain::{
    Action, Policy, PolicyEffect, ResourceType,
    conditions::{evaluate_constraints, parse_constraints},
    matches_resource_pattern,
};

/// Policy evaluation decision result.
///
/// Policies are evaluated using deny-override semantics:
/// - If any policy denies, the decision is Deny
/// - Otherwise, if any policy allows, the decision is Allow
/// - If no policies match, the decision is Deny (default deny)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Access is allowed
    Allow,
    /// Access is denied
    Deny,
}

/// Detailed metadata about policy evaluation result.
///
/// Contains information about which policies matched and why,
/// useful for debugging and audit logging.
#[derive(Debug, Clone)]
pub struct PolicyDecisionMetadata {
    /// Final decision (Allow or Deny)
    pub decision: PolicyDecision,
    /// IDs of policies that matched with Allow effect
    pub matched_allow: Vec<Uuid>,
    /// IDs of policies that matched with Deny effect
    pub matched_deny: Vec<Uuid>,
    /// Human-readable reasons for the decision
    pub reasons: Vec<String>,
}

/// Evaluates policies and returns a simple Allow/Deny decision.
///
/// This is a convenience wrapper around `evaluate_policies_with_metadata`
/// that discards metadata and only returns the decision.
pub fn evaluate_policies(
    policies: &[Policy],
    action: Action,
    resource_type: ResourceType,
    resource_id: Option<&str>,
) -> PolicyDecision {
    evaluate_policies_with_metadata(policies, action, resource_type, resource_id, &Value::Null)
        .decision
}

/// Evaluates policies against an action and resource type with context.
///
/// Evaluation process:
/// 1. Filters policies that match the action and resource type
/// 2. Applies resource id matching (if provided and policy is scoped)
/// 3. Evaluates policy constraints against the provided context (if any)
/// 4. Collects matching policies by effect (Allow or Deny)
/// 5. Applies deny-override semantics: Deny takes precedence over Allow
/// 6. Returns decision with metadata about matched policies
///
/// Decision logic:
/// - If any policy denies → Deny
/// - Else if any policy allows → Allow
/// - Else (no policies match) → Deny (default deny)
///
/// The context parameter allows policies to evaluate dynamic conditions
/// (e.g., user attributes, resource properties, time-based rules).
pub fn evaluate_policies_with_metadata(
    policies: &[Policy],
    action: Action,
    resource_type: ResourceType,
    resource_id: Option<&str>,
    context: &Value,
) -> PolicyDecisionMetadata {
    if !super::is_valid_permission_pair(&action, &resource_type) {
        return PolicyDecisionMetadata {
            decision: PolicyDecision::Deny,
            matched_allow: Vec::new(),
            matched_deny: Vec::new(),
            reasons: vec!["invalid action/resource permission pair".to_string()],
        };
    }

    let mut matched_allow = Vec::new();
    let mut matched_deny = Vec::new();
    let mut reasons = Vec::new();

    // Evaluate each policy
    for policy in policies {
        // Skip policies that don't match the action or resource type
        if !policy.actions.contains(&action) || !policy.resource_types.contains(&resource_type) {
            continue;
        }

        // Skip policies that don't match the resource id (if scoped)
        if !policy_resources_match(&policy.resources, resource_id) {
            continue;
        }

        // Evaluate constraints if present (e.g., user attributes, time windows)
        // Constraints are JSON expressions that must evaluate to true for the policy to match
        if let Some(constraints) = &policy.constraints {
            let expression = parse_constraints(constraints);
            if !evaluate_constraints(&expression, context) {
                continue; // Constraints not satisfied, skip this policy
            }
        }

        // Policy matches: record it based on effect
        match policy.effect {
            PolicyEffect::Deny => {
                matched_deny.push(policy.id);
                reasons.push(format!("policy {} matched deny", policy.id));
            }
            PolicyEffect::Allow => {
                matched_allow.push(policy.id);
                reasons.push(format!("policy {} matched allow", policy.id));
            }
        }
    }

    // Apply deny-override semantics: Deny takes precedence
    let decision = if !matched_deny.is_empty() {
        PolicyDecision::Deny
    } else if !matched_allow.is_empty() {
        PolicyDecision::Allow
    } else {
        // No policies matched: default deny
        PolicyDecision::Deny
    };

    PolicyDecisionMetadata {
        decision,
        matched_allow,
        matched_deny,
        reasons,
    }
}

fn policy_resources_match(resources: &[String], resource_id: Option<&str>) -> bool {
    if resources.is_empty() {
        return true;
    }

    let resource_id = match resource_id {
        Some(resource_id) => resource_id,
        None => return true,
    };

    resources
        .iter()
        .any(|pattern| matches_resource_pattern(pattern, resource_id))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    fn policy_with_resources(effect: PolicyEffect, resources: Vec<String>) -> Policy {
        Policy {
            id: Uuid::now_v7(),
            name: "policy".to_string(),
            description: None,
            effect,
            actions: vec![Action::Read],
            resource_types: vec![ResourceType::ConnectionTarget],
            constraints: Some(json!({"attributes": {"env": "prod"}})),
            subjects: Vec::new(),
            resources,
            enabled: true,
        }
    }

    #[test]
    fn evaluates_resource_scoped_policies() {
        let policy =
            policy_with_resources(PolicyEffect::Allow, vec!["connection_target:*".to_string()]);
        let context = json!({"attributes": {"env": "prod"}});
        let decision = evaluate_policies_with_metadata(
            &[policy],
            Action::Read,
            ResourceType::ConnectionTarget,
            Some("connection_target:abc"),
            &context,
        )
        .decision;
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn skips_policy_when_resource_mismatch() {
        let policy = policy_with_resources(
            PolicyEffect::Allow,
            vec!["connection_target:abc".to_string()],
        );
        let context = json!({"attributes": {"env": "prod"}});
        let decision = evaluate_policies_with_metadata(
            &[policy],
            Action::Read,
            ResourceType::ConnectionTarget,
            Some("connection_target:def"),
            &context,
        )
        .decision;
        assert_eq!(decision, PolicyDecision::Deny);
    }
}
