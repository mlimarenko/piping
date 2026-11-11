//! Row-level connection-target visibility gate for list and host-scoped detail
//! endpoints.
//!
//! Resolves the caller's policies ONCE and compiles their
//! `Read × CONNECTION_TARGET` grants into a [`VisibilityPredicate`] the read-
//! model applies in SQL, so a list shows only the rows the policy engine would
//! allow. This is the row-level companion to the per-target action
//! gating: the same attribute constraints (site/role/tenant/…) that scope a
//! connect now also scope which hosts appear in a list.

use uuid::Uuid;

use crate::api::access_control_helpers::build_access_control_service;
use crate::api::connection_authz_attributes::ConnectionTargetFacets;
use crate::app::AppState;
use crate::app::live_launch_principal;
use crate::contexts::access_control::application::access_control_service::AccessControlService;
use crate::contexts::access_control::domain::{
    Action, PolicyDecision, Principal, ResourceType, RowFacets, VisibilityPredicate,
    compile_connection_target_visibility,
};
use crate::contexts::access_control::infrastructure::persistence::{
    SqlxPolicyRepository, SqlxRoleRepository, SqlxUserRepository,
};
use crate::contexts::inventory::domain::native::dataflow::{DataflowEndpoint, EndpointType};
use crate::contexts::inventory::infrastructure::persistence::repositories::{
    ConnectionTargetFilters, ConnectionTargetKind, ConnectionTargetRepository,
};
use crate::contexts::shared::request_context::{PlaybookJobBinding, RequestContext};
use crate::errors::AppError;

type ConcreteAccessControlService =
    AccessControlService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository>;

/// The compiled visibility decision for one list request.
pub enum ConnectionTargetVisibility {
    /// The caller may see every row — do not filter.
    All,
    /// The caller may see no row (no applicable read grant). A connection-target-
    /// typed endpoint should turn this into 403; an inventory-typed endpoint into
    /// an empty page.
    None,
    /// The caller sees the rows matching this predicate.
    Restricted(VisibilityPredicate),
}

impl ConnectionTargetVisibility {
    /// Convert to the repository filter value: `None` predicate = no filtering
    /// (All), otherwise the compiled predicate (`Restricted`, or the constant
    /// `Never` for `None`).
    pub fn into_filter(self) -> Option<VisibilityPredicate> {
        match self {
            ConnectionTargetVisibility::All => None,
            ConnectionTargetVisibility::None => Some(VisibilityPredicate::Never),
            ConnectionTargetVisibility::Restricted(predicate) => Some(predicate),
        }
    }
}

/// Resolve and compile the caller's connection-target row visibility for a
/// connection-target-typed (or monitoring-typed) endpoint. Enforces the PAT
/// token-scope ceiling first (a scoped token may only narrow), then compiles
/// the subject's resolved `Read × CONNECTION_TARGET` policies against the
/// request constraint context (so `ip_range`/`time_window` fold once).
pub async fn resolve_connection_target_visibility(
    state: &AppState,
    request_context: &RequestContext,
) -> Result<ConnectionTargetVisibility, AppError> {
    // PAT ceiling: an API token scoped away from `Read × CONNECTION_TARGET` (or a
    // scope pinned to a specific resource, which cannot satisfy a list call) sees
    // nothing here — the same ceiling `ensure_access` applies on per-target calls.
    if !ConcreteAccessControlService::api_token_scope_allows(
        request_context,
        &Action::Read,
        &ResourceType::ConnectionTarget,
        None,
    ) {
        return Ok(ConnectionTargetVisibility::None);
    }
    resolve_policy_visibility(state, request_context).await
}

/// Resolve the row visibility for an INVENTORY-typed endpoint. Skips the
/// `Read × CONNECTION_TARGET` token-scope ceiling: the endpoint's own coarse
/// `Read × Inventory` gate (which already honors the token scope) has run, and
/// a `read:inventory`-scoped PAT keeps browsing exactly the inventory its token
/// scope grants — only the SUBJECT's policies scope the rows. Applying the CT
/// ceiling here would deny that PAT its inventory browse.
pub async fn resolve_inventory_connection_target_visibility(
    state: &AppState,
    request_context: &RequestContext,
) -> Result<ConnectionTargetVisibility, AppError> {
    resolve_policy_visibility(state, request_context).await
}

