use piping_backend::api::connection_authz_attributes::{
    ConnectionTargetFacets, batch_connection_target_is_allowed, connection_target_authz_attributes,
};
use piping_backend::contexts::access_control::domain::{
    Action, Policy, PolicyDecision, PolicyEffect, ResourceType,
    compile_connection_target_visibility, evaluate_policies_with_metadata,
};
use serde_json::{Value, json};
use uuid::Uuid;

fn policy(effect: PolicyEffect, resources: Vec<String>, constraints: Option<Value>) -> Policy {
    Policy {
        id: Uuid::now_v7(),
        name: "playbook batch authorization test".to_string(),
        description: None,
        effect,
        actions: vec![Action::Read],
        resource_types: vec![ResourceType::ConnectionTarget],
        constraints,
        subjects: Vec::new(),
        resources,
        enabled: true,
    }
}

fn point_lookup_decision(
    policies: &[Policy],
    target_id: Uuid,
    facets: Option<&ConnectionTargetFacets>,
) -> bool {
    let facets = facets.cloned().unwrap_or_default();
    let attributes: serde_json::Map<String, Value> =
        connection_target_authz_attributes(&facets, None)
            .into_iter()
            .collect();
    let context = json!({
        "ip": "10.20.30.40",
        "timestamp": "2026-08-03T12:00:00Z",
        "attributes": attributes,
    });
    evaluate_policies_with_metadata(
        policies,
        Action::Read,
        ResourceType::ConnectionTarget,
        Some(format!("connection_target:{target_id}").as_str()),
        &context,
    )
    .decision
        == PolicyDecision::Allow
}

#[test]
fn batch_decisions_match_point_lookup_for_security_boundaries() {
    let target_id = Uuid::now_v7();
    let allowed_site = Uuid::now_v7();
    let other_site = Uuid::now_v7();
    let allowed_tenant = Uuid::now_v7();
    let other_tenant = Uuid::now_v7();
    let denied_tenant = Uuid::now_v7();
    let request_context = json!({
        "ip": "10.20.30.40",
        "timestamp": "2026-08-03T12:00:00Z",
    });

    let unreadable_policies = Vec::new();
    let unconstrained_allow = vec![policy(PolicyEffect::Allow, vec!["*".to_string()], None)];
    let different_scope_policies = vec![policy(
        PolicyEffect::Allow,
        vec!["*".to_string()],
        Some(json!({"attributes": {
            "site_id": allowed_site.to_string(),
            "tenant_id": allowed_tenant.to_string(),
        }})),
    )];
    let deny_match_policies = vec![
        policy(PolicyEffect::Allow, vec!["*".to_string()], None),
        policy(
            PolicyEffect::Deny,
            vec![format!("connection_target:{target_id}")],
            Some(json!({"attributes": {"tenant_id": denied_tenant.to_string()}})),
        ),
    ];
    let unreadable_facets = ConnectionTargetFacets {
        site_id: Some(other_site),
        ..Default::default()
    };
    let different_scope_facets = ConnectionTargetFacets {
        site_id: Some(other_site),
        tenant_id: Some(other_tenant),
        ..Default::default()
    };
    let deny_match_facets = ConnectionTargetFacets {
        tenant_id: Some(denied_tenant),
        ..Default::default()
    };

    let cases = [
        (
            "target the user may not read",
            unreadable_policies.as_slice(),
            Some(&unreadable_facets),
        ),
        (
            "target that does not exist",
            unconstrained_allow.as_slice(),
            None,
        ),
        (
            "target whose site and tenant differ",
            different_scope_policies.as_slice(),
            Some(&different_scope_facets),
        ),
        (
            "deny-policy match",
            deny_match_policies.as_slice(),
            Some(&deny_match_facets),
        ),
    ];

    for (name, policies, facets) in cases {
        let expected = point_lookup_decision(policies, target_id, facets);
        let predicate = compile_connection_target_visibility(policies, &request_context);
        let actual = batch_connection_target_is_allowed(&predicate, target_id, facets);
        assert_eq!(actual, expected, "authorization changed for {name}");
    }

    assert!(!point_lookup_decision(
        &unreadable_policies,
        target_id,
        Some(&unreadable_facets)
    ));
    assert!(point_lookup_decision(&unconstrained_allow, target_id, None));
    assert!(!point_lookup_decision(
        &different_scope_policies,
        target_id,
        Some(&different_scope_facets)
    ));
    assert!(!point_lookup_decision(
        &deny_match_policies,
        target_id,
        Some(&deny_match_facets)
    ));
}
