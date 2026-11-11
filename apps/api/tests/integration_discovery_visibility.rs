//! DB-backed fidelity test for row-level visibility on the discovery read-model.
//!
//! The generic SQL renderer is pinned to the pure `matches` reference by
//! `integration_connection_target_visibility_sql.rs` over a temp table. This
//! test covers what is NEW in the discovery surface: the dynamically rebuilt
//! `list_discovery_hosts` WHERE clause, the lean `list_visible_host_ids`
//! projection, and the raw-site semantics — a VM with no own `site_id` must be
//! HIDDEN from a site-scoped caller even when its cluster denormalizes a site
//! (the display `site_id` coalesces, the visibility facet must not, mirroring
//! the per-target enforcement sites and the device/VM list read-models).
//!
//! Self-skips like the other DB integration tests; requires a Postgres with the
//! application schema at `PIPING_DATABASE__URL` (e.g. `make run-up`).

mod common;

use std::collections::HashSet;

use piping_backend::contexts::access_control::domain::{
    Action, Policy, PolicyEffect, ResourceType, VisibilityPredicate,
    compile_connection_target_visibility,
};
use piping_backend::contexts::inventory::domain::DiscoveryHostFilters;
use piping_backend::contexts::inventory::infrastructure::persistence::{
    DiscoveryRepository, SqlxDiscoveryRepository,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

fn policy(effect: PolicyEffect, resources: Vec<&str>, constraints: Option<Value>) -> Policy {
    Policy {
        id: Uuid::now_v7(),
        name: "p".to_string(),
        description: None,
        effect,
        actions: vec![Action::Read],
        resource_types: vec![ResourceType::ConnectionTarget],
        constraints,
        subjects: Vec::new(),
        resources: resources.into_iter().map(str::to_string).collect(),
        enabled: true,
    }
}

fn context() -> Value {
    json!({"ip": "10.1.2.3", "timestamp": "2026-07-13T12:00:00Z"})
}

struct Fixtures {
    prefix: String,
    site_id: Uuid,
    cluster_type_id: Uuid,
    cluster_id: Uuid,
    /// VM with its OWN `site_id` set, status `active`.
    vm_own_site: Uuid,
    /// VM with NULL own `site_id` but a cluster carrying `_site_id` — visible
    /// in a site-scoped list ONLY if the filter (wrongly) used the coalesced
    /// display site.
    vm_cluster_site: Uuid,
    /// VM with no site at all, status `offline`.
    vm_no_site: Uuid,
}

async fn insert_fixtures(pool: &PgPool) -> Result<Fixtures, sqlx::Error> {
    let prefix = format!("ctvisdisco-{}", Uuid::now_v7().simple());
    let fixtures = Fixtures {
        site_id: Uuid::now_v7(),
        cluster_type_id: Uuid::now_v7(),
        cluster_id: Uuid::now_v7(),
        vm_own_site: Uuid::now_v7(),
        vm_cluster_site: Uuid::now_v7(),
        vm_no_site: Uuid::now_v7(),
        prefix,
    };
    sqlx::query(
        "INSERT INTO dcim_site (custom_field_data, id, name, slug, status, facility, \
         description, physical_address, shipping_address, comments) \
         VALUES ('{}', $1, $2, $2, 'active', '', '', '', '', '')",
    )
    .bind(fixtures.site_id)
    .bind(format!("{}-site", fixtures.prefix))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO virtualization_clustertype (custom_field_data, id, name, slug, \
         description, comments) VALUES ('{}', $1, $2, $2, '', '')",
    )
    .bind(fixtures.cluster_type_id)
    .bind(format!("{}-ct", fixtures.prefix))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO virtualization_cluster (custom_field_data, id, name, comments, type_id, \
         status, description, _site_id) VALUES ('{}', $1, $2, '', $3, 'active', '', $4)",
    )
    .bind(fixtures.cluster_id)
    .bind(format!("{}-cl", fixtures.prefix))
    .bind(fixtures.cluster_type_id)
    .bind(fixtures.site_id)
    .execute(pool)
    .await?;
    for (id, suffix, status, site_id, cluster_id) in [
        (
            fixtures.vm_own_site,
            "own-site",
            "active",
            Some(fixtures.site_id),
            None,
        ),
        (
            fixtures.vm_cluster_site,
            "cluster-site",
            "active",
            None,
            Some(fixtures.cluster_id),
        ),
        (fixtures.vm_no_site, "no-site", "offline", None, None),
    ] {
        sqlx::query(
            "INSERT INTO virtualization_virtualmachine (custom_field_data, id, name, status, \
             comments, description, interface_count, virtual_disk_count, serial, start_on_boot, \
             site_id, cluster_id) VALUES ('{}', $1, $2, $3, '', '', 0, 0, '', 'inherit', $4, $5)",
        )
        .bind(id)
        .bind(format!("{}-{suffix}", fixtures.prefix))
        .bind(status)
        .bind(site_id)
        .bind(cluster_id)
        .execute(pool)
        .await?;
    }
    Ok(fixtures)
}

