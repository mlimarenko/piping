//! Curated catalog of outbound-webhook-worthy domain events.
//!
//! Each entry's `value` is the EXACT `event_type` string emitted through the
//! audit sink, so a subscription created from a catalog value will actually
//! match delivered events. Internal/read-only events (`*.read`,
//! `secrets.resolve`, dead-letter, mcp tool invocations) and non-dotted event
//! names (e.g. `seat_assigned`) are deliberately excluded — the latter would
//! also fail the `^[a-z][a-z0-9.]*$` subscription validation.

/// A single curated catalog entry: an `object × action` pairing mapped to the
/// real emitted `event_type`.
#[derive(Debug, Clone, Copy)]
pub struct WebhookEventCatalogItem {
    /// The actual emitted `event_type` string (subscription target).
    pub value: &'static str,
    /// The domain object the event concerns (e.g. `tenant`, `session`).
    pub object: &'static str,
    /// The action performed (e.g. `created`, `terminated`).
    pub action: &'static str,
    /// A high-level grouping for UI organization.
    pub group: &'static str,
}

const CATALOG: &[WebhookEventCatalogItem] = &[
    // Inventory
    WebhookEventCatalogItem {
        value: "inventory.tenant.created",
        object: "tenant",
        action: "created",
        group: "inventory",
    },
    WebhookEventCatalogItem {
        value: "inventory.tenant.updated",
        object: "tenant",
        action: "updated",
        group: "inventory",
    },
    WebhookEventCatalogItem {
        value: "inventory.tenant.deleted",
        object: "tenant",
        action: "deleted",
        group: "inventory",
    },
    // Connections
    WebhookEventCatalogItem {
        value: "connection.session.created",
        object: "session",
        action: "created",
        group: "connections",
    },
    WebhookEventCatalogItem {
        value: "connection.session.activated",
        object: "session",
        action: "activated",
        group: "connections",
    },
    WebhookEventCatalogItem {
        value: "connection.session.disconnected",
        object: "session",
        action: "disconnected",
        group: "connections",
    },
    WebhookEventCatalogItem {
        value: "connection.session.failed",
        object: "session",
        action: "failed",
        group: "connections",
    },
    WebhookEventCatalogItem {
        value: "connection.session.terminated",
        object: "session",
        action: "terminated",
        group: "connections",
    },
    // Sync
    WebhookEventCatalogItem {
        value: "sync.mapping.created",
        object: "mapping",
        action: "created",
        group: "sync",
    },
    WebhookEventCatalogItem {
        value: "sync.mapping.updated",
        object: "mapping",
        action: "updated",
        group: "sync",
    },
    WebhookEventCatalogItem {
        value: "sync.mapping.deleted",
        object: "mapping",
        action: "deleted",
        group: "sync",
    },
];

/// Return the curated webhook event catalog.
#[must_use]
pub fn webhook_event_catalog() -> &'static [WebhookEventCatalogItem] {
    CATALOG
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    use regex::Regex;

    fn event_type_regex() -> &'static Regex {
        static CELL: OnceLock<Regex> = OnceLock::new();
        CELL.get_or_init(|| Regex::new(r"^[a-z][a-z0-9.]*$").unwrap())
    }

    #[test]
    fn all_values_pass_subscription_regex() {
        let re = event_type_regex();
        for item in webhook_event_catalog() {
            assert!(
                re.is_match(item.value),
                "catalog value '{}' must match subscription regex",
                item.value
            );
        }
    }

    #[test]
    fn values_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for item in webhook_event_catalog() {
            assert!(
                seen.insert(item.value),
                "duplicate catalog value {}",
                item.value
            );
        }
    }

    #[test]
    fn catalog_is_non_empty() {
        assert!(!webhook_event_catalog().is_empty());
    }
}
