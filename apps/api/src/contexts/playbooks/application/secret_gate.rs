//! Pure C3 secret-use authorization core.
//!
//! A playbook version declares the secrets it exports into the runner env via
//! `token_policy.runtime.secrets.env = {ENV_NAME: secret_key}`. Launching such a
//! version requires, IN ADDITION to the composite run grant (`Execute × Playbook`)
//! and the per-host `Read × ConnectionTarget` fan-out, an `Execute × SecretVariable`
//! grant for EVERY declared secret. That extra leg is gate **C3**.
//!
//! This module holds the *pure* per-secret decision so it is unit-testable without
//! a database: given the principal's already-resolved policies, a constraint
//! context, and a PAT-ceiling probe, it returns whether the subject may use one
//! secret. The app-layer C3 orchestrator (`app::playbook_secret_gate`) resolves
//! policies/context/secret metadata once and drives this core per secret, for
//! both the manual REST launch and the scheduled-fire authoring path, so
//! enforcement, preflight, and the authorization snapshot all share one verdict
//! (preflight == launch by construction).
//!
//! Semantics mirror `ensure_playbook_run_grant` exactly:
//! 1. **Deny veto first** over BOTH canonical candidates
//!    (`secret_variable:{uuid}` and `secret_variable:key:{key}`): an explicit Deny
//!    matching EITHER candidate vetoes, even if the other candidate (or a wildcard)
//!    would allow. This is the engine's deny-override extended across the OR.
//! 2. **Allow legs** otherwise: allow when EITHER candidate is allowed, each leg
//!    gated by the PAT token-scope ceiling (`pat_allows`).
//! 3. **Default deny**: no matching allow policy ⇒ denied. A brand-new secret with
//!    no allow policy is runnable by nobody — no explicit deny row required.
//!
//! There is intentionally NO ownership short-circuit here: the playbook owner does
//! not implicitly gain the right to export a secret they lack a grant on. C3 runs
//! unconditionally after the playbook run grant.
//!
//! Fidelity note: the constraint `context` is built once with no `resource.id`
//! (the two candidates differ only in resource id, which the engine matches via
//! `policy.resources` patterns, not via context). Constraints keyed on `ip`,
//! `timestamp`, or subject apply faithfully; a policy whose constraint reads
//! `resource.id` is skipped — fail-closed for an Allow (safe), while Deny scoping
//! remains effective through `resources` patterns.

use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::contexts::access_control::domain::{
    Action, Policy, PolicyDecision, ResourceType, evaluate_policies_with_metadata,
};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

/// The RBAC action that authorizes exporting a secret into a playbook run env.
pub const SECRET_USE_ACTION: Action = Action::Execute;

/// Canonical resource-id candidates for a secret's C3 grant: the UUID leg (for
/// per-secret grants) and the key leg (for group grants via naming-convention
/// `re:` patterns, e.g. `re:secret_variable:key:grad-.*`). We always pass the
/// prefixed forms; a bare (`*` or prefix-less) policy pattern matches both.
pub fn secret_use_candidates(secret_id: Uuid, key: &str) -> [String; 2] {
    [
        format!("secret_variable:{secret_id}"),
        format!("secret_variable:key:{key}"),
    ]
}

/// Pure per-secret C3 verdict. `pat_allows(resource_id)` reports whether the
/// caller's API-token scope ceiling permits `Execute × SecretVariable` on that
/// candidate (always `true` for non-token auth). See module docs for semantics.
pub fn secret_use_allowed(
    policies: &[Policy],
    context: &Value,
    secret_id: Uuid,
    key: &str,
    pat_allows: impl Fn(&str) -> bool,
) -> bool {
    let candidates = secret_use_candidates(secret_id, key);

    // (1) Deny veto: an explicit Deny on EITHER candidate wins over every Allow leg.
    for candidate in &candidates {
        let metadata = evaluate_policies_with_metadata(
            policies,
            SECRET_USE_ACTION,
            ResourceType::SecretVariable,
            Some(candidate.as_str()),
            context,
        );
        if !metadata.matched_deny.is_empty() {
            return false;
        }
    }

    // (2) Allow legs: allow via EITHER candidate, each behind the PAT ceiling.
    for candidate in &candidates {
        if pat_allows(candidate.as_str())
            && evaluate_policies_with_metadata(
                policies,
                SECRET_USE_ACTION,
                ResourceType::SecretVariable,
                Some(candidate.as_str()),
                context,
            )
            .decision
                == PolicyDecision::Allow
        {
            return true;
        }
    }

    false
}