async fn cleanup_fixtures(pool: &PgPool, fixtures: &Fixtures) {
    let _ = sqlx::query("DELETE FROM virtualization_virtualmachine WHERE id = ANY($1)")
        .bind(vec![
            fixtures.vm_own_site,
            fixtures.vm_cluster_site,
            fixtures.vm_no_site,
        ])
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM virtualization_cluster WHERE id = $1")
        .bind(fixtures.cluster_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM virtualization_clustertype WHERE id = $1")
        .bind(fixtures.cluster_type_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM dcim_site WHERE id = $1")
        .bind(fixtures.site_id)
        .execute(pool)
        .await;
}

fn scoped_filters(prefix: &str) -> DiscoveryHostFilters {
    DiscoveryHostFilters {
        search: Some(prefix.to_string()),
        fetch_limit: 50,
        ..DiscoveryHostFilters::default()
    }
}

async fn listed_ids(
    repository: &SqlxDiscoveryRepository,
    filters: &DiscoveryHostFilters,
    visibility: Option<&VisibilityPredicate>,
) -> Result<HashSet<Uuid>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(repository
        .list_discovery_hosts(filters, visibility)
        .await
        .map_err(|e| format!("list discovery hosts: {e}"))?
        .into_iter()
        .map(|row| row.host_id)
        .collect())
}

#[tokio::test]
#[ignore = "requires database (run with local dev Postgres)"]
async fn discovery_visibility_filters_rows_like_the_engine()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    let fixtures = match insert_fixtures(&pool).await {
        Ok(fixtures) => fixtures,
        Err(error) => {
            panic!("insert fixtures: {error}");
        }
    };

    let repository = SqlxDiscoveryRepository::new(pool.clone());
    let filters = scoped_filters(&fixtures.prefix);
    let site = fixtures.site_id.to_string();

    let site_scoped = compile_connection_target_visibility(
        &[policy(
            PolicyEffect::Allow,
            vec!["*"],
            Some(json!({"attributes": {"site_id": site}})),
        )],
        &context(),
    );
    let status_deny = compile_connection_target_visibility(
        &[
            policy(PolicyEffect::Allow, vec!["*"], None),
            policy(
                PolicyEffect::Deny,
                vec!["*"],
                Some(json!({"attributes": {"status": "offline"}})),
            ),
        ],
        &context(),
    );

    // Collect every result first, clean up, then assert — a failed assertion
    // must not leave fixture rows behind in the shared dev database.
    let unfiltered = listed_ids(&repository, &filters, None).await?;
    let site_visible = listed_ids(&repository, &filters, Some(&site_scoped)).await?;
    let status_visible = listed_ids(&repository, &filters, Some(&status_deny)).await?;
    let never_visible =
        listed_ids(&repository, &filters, Some(&VisibilityPredicate::Never)).await?;

    let unknown_id = Uuid::now_v7();
    let requested = vec![
        fixtures.vm_own_site,
        fixtures.vm_cluster_site,
        fixtures.vm_no_site,
        unknown_id,
    ];
    let visible_ids_site = repository
        .list_visible_host_ids(&requested, &site_scoped)
        .await
        .expect("visible ids (site)");
    let visible_ids_status = repository
        .list_visible_host_ids(&requested, &status_deny)
        .await
        .expect("visible ids (status deny)");
    let visible_ids_never = repository
        .list_visible_host_ids(&requested, &VisibilityPredicate::Never)
        .await
        .expect("visible ids (never)");
    let visible_ids_always = repository
        .list_visible_host_ids(&requested, &VisibilityPredicate::Always)
        .await
        .expect("visible ids (always)");

    cleanup_fixtures(&pool, &fixtures).await;

    let all: HashSet<Uuid> = [
        fixtures.vm_own_site,
        fixtures.vm_cluster_site,
        fixtures.vm_no_site,
    ]
    .into_iter()
    .collect();
    assert_eq!(unfiltered, all, "unfiltered read must return every fixture");

    // RAW site semantics: only the VM with its OWN site is visible. The
    // cluster-derived display site must not widen visibility.
    assert_eq!(
        site_visible,
        HashSet::from([fixtures.vm_own_site]),
        "site-scoped visibility must use the raw row site, not the cluster fallback"
    );

    assert_eq!(
        status_visible,
        HashSet::from([fixtures.vm_own_site, fixtures.vm_cluster_site]),
        "status deny must hide only the matching row (NULL facets stay visible)"
    );

    assert!(
        never_visible.is_empty(),
        "a Never predicate must return no rows"
    );

    assert_eq!(
        visible_ids_site,
        vec![fixtures.vm_own_site],
        "visible-id filter must keep only the raw-site match and drop unknown ids"
    );
    let status_set: HashSet<Uuid> = visible_ids_status.into_iter().collect();
    assert_eq!(
        status_set,
        HashSet::from([fixtures.vm_own_site, fixtures.vm_cluster_site]),
        "visible-id filter must apply deny-override and drop unknown ids"
    );
    assert!(
        visible_ids_never.is_empty(),
        "a Never predicate must admit no ids"
    );
    // `Always` short-circuits to the input as-is (unknown ids included): the
    // caller treats every requested id as visible, exactly like the pre-filter
    // behavior for unconstrained callers — unknown ids then surface as plain
    // cache misses, never as data.
    assert_eq!(visible_ids_always, requested);
    Ok(())
}