/// Shared tail: compile the subject's resolved `Read × CONNECTION_TARGET`
/// policies against the request constraint context.
async fn resolve_policy_visibility(
    state: &AppState,
    request_context: &RequestContext,
) -> Result<ConnectionTargetVisibility, AppError> {
    let service = build_access_control_service(state);
    let policies = service
        .resolve_policies_for_request(request_context)
        .await?;
    let context = service.constraint_context_for_request(
        request_context,
        ResourceType::ConnectionTarget,
        None,
    )?;
    Ok(visibility_from_predicate(
        compile_connection_target_visibility(&policies, &context),
    ))
}

/// Classify a compiled predicate into the list decision it represents.
fn visibility_from_predicate(predicate: VisibilityPredicate) -> ConnectionTargetVisibility {
    if predicate.is_always() {
        ConnectionTargetVisibility::All
    } else if predicate.is_never() {
        ConnectionTargetVisibility::None
    } else {
        ConnectionTargetVisibility::Restricted(predicate)
    }
}

/// Resolve the row visibility for the ONE concrete host a playbook job token is
/// bound to, authorizing as the run's live initiator instead of the token.
///
/// A job token has no user identity. This endpoint is reachable through it only
/// when every binding holds:
///
/// - the requested `host_type` is a concrete host kind (`dcim.device` or
///   `virtualization.virtualmachine`);
/// - the token's bound connection target is the run's actual target;
/// - that connection target resolves to the requested concrete host, so the
///   caller cannot ask for another device, another virtual machine, or the other
///   kind sharing the same id; an id present in both host tables is ambiguous
///   and refused rather than resolved by precedence;
/// - the run group's initiator (requesting user, else service principal) still
///   exists in an active state;
/// - that initiator still holds the endpoint's coarse `Read × Inventory` grant —
///   the token's `inventory:read` ceiling narrows a live grant and never
///   restores a revoked one — plus a `Read × CONNECTION_TARGET` grant that covers
///   the host and every contributing peer.
///
/// Both decisions evaluate in THIS request's condition scope: the actor's
/// identity comes from the persisted run group, never from request input, and
/// `RequestContext::user_id` stays `None` for a job token, while the request's
/// resolved client IP and the current timestamp are combined with the actor's
/// principal subject. An `ip_range`/`time_window` constraint therefore applies
/// exactly as it does for the same actor's interactive request: an in-range
/// IP-constrained DENY still denies (fail-closed), and a grant whose IP range or
/// time window does not cover this request does not match.
pub async fn resolve_job_bound_host_visibility(
    state: &AppState,
    request_context: &RequestContext,
    host_type: EndpointType,
    host_id: Uuid,
) -> Result<ConnectionTargetVisibility, AppError> {
    let Some(binding) = request_context.playbook_job_binding.as_ref() else {
        return Err(AppError::Forbidden(
            "Playbook job token binding is unavailable".into(),
        ));
    };
    // Same refusal the snapshot applies, so a job token cannot probe non-host
    // endpoint types the external caller cannot either.
    if !host_type.is_host() {
        return Err(AppError::Validation(
            "host_type must be dcim.device or virtualization.virtualmachine".into(),
        ));
    }

    let initiator = resolve_job_bound_initiator(
        state,
        binding,
        DataflowEndpoint {
            object_type: host_type,
            object_id: host_id,
        },
    )
    .await?;

    let service = build_access_control_service(state);
    let inventory_decision = service
        .check_access_for_actor_in_request(
            &initiator,
            request_context,
            Action::Read,
            ResourceType::Inventory,
            None,
            None,
        )
        .await?;
    if inventory_decision.decision != PolicyDecision::Allow {
        return Err(AppError::Forbidden("Access denied".into()));
    }
    let policies = service.resolve_policies_for_principal(&initiator).await?;
    let context = service.constraint_context_for_actor_in_request(
        &initiator,
        request_context,
        ResourceType::ConnectionTarget,
        None,
    )?;
    Ok(visibility_from_predicate(
        compile_connection_target_visibility(&policies, &context),
    ))
}