/// PAT token-scope ceiling for `Execute × SecretVariable` on one canonical
/// secret candidate. Session and OIDC callers are not PAT-constrained; both
/// playbook C3 and any other named-secret consumer must use this exact rule.
pub fn secret_use_pat_allows(request_context: &RequestContext, candidate: &str) -> bool {
    !matches!(
        request_context.auth_method.as_str(),
        "api_token" | "playbook_job_token"
    ) || request_context.allows_api_token_scope_for_resource(
        SECRET_USE_ACTION.as_str(),
        ResourceType::SecretVariable.as_str(),
        Some(candidate),
    )
}

/// A declared runtime secret already resolved to its stored row: the env name it
/// is exported as, its key, its id, and whether it is protected.
///
/// Resolution (existence + row load) is the caller's concern; this type is the
/// pure input to [`secret_env_authorization_snapshot`].
pub struct ResolvedDeclaredSecret {
    pub env_name: String,
    pub key: String,
    pub secret_id: Uuid,
    pub is_protected: bool,
}

/// Build the server-authored C3 authorization snapshot for a set of resolved
/// declared secrets, or `None` when nothing is declared.
///
/// This is the ONE canonical implementation shared by the manual REST launch
/// path and the scheduled-run authoring path: for each entry it refuses a
/// protected secret and requires the per-secret `Execute × SecretVariable`
/// verdict from [`secret_use_allowed`], then records
/// `env → {secret_id, key}` (never a value). Byte-identical error strings and
/// JSON keys are preserved so both callers produce the same observable outcome.
///
/// `resolved` yields one fallible entry per declared secret IN DECLARED ORDER:
/// a resolution failure for an entry is returned first, then its protected or
/// grant refusal, so the first failure matches the manual path's interleaved
/// `load → protected → grant` order. An empty iterator yields `None`.
///
/// `pat_allows(resource_id)` is the caller's PAT token-scope ceiling probe:
/// session/OIDC callers pass [`secret_use_pat_allows`]; the service-principal
/// path has no token and passes an always-true probe.
pub fn secret_env_authorization_snapshot(
    policies: &[Policy],
    context: &Value,
    subject: &str,
    resolved: impl IntoIterator<Item = Result<ResolvedDeclaredSecret, AppError>>,
    pat_allows: impl Fn(&str) -> bool,
) -> Result<Option<Value>, AppError> {
    let resolved: Vec<Result<ResolvedDeclaredSecret, AppError>> = resolved.into_iter().collect();
    if resolved.is_empty() {
        return Ok(None);
    }

    let mut env_snapshot = serde_json::Map::new();
    for entry in resolved {
        let entry = entry?;
        if entry.is_protected {
            return Err(AppError::Forbidden(format!(
                "Playbook runtime secret `{}` (env {}) is protected and cannot be exported to a run environment",
                entry.key, entry.env_name
            )));
        }
        let allowed =
            secret_use_allowed(policies, context, entry.secret_id, &entry.key, &pat_allows);
        if !allowed {
            return Err(AppError::Forbidden(format!(
                "Playbook requires secret `{}` (env {}); missing Execute grant on secret_variable",
                entry.key, entry.env_name
            )));
        }
        env_snapshot.insert(
            entry.env_name.clone(),
            json!({ "secret_id": entry.secret_id.to_string(), "key": entry.key }),
        );
    }

    Ok(Some(json!({
        "decided_at": Utc::now().to_rfc3339(),
        "subject": subject,
        "env": Value::Object(env_snapshot),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::access_control::domain::PolicyEffect;
    use crate::contexts::shared::request_context::{
        ApiTokenScopeContext, RequestAuth, RequestContext,
    };
    use serde_json::json;

    fn policy(effect: PolicyEffect, resources: Vec<&str>) -> Policy {
        Policy {
            id: Uuid::now_v7(),
            name: "policy".to_string(),
            description: None,
            effect,
            actions: vec![Action::Execute],
            resource_types: vec![ResourceType::SecretVariable],
            constraints: None,
            subjects: Vec::new(),
            resources: resources.into_iter().map(str::to_string).collect(),
            enabled: true,
        }
    }

    fn allow_all() -> impl Fn(&str) -> bool {
        |_candidate: &str| true
    }

    fn request_context(
        auth_method: &str,
        api_token_scopes: Vec<ApiTokenScopeContext>,
    ) -> RequestContext {
        RequestContext::new(
            Some(Uuid::nil()),
            auth_method.to_string(),
            None,
            "test-correlation".to_string(),
            RequestAuth {
                api_token_scopes,
                ..RequestAuth::default()
            },
        )
    }

    const SECRET_ID: &str = "01890000-0000-7000-8000-000000000001";

    fn secret_id() -> Uuid {
        Uuid::parse_str(SECRET_ID).unwrap()
    }

    #[test]
    fn candidates_are_the_prefixed_uuid_and_key_legs() {
        let candidates = secret_use_candidates(secret_id(), "grad-token");
        assert_eq!(candidates[0], format!("secret_variable:{SECRET_ID}"));
        assert_eq!(candidates[1], "secret_variable:key:grad-token");
    }

    #[test]
    fn default_deny_when_no_policy_grants_the_secret() {
        assert!(!secret_use_allowed(
            &[],
            &Value::Null,
            secret_id(),
            "grad-token",
            allow_all(),
        ));
    }

    #[test]
    fn allows_via_id_leg_grant() {
        let policies = vec![policy(
            PolicyEffect::Allow,
            vec![&format!("secret_variable:{SECRET_ID}")],
        )];
        assert!(secret_use_allowed(
            &policies,
            &Value::Null,
            secret_id(),
            "grad-token",
            allow_all(),
        ));
    }

    #[test]
    fn allows_via_key_leg_group_pattern() {
        // A `re:` key-leg pattern grants by naming-convention without any per-id
        // grant; the UUID leg does NOT match this pattern.
        let policies = vec![policy(
            PolicyEffect::Allow,
            vec!["re:secret_variable:key:grad-.*"],
        )];
        assert!(secret_use_allowed(
            &policies,
            &Value::Null,
            secret_id(),
            "grad-token",
            allow_all(),
        ));
        // A key outside the pattern is still denied.
        assert!(!secret_use_allowed(
            &policies,
            &Value::Null,
            secret_id(),
            "other-token",
            allow_all(),
        ));
    }

    #[test]
    fn allows_via_wildcard_general_policy() {
        let policies = vec![policy(PolicyEffect::Allow, vec!["*"])];
        assert!(secret_use_allowed(
            &policies,
            &Value::Null,
            secret_id(),
            "grad-token",
            allow_all(),
        ));
    }

    #[test]
    fn deny_override_on_id_leg_beats_wildcard_allow() {
        let policies = vec![
            policy(PolicyEffect::Allow, vec!["*"]),
            policy(
                PolicyEffect::Deny,
                vec![&format!("secret_variable:{SECRET_ID}")],
            ),
        ];
        assert!(!secret_use_allowed(
            &policies,
            &Value::Null,
            secret_id(),
            "grad-token",
            allow_all(),
        ));
    }

    #[test]
    fn deny_override_on_key_leg_vetoes_id_leg_allow() {
        // Deny on the key candidate must veto even when the id leg would allow.
        let policies = vec![
            policy(
                PolicyEffect::Allow,
                vec![&format!("secret_variable:{SECRET_ID}")],
            ),
            policy(PolicyEffect::Deny, vec!["re:secret_variable:key:grad-.*"]),
        ];
        assert!(!secret_use_allowed(
            &policies,
            &Value::Null,
            secret_id(),
            "grad-token",
            allow_all(),
        ));
    }

    #[test]
    fn pat_ceiling_denies_when_token_scope_missing_despite_policy_allow() {
        let policies = vec![policy(PolicyEffect::Allow, vec!["*"])];
        // Token scope permits nothing → denied even with a valid user policy.
        assert!(!secret_use_allowed(
            &policies,
            &Value::Null,
            secret_id(),
            "grad-token",
            |_candidate: &str| false,
        ));
    }

    #[test]
    fn pat_ceiling_allows_when_token_scope_present_and_policy_allows() {
        let policies = vec![policy(PolicyEffect::Allow, vec!["*"])];
        assert!(secret_use_allowed(
            &policies,
            &json!({ "auth_method": "api_token" }),
            secret_id(),
            "grad-token",
            allow_all(),
        ));
    }

    #[test]
    fn secret_use_pat_ceiling_is_shared_by_session_and_api_token_callers() {
        let candidate = format!("secret_variable:{SECRET_ID}");
        assert!(secret_use_pat_allows(
            &request_context("session", Vec::new()),
            &candidate,
        ));
        assert!(!secret_use_pat_allows(
            &request_context("api_token", Vec::new()),
            &candidate,
        ));
        assert!(secret_use_pat_allows(
            &request_context(
                "api_token",
                vec![ApiTokenScopeContext {
                    action: SECRET_USE_ACTION.as_str().to_string(),
                    resource_type: ResourceType::SecretVariable.as_str().to_string(),
                    resource_pattern: candidate.clone(),
                }],
            ),
            &candidate,
        ));
    }

    fn declared(env_name: &str, key: &str, is_protected: bool) -> ResolvedDeclaredSecret {
        ResolvedDeclaredSecret {
            env_name: env_name.to_string(),
            key: key.to_string(),
            secret_id: secret_id(),
            is_protected,
        }
    }

    #[test]
    fn snapshot_is_none_when_nothing_is_declared() {
        let snapshot =
            secret_env_authorization_snapshot(&[], &Value::Null, "user:x", vec![], allow_all())
                .expect("empty declaration is not an error");
        assert!(snapshot.is_none());
    }

    #[test]
    fn snapshot_records_env_secret_id_and_key_when_allowed() {
        let policies = vec![policy(PolicyEffect::Allow, vec!["*"])];
        let snapshot = secret_env_authorization_snapshot(
            &policies,
            &Value::Null,
            "user:01890000-0000-7000-8000-0000000000aa",
            vec![Ok(declared("GRAD_TOKEN", "grad-token", false))],
            allow_all(),
        )
        .expect("allowed secret must produce a snapshot")
        .expect("non-empty declaration produces Some");
        assert_eq!(
            snapshot["subject"],
            "user:01890000-0000-7000-8000-0000000000aa"
        );
        assert_eq!(snapshot["env"]["GRAD_TOKEN"]["key"], "grad-token");
        assert_eq!(snapshot["env"]["GRAD_TOKEN"]["secret_id"], SECRET_ID);
        let keys: std::collections::BTreeSet<&str> = snapshot
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["decided_at", "subject", "env"].into_iter().collect());
    }

    #[test]
    fn snapshot_denies_a_declared_secret_without_a_grant() {
        let error = secret_env_authorization_snapshot(
            &[],
            &Value::Null,
            "user:x",
            vec![Ok(declared("GRAD_TOKEN", "grad-token", false))],
            allow_all(),
        )
        .expect_err("default deny must refuse");
        assert!(
            matches!(error, AppError::Forbidden(message) if message.contains("missing Execute grant on secret_variable"))
        );
    }

    #[test]
    fn snapshot_resolution_error_does_not_mask_an_earlier_missing_grant() {
        // Declared order is authoritative: the first entry's missing grant is
        // refused before the second entry's resolution error is inspected,
        // matching the manual path's interleaved `load → protected → grant`.
        let error = secret_env_authorization_snapshot(
            &[],
            &Value::Null,
            "user:x",
            vec![
                Ok(declared("GRAD_TOKEN", "grad-token", false)),
                Err(AppError::Validation(
                    "Playbook declares runtime secret `Z_TOKEN_KEY` (env Z_TOKEN) that does not exist"
                        .to_string(),
                )),
            ],
            allow_all(),
        )
        .expect_err("the first declared entry's missing grant must win");
        assert!(
            matches!(error, AppError::Forbidden(message) if message.contains("missing Execute grant on secret_variable"))
        );
    }

    #[test]
    fn snapshot_refuses_a_protected_declared_secret() {
        let policies = vec![policy(PolicyEffect::Allow, vec!["*"])];
        let error = secret_env_authorization_snapshot(
            &policies,
            &Value::Null,
            "user:x",
            vec![Ok(declared("GRAD_TOKEN", "grad-token", true))],
            allow_all(),
        )
        .expect_err("protected secret must be refused");
        assert!(
            matches!(error, AppError::Forbidden(message) if message.contains("is protected and cannot be exported"))
        );
    }

    #[test]
    fn snapshot_honours_the_pat_ceiling() {
        let policies = vec![policy(PolicyEffect::Allow, vec!["*"])];
        let error = secret_env_authorization_snapshot(
            &policies,
            &Value::Null,
            "user:x",
            vec![Ok(declared("GRAD_TOKEN", "grad-token", false))],
            |_candidate: &str| false,
        )
        .expect_err("a token-scope ceiling that denies must refuse");
        assert!(matches!(error, AppError::Forbidden(_)));
    }
}
