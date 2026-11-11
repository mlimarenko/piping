//! Attribute resolution for connection-target authorization (ABAC).
//!
//! Turns a resolved target object's inventory facets plus the requested
//! connection protocol into the `{key: value}` attribute map the policy engine
//! evaluates `attributes` constraints against. This is what lets an operator
//! author "connect only to device-role RU3 over VNC" or "only on site A/B" and
//! have it actually enforced at the connect site.
//!
//! SECURITY: every attribute here derives from the RESOLVED TARGET OBJECT (the
//! device/VM the caller is connecting to) or from a server-validated request
//! field (the parsed protocol) — never from unvalidated caller input. A
//! caller-controlled attribute would be an authorization bypass.

use std::collections::HashMap;

use serde_json::Value;
use uuid::Uuid;

// The attribute keys are the single source of truth in the access-control
// vocabulary registry (so the write-time validator and the UI reference the same
// spelling); this enforcement site builds the map from those keys.
use crate::contexts::access_control::domain::vocabulary::{
    ATTR_CLUSTER_ID, ATTR_PLATFORM_ID, ATTR_PROTOCOL, ATTR_REGION_ID, ATTR_ROLE_ID, ATTR_SITE_ID,
    ATTR_STATUS, ATTR_TENANT_ID,
};

use crate::app::AppState;
use crate::contexts::access_control::domain::{RowFacets, VisibilityPredicate};
pub use crate::contexts::inventory::application::ConnectionTargetAuthorizationFacets as ConnectionTargetFacets;
use crate::contexts::inventory::application::resolve_connection_target_authorization_facets;
use crate::errors::AppError;

/// Fill `region_id` from the facets' site (one indexed PK lookup). A facets
/// value without a site — or whose site row disappeared — keeps `region_id`
/// as `None`: region-scoped policies then fail closed for that object, which
/// matches how every other absent facet behaves.
pub async fn enrich_facets_with_region(
    state: &AppState,
    mut facets: ConnectionTargetFacets,
) -> Result<ConnectionTargetFacets, AppError> {
    if let Some(site_id) = facets.site_id {
        facets.region_id = state
            .services
            .inventory_admin
            .get_site(site_id)
            .await?
            .and_then(|site| site.region_id);
    }
    Ok(facets)
}

/// Resolve a connection target's facets by id — device first, then virtual
/// machine. Returns `None` if the id matches neither. The single resolver every
/// connect/monitor/execute enforcement site uses so they all scope on the same
/// facets (and one place owns the per-check inventory lookup).
pub async fn resolve_connection_target_facets(
    state: &AppState,
    target_id: Uuid,
) -> Result<Option<ConnectionTargetFacets>, AppError> {
    let inventory = state.services.inventory_admin.clone();
    if let Some(device) = inventory.get_device(target_id).await? {
        let facets =
            enrich_facets_with_region(state, ConnectionTargetFacets::from(&device)).await?;
        return Ok(Some(facets));
    }
    if let Some(vm) = inventory.get_virtual_machine(target_id).await? {
        let facets = enrich_facets_with_region(state, ConnectionTargetFacets::from(&vm)).await?;
        return Ok(Some(facets));
    }
    Ok(None)
}

/// Resolve authorization facets for every known target with a fixed three-query
/// inventory plan. The returned values are owned by the inventory context; this
/// API module only translates them into the access-control engine's wire values.
pub async fn resolve_connection_target_facets_batch(
    state: &AppState,
    target_ids: &[Uuid],
) -> Result<HashMap<Uuid, ConnectionTargetFacets>, AppError> {
    resolve_connection_target_authorization_facets(&state.db_pool, target_ids).await
}

/// Evaluate one already-batched target against the compiled policy predicate.
/// Unknown targets intentionally use empty facets, matching the historical
/// point resolver's `unwrap_or_default()` behavior. The canonical resource id
/// retains both canonical (`connection_target:{id}`) and bare-id policy matching.
#[must_use]
pub fn batch_connection_target_is_allowed(
    predicate: &VisibilityPredicate,
    target_id: Uuid,
    facets: Option<&ConnectionTargetFacets>,
) -> bool {
    let facets = facets.cloned().unwrap_or_default();
    predicate.matches(&RowFacets {
        id: format!("connection_target:{target_id}"),
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
    })
}

/// Resolve the authorization attribute map for a known target id — the common
/// pattern every per-target enforcement site (monitor, execute, session
/// lifecycle, config/launch-option reads) uses. An unknown target yields a map
/// WITHOUT facets rather than an error: unconstrained policies behave exactly
/// as before (the handler's own downstream not-found still fires), while
/// facet-constrained policies fail closed. `protocol` is NOT facet-derived —
/// it comes from server state (the session's protocol or a parsed request
/// field) — so it is kept even when the target no longer resolves; a
/// protocol-keyed DENY must keep matching an active session whose target was
/// deleted from inventory. The connect-create site is the one place that
/// instead 404s on an unknown target, because it needs the facets for its
/// protocol decision anyway (documented trade-off there).
pub async fn connection_target_authz_attributes_for(
    state: &AppState,
    target_id: Uuid,
    protocol: Option<&str>,
) -> Result<HashMap<String, Value>, AppError> {
    let facets = resolve_connection_target_facets(state, target_id)
        .await?
        .unwrap_or_default();
    Ok(connection_target_authz_attributes(&facets, protocol))
}