/// Validate the token/run/target binding against the requested concrete host and
/// return the run group's live initiator principal.
///
/// The token is already validated (active, unexpired, attached to a live run) by
/// the authentication middleware; this re-checks the same run and target because
/// the endpoint addresses a host rather than the run, and because the actor must
/// come from the persisted run group, never from request input.
async fn resolve_job_bound_initiator(
    state: &AppState,
    binding: &PlaybookJobBinding,
    requested_host: DataflowEndpoint,
) -> Result<Principal, AppError> {
    if binding.target_kind != "connection_target" {
        return Err(AppError::Forbidden(
            "Playbook job token is not bound to a connection target".into(),
        ));
    }
    let Some(run) = state.services.playbooks.get_run(binding.run_id).await? else {
        return Err(AppError::Forbidden(
            "Playbook job token run is unavailable".into(),
        ));
    };
    if run.target_kind.as_deref() != Some(binding.target_kind.as_str())
        || run.target_id != Some(binding.target_id)
    {
        return Err(AppError::Forbidden(
            "Playbook job token is bound to another run target".into(),
        ));
    }

    // The connection-target union exposes device and VM ids. A token binds only
    // `connection_target`, so an id in both tables cannot identify its host kind.
    // Reject ambiguity even when the initiator may read both hosts.
    let (targets, _) = state
        .repositories
        .connection_targets
        .list(
            2,
            0,
            ConnectionTargetFilters {
                id: Some(binding.target_id),
                ..ConnectionTargetFilters::default()
            },
        )
        .await?;
    let target = match targets.as_slice() {
        [] => {
            return Err(AppError::Forbidden(
                "Playbook job token target is unavailable".into(),
            ));
        }
        [target] => target,
        _ => {
            return Err(AppError::Forbidden(
                "Playbook job token target host kind is ambiguous".into(),
            ));
        }
    };
    let bound_host = match target.target_kind {
        ConnectionTargetKind::Device => target.device_id.map(|id| DataflowEndpoint {
            object_type: EndpointType::Device,
            object_id: id,
        }),
        ConnectionTargetKind::VirtualMachine => target.vm_id.map(|id| DataflowEndpoint {
            object_type: EndpointType::VirtualMachine,
            object_id: id,
        }),
    };
    if bound_host.as_ref() != Some(&requested_host) {
        return Err(AppError::Forbidden(
            "Host is outside the playbook job token's bound target".into(),
        ));
    }

    let Some(detail) = state
        .services
        .playbooks
        .get_run_group_with_runs(run.run_group_id, 1)
        .await?
    else {
        return Err(AppError::Forbidden(
            "Playbook job token run group is unavailable".into(),
        ));
    };
    live_launch_principal(state, &detail.run_group).await
}

/// The row-facet view of one already-resolved target, in the exact string forms
/// the policy engine compares (canonical lowercase UUID rendering; status
/// trimmed and dropped when blank, mirroring the enforcement-site attribute
/// map). For per-row detail (deep-link) checks via
/// [`connection_target_row_visible`].
pub fn connection_target_row_facets(id: Uuid, facets: &ConnectionTargetFacets) -> RowFacets {
    RowFacets {
        id: id.to_string(),
        role_id: facets.role_id.map(|value| value.to_string()),
        site_id: facets.site_id.map(|value| value.to_string()),
        region_id: facets.region_id.map(|value| value.to_string()),
        tenant_id: facets.tenant_id.map(|value| value.to_string()),
        platform_id: facets.platform_id.map(|value| value.to_string()),
        cluster_id: facets.cluster_id.map(|value| value.to_string()),
        status: facets
            .status
            .as_deref()
            .map(str::trim)
            .filter(|status| !status.is_empty())
            .map(str::to_string),
    }
}

