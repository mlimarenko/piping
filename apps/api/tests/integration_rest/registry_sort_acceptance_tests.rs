//! Sort acceptance for DB-backed registry list endpoints.
//!
//! Positive acceptance: every representative collection answers
//! `GET <path>?sort=<key>&order=asc` with a page envelope instead of a
//! validation error. Negative acceptance: an unknown key is rejected with 400
//! for the same representatives, which extends the existing 400 coverage beyond
//! the ten inventory collections in `inventory_and_provider_tests.rs`.
//!
//! Sort keys are not guesses: each probe names the collection's own
//! `&[ListColumn]` allowlist, resolved by `SortSpec::parse_columns` in
//! `contexts/shared/list_query.rs`. The prefix and the ordering of that
//! allowlist decide which logical key is legal; the response envelope shape
//! decides which pagination fields must be present.

use super::*;

/// One DB-backed registry list endpoint exercised by the sort acceptance pass.
struct RegistryListProbe {
    /// Query path without the `sort`/`order` pair.
    path: &'static str,
    /// Allowlisted logical sort key; the owning `ListColumn` registry is named
    /// in the comment above the entry.
    sort_key: &'static str,
    /// `false` only for a list endpoint whose envelope omits `limit`/`offset`.
    paginated: bool,
}

const REGISTRY_LIST_PROBES: &[RegistryListProbe] = &[
    // --- inventory family ---
    // DEVICE_COLUMNS: `ListColumn::sortable("name", "d.name", ..)`.
    RegistryListProbe {
        path: "/api/v1/inventory/devices",
        sort_key: "name",
        paginated: true,
    },
    // SITE_COLUMNS: `ListColumn::sortable("name", "s.name", ..)`.
    RegistryListProbe {
        path: "/api/v1/inventory/sites",
        sort_key: "name",
        paginated: true,
    },
    // IP_ADDRESS_COLUMNS: `ListColumn::sortable("address", "ip.address", ..)`.
    RegistryListProbe {
        path: "/api/v1/inventory/ip-addresses",
        sort_key: "address",
        paginated: true,
    },
    // VLAN_COLUMNS: `ListColumn::sortable("vid", "vl.vid", ..)`.
    RegistryListProbe {
        path: "/api/v1/inventory/vlans",
        sort_key: "vid",
        paginated: true,
    },
    // VIRTUAL_MACHINE_COLUMNS: `ListColumn::sortable("name", "v.name", ..)`.
    RegistryListProbe {
        // Route path uses an underscore: `router.rs` line for `list_virtual_machines`.
        path: "/api/v1/inventory/virtual_machines",
        sort_key: "name",
        paginated: true,
    },
    // --- non-inventory family ---
    // USER_COLUMNS: `ListColumn::sortable("username", "username", ..)`.
    RegistryListProbe {
        path: "/api/v1/admin/access-control/users",
        sort_key: "username",
        paginated: true,
    },
    // PLAYBOOK_RUN_COLUMNS: `ListColumn::sortable("started_at", "started_at", ..)`.
    RegistryListProbe {
        path: "/api/v1/playbooks/runs",
        sort_key: "started_at",
        paginated: true,
    },
    // WEBHOOK_SUBSCRIPTION_COLUMNS: `ListColumn::sortable("name", "name", ..)`.
    RegistryListProbe {
        path: "/api/v1/webhooks",
        sort_key: "name",
        paginated: true,
    },
    // EXPORTER_COLUMNS: `ListColumn::sortable("slug", "slug", ..)`. The catalog
    // envelope carries `items`/`total` only, so `paginated` is false.
    RegistryListProbe {
        path: "/api/v1/monitoring/exporters",
        sort_key: "slug",
        paginated: false,
    },
    // CONNECTION_TARGET_COLUMNS: `ListColumn::sortable("name", ..)`.
    RegistryListProbe {
        path: "/api/v1/connections/targets",
        sort_key: "name",
        paginated: true,
    },
];

/// Key that no collection's allowlist can contain.
const UNKNOWN_SORT_KEY: &str = "__definitely_not_a_column__";

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis"]
async fn registry_sorts_are_accepted_and_unknown_keys_rejected() {
    let state = build_test_app_state().await.expect("app state");
    let app = piping_backend::app::build_router(state);
    let cookie = login_and_get_cookie(&app, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD)
        .await
        .expect("login");

    for probe in REGISTRY_LIST_PROBES {
        let accepted = format!("{}?sort={}&order=asc", probe.path, probe.sort_key);
        let (status, body) = request(&app, "GET", &accepted, None, Some(&cookie))
            .await
            .expect("sorted request");
        let rendered = String::from_utf8_lossy(&body);
        assert_eq!(
            status,
            StatusCode::OK,
            "{} rejected allowlisted sort={}: {rendered}",
            probe.path,
            probe.sort_key
        );
        let page: serde_json::Value = serde_json::from_slice(&body).expect("page JSON");
        assert!(
            page.get("items")
                .and_then(|value| value.as_array())
                .is_some(),
            "{}: items array missing: {rendered}",
            probe.path
        );
        assert!(
            page.get("total").and_then(|value| value.as_u64()).is_some(),
            "{}: total missing: {rendered}",
            probe.path
        );
        if probe.paginated {
            assert!(
                page.get("limit").and_then(|value| value.as_u64()).is_some(),
                "{}: limit missing: {rendered}",
                probe.path
            );
            assert!(
                page.get("offset")
                    .and_then(|value| value.as_u64())
                    .is_some(),
                "{}: offset missing: {rendered}",
                probe.path
            );
        }

        let rejected = format!("{}?sort={UNKNOWN_SORT_KEY}", probe.path);
        let (status, body) = request(&app, "GET", &rejected, None, Some(&cookie))
            .await
            .expect("unknown-sort request");
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{} accepted unknown sort key: {}",
            probe.path,
            String::from_utf8_lossy(&body)
        );
    }
}