/// Build the authorization attribute map for a connect/observe request. Absent
/// facets are omitted (a constraint keyed on a missing facet simply never
/// matches — fail-closed for that policy, not fail-open). `protocol` is the
/// server-parsed protocol string (e.g. `ssh`/`rdp`/`vnc`), lowercased.
pub fn connection_target_authz_attributes(
    facets: &ConnectionTargetFacets,
    protocol: Option<&str>,
) -> HashMap<String, Value> {
    let mut attributes = HashMap::new();
    let mut put_uuid = |key: &str, value: Option<Uuid>| {
        if let Some(id) = value {
            attributes.insert(key.to_string(), Value::String(id.to_string()));
        }
    };
    put_uuid(ATTR_ROLE_ID, facets.role_id);
    put_uuid(ATTR_SITE_ID, facets.site_id);
    put_uuid(ATTR_REGION_ID, facets.region_id);
    put_uuid(ATTR_TENANT_ID, facets.tenant_id);
    put_uuid(ATTR_PLATFORM_ID, facets.platform_id);
    put_uuid(ATTR_CLUSTER_ID, facets.cluster_id);
    if let Some(status) = facets.status.as_deref() {
        let status = status.trim();
        if !status.is_empty() {
            attributes.insert(ATTR_STATUS.to_string(), Value::String(status.to_string()));
        }
    }
    if let Some(protocol) = protocol {
        let protocol = protocol.trim().to_ascii_lowercase();
        if !protocol.is_empty() {
            attributes.insert(ATTR_PROTOCOL.to_string(), Value::String(protocol));
        }
    }
    attributes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_only_present_facets_plus_protocol() {
        let role = Uuid::now_v7();
        let site = Uuid::now_v7();
        let facets = ConnectionTargetFacets {
            role_id: Some(role),
            site_id: Some(site),
            status: Some("active".to_string()),
            ..Default::default()
        };
        let attrs = connection_target_authz_attributes(&facets, Some("VNC"));
        assert_eq!(
            attrs.get(ATTR_ROLE_ID),
            Some(&Value::String(role.to_string()))
        );
        assert_eq!(
            attrs.get(ATTR_SITE_ID),
            Some(&Value::String(site.to_string()))
        );
        assert_eq!(
            attrs.get(ATTR_STATUS),
            Some(&Value::String("active".to_string()))
        );
        // Protocol is normalized to lowercase so `protocol = "vnc"` constraints match.
        assert_eq!(
            attrs.get(ATTR_PROTOCOL),
            Some(&Value::String("vnc".to_string()))
        );
        // Absent facets are omitted, not null — a constraint on them never matches.
        assert!(!attrs.contains_key(ATTR_TENANT_ID));
        assert!(!attrs.contains_key(ATTR_PLATFORM_ID));
        assert!(!attrs.contains_key(ATTR_CLUSTER_ID));
        // Region is resolved separately (site lookup); un-enriched facets must
        // not leak a region key at all.
        assert!(!attrs.contains_key(ATTR_REGION_ID));
    }

    /// Extension tripwire (see the vocabulary module's extension guide): every
    /// attribute key DECLARED for `CONNECTION_TARGET` must actually land in the
    /// evaluation map when its facet is present — otherwise a policy authored
    /// on the new key would validate at write time yet silently never match at
    /// enforcement time. If this test is red after adding a key, wire the new
    /// `ConnectionTargetFacets` field into
    /// [`connection_target_authz_attributes`] (and populate it here).
    #[test]
    fn attribute_map_covers_every_declared_connection_target_key() {
        use crate::contexts::access_control::domain::ResourceType;
        use crate::contexts::access_control::domain::vocabulary::constraint_attribute_keys;

        let facets = ConnectionTargetFacets {
            role_id: Some(Uuid::now_v7()),
            site_id: Some(Uuid::now_v7()),
            region_id: Some(Uuid::now_v7()),
            tenant_id: Some(Uuid::now_v7()),
            platform_id: Some(Uuid::now_v7()),
            cluster_id: Some(Uuid::now_v7()),
            status: Some("active".to_string()),
        };
        let attrs = connection_target_authz_attributes(&facets, Some("ssh"));
        for key in constraint_attribute_keys(ResourceType::ConnectionTarget) {
            assert!(
                attrs.contains_key(*key),
                "declared attribute key `{key}` never reaches the evaluation map — \
                 populate it in connection_target_authz_attributes (and in this \
                 test's fully-populated facets)"
            );
        }
    }

    #[test]
    fn enriched_region_lands_in_the_attribute_map() {
        let region = Uuid::now_v7();
        let facets = ConnectionTargetFacets {
            region_id: Some(region),
            ..Default::default()
        };
        let attrs = connection_target_authz_attributes(&facets, None);
        assert_eq!(
            attrs.get(ATTR_REGION_ID),
            Some(&Value::String(region.to_string()))
        );
    }

    #[test]
    fn empty_facets_and_no_protocol_yield_empty_map() {
        let attrs = connection_target_authz_attributes(&ConnectionTargetFacets::default(), None);
        assert!(attrs.is_empty());
    }

    #[test]
    fn blank_status_and_protocol_are_dropped() {
        let facets = ConnectionTargetFacets {
            status: Some("   ".to_string()),
            ..Default::default()
        };
        let attrs = connection_target_authz_attributes(&facets, Some("  "));
        assert!(attrs.is_empty());
    }
}