/// Per-row visibility check for INVENTORY detail (deep-link) endpoints: a host
/// the caller's `Read × CONNECTION_TARGET` policies hide from every list must
/// not resolve by direct URL either. Callers map `false` to their endpoint's
/// 404 so hidden and nonexistent ids stay indistinguishable. Uses the
/// inventory-typed resolution (no CT token-scope ceiling) to stay consistent
/// with the corresponding list endpoints.
pub async fn connection_target_row_visible(
    state: &AppState,
    request_context: &RequestContext,
    row: &RowFacets,
) -> Result<bool, AppError> {
    Ok(
        match resolve_inventory_connection_target_visibility(state, request_context).await? {
            ConnectionTargetVisibility::All => true,
            ConnectionTargetVisibility::None => false,
            ConnectionTargetVisibility::Restricted(predicate) => predicate.matches(row),
        },
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::contexts::access_control::domain::policy::{Policy, PolicyEffect};

    fn policy(
        effect: PolicyEffect,
        resources: Vec<String>,
        constraints: Option<serde_json::Value>,
    ) -> Policy {
        Policy {
            id: Uuid::now_v7(),
            name: "p".to_string(),
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

    #[test]
    fn row_facets_use_canonical_uuid_strings_and_trimmed_status() {
        let id = Uuid::now_v7();
        let site = Uuid::now_v7();
        let facets = ConnectionTargetFacets {
            site_id: Some(site),
            status: Some("  active  ".to_string()),
            ..Default::default()
        };
        let row = connection_target_row_facets(id, &facets);
        // Bare row id — policy `resources` patterns match the BARE id.
        assert_eq!(row.id, id.to_string());
        assert_eq!(row.site_id.as_deref(), Some(site.to_string().as_str()));
        // Trimmed, like the enforcement-site attribute map.
        assert_eq!(row.status.as_deref(), Some("active"));
        assert_eq!(row.role_id, None);
        assert_eq!(row.tenant_id, None);
    }

    #[test]
    fn row_facets_drop_blank_status_like_the_attribute_map() {
        let facets = ConnectionTargetFacets {
            status: Some("   ".to_string()),
            ..Default::default()
        };
        let row = connection_target_row_facets(Uuid::now_v7(), &facets);
        assert_eq!(row.status, None);
    }

    #[test]
    fn row_facets_satisfy_compiled_predicates_like_the_engine() {
        let id = Uuid::now_v7();
        let site = Uuid::now_v7();
        let other_site = Uuid::now_v7();
        let context = json!({"ip": "10.0.0.1", "timestamp": "2026-07-13T12:00:00Z"});
        let row = connection_target_row_facets(
            id,
            &ConnectionTargetFacets {
                site_id: Some(site),
                status: Some("active".to_string()),
                ..Default::default()
            },
        );

        // Site-scoped allow: matches the row's site, not another.
        let matching = compile_connection_target_visibility(
            &[policy(
                PolicyEffect::Allow,
                vec!["*".to_string()],
                Some(json!({"attributes": {"site_id": site.to_string()}})),
            )],
            &context,
        );
        assert!(matching.matches(&row));
        let mismatching = compile_connection_target_visibility(
            &[policy(
                PolicyEffect::Allow,
                vec!["*".to_string()],
                Some(json!({"attributes": {"site_id": other_site.to_string()}})),
            )],
            &context,
        );
        assert!(!mismatching.matches(&row));

        // Bare-id resource grant admits exactly this row.
        let by_id = compile_connection_target_visibility(
            &[policy(PolicyEffect::Allow, vec![id.to_string()], None)],
            &context,
        );
        assert!(by_id.matches(&row));

        // A target that no longer resolves (empty facets) fails closed for
        // attribute-scoped grants but still honors a bare-id grant.
        let unknown = connection_target_row_facets(id, &ConnectionTargetFacets::default());
        assert!(!matching.matches(&unknown));
        assert!(by_id.matches(&unknown));
    }
}
