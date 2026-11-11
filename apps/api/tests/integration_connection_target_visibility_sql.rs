//! DB-backed fidelity test for the connection-target visibility SQL rendering.
//!
//! The pure `VisibilityPredicate::matches` is unit-tested against the real policy
//! engine (`domain::visibility` tests). This test pins the SEPARATE SQL rendering
//! to that same reference: it renders each compiled predicate into a real query
//! against a temp table of synthetic rows spanning every facet/NULL combination
//! and asserts the returned id-set equals `matches` over the same rows. This is
//! the test that catches three-valued-logic divergences (a negated equality over
//! a NULL column) and id-pattern SQL-dialect drift (glob → LIKE, `re:` → POSIX
//! regex) that a string snapshot never would.
//!
//! Ignored by default like the other DB integration tests (the attribute was
//! missing until 2026-07-27, so this ran and self-skipped to PASS without a
//! database — the security boundary it pins was unverified); requires a Postgres
//! reachable at `PIPING_DATABASE__URL` (any database — it only uses a TEMP table).

mod common;

use piping_backend::contexts::access_control::domain::{
    Action, Policy, PolicyEffect, ResourceType, RowFacets, compile_connection_target_visibility,
};
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::visibility_sql::{
    VisibilityColumns, push_visibility_and,
};
use serde_json::{Value, json};
use sqlx::{Postgres, QueryBuilder, Row};
use uuid::Uuid;

const COLUMNS: VisibilityColumns = VisibilityColumns {
    id: "id",
    role_id: "role_id",
    site_id: "site_id",
    // Same scalar-subquery SHAPE as the production read-models (which resolve
    // region over `dcim_site`); here it resolves over the temp site table so
    // the 3-valued semantics of a derived (non-physical) facet column are
    // pinned by the fidelity loop too.
    region_id: "(SELECT s.region_id FROM ct_vis_site s WHERE s.id = site_id)",
    tenant_id: "tenant_id",
    platform_id: "platform_id",
    cluster_id: "cluster_id",
    status: "status",
};

// Fixed ids so policies and rows reference the same values.
const SITE_A: &str = "00000000-0000-7000-8000-0000000000a1";
const SITE_B: &str = "00000000-0000-7000-8000-0000000000a2";
// SITE_A sits in REGION_R1; SITE_B has NO region (NULL) — the derived-facet
// three-valued stressor (deny-by-region must not hide region-less rows).
const REGION_R1: &str = "00000000-0000-7000-8000-0000000000d1";
const RU1: &str = "00000000-0000-7000-8000-0000000000b1";
const RU3: &str = "00000000-0000-7000-8000-0000000000b3";
const DEV1: &str = "00000000-0000-7000-8000-0000000000c1";
const DEV2: &str = "00000000-0000-7000-8000-0000000000c2";
const DEV_GLOB: &str = "00000000-0000-7000-8000-0000000000c9";

struct Row0 {
    id: &'static str,
    site: Option<&'static str>,
    role: Option<&'static str>,
    status: Option<&'static str>,
}

fn rows() -> Vec<Row0> {
    vec![
        Row0 {
            id: DEV1,
            site: Some(SITE_A),
            role: Some(RU1),
            status: Some("active"),
        },
        Row0 {
            id: DEV2,
            site: Some(SITE_B),
            role: Some(RU3),
            status: Some("decommissioning"),
        },
        // NULL facets — the three-valued-logic stressors.
        Row0 {
            id: "00000000-0000-7000-8000-0000000000c3",
            site: None,
            role: Some(RU1),
            status: None,
        },
        Row0 {
            id: "00000000-0000-7000-8000-0000000000c4",
            site: Some(SITE_A),
            role: None,
            status: Some("active"),
        },
        Row0 {
            id: DEV_GLOB,
            site: None,
            role: None,
            status: None,
        },
    ]
}

/// The region the temp site table assigns to a row's site: SITE_A → REGION_R1,
/// SITE_B → none, no site → none. The `matches()` reference must see exactly
/// what the SQL subquery resolves.
fn region_for(site: Option<&str>) -> Option<&'static str> {
    (site == Some(SITE_A)).then_some(REGION_R1)
}

fn row_facets(row: &Row0) -> RowFacets {
    RowFacets {
        id: row.id.to_string(),
        site_id: row.site.map(str::to_string),
        region_id: region_for(row.site).map(str::to_string),
        role_id: row.role.map(str::to_string),
        status: row.status.map(str::to_string),
        ..Default::default()
    }
}

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

#[tokio::test]
#[ignore = "requires database (run with local dev Postgres)"]
async fn sql_rendering_matches_engine_reference_across_null_and_pattern_cases() {
    let pool = common::build_test_db_pool().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    // A single connection holds the TEMP table for the whole test.
    let mut conn = pool.acquire().await.expect("acquire");
    sqlx::query(
        "CREATE TEMP TABLE ct_vis_test (
            id uuid PRIMARY KEY, role_id uuid, site_id uuid, tenant_id uuid,
            platform_id uuid, cluster_id uuid, status text)",
    )
    .execute(conn.as_mut())
    .await
    .expect("create temp table");
    // Site → region mapping the derived region facet resolves through (the
    // production columns run the same subquery over `dcim_site`).
    sqlx::query("CREATE TEMP TABLE ct_vis_site (id uuid PRIMARY KEY, region_id uuid)")
        .execute(conn.as_mut())
        .await
        .expect("create temp site table");
    for (site, region) in [(SITE_A, Some(REGION_R1)), (SITE_B, None)] {
        sqlx::query("INSERT INTO ct_vis_site (id, region_id) VALUES ($1,$2)")
            .bind(Uuid::parse_str(site).unwrap())
            .bind(region.map(|v| Uuid::parse_str(v).unwrap()))
            .execute(conn.as_mut())
            .await
            .expect("insert site row");
    }
    for row in rows() {
        sqlx::query("INSERT INTO ct_vis_test (id, site_id, role_id, status) VALUES ($1,$2,$3,$4)")
            .bind(Uuid::parse_str(row.id).unwrap())
            .bind(row.site.map(|v| Uuid::parse_str(v).unwrap()))
            .bind(row.role.map(|v| Uuid::parse_str(v).unwrap()))
            .bind(row.status)
            .execute(conn.as_mut())
            .await
            .expect("insert row");
    }

    let policy_sets: Vec<Vec<Policy>> = vec![
        // unconstrained allow → Always (renders nothing; all rows)
        vec![policy(PolicyEffect::Allow, vec!["*"], None)],
        // site-scoped allow
        vec![policy(
            PolicyEffect::Allow,
            vec!["*"],
            Some(json!({"attributes": {"site_id": SITE_A}})),
        )],
        // allow-all + deny site-a → NULL-site rows MUST remain visible
        vec![
            policy(PolicyEffect::Allow, vec!["*"], None),
            policy(
                PolicyEffect::Deny,
                vec!["*"],
                Some(json!({"attributes": {"site_id": SITE_A}})),
            ),
        ],
        // deny by role OR (deny-override, NULL role must not be hidden)
        vec![
            policy(PolicyEffect::Allow, vec!["*"], None),
            policy(
                PolicyEffect::Deny,
                vec!["*"],
                Some(
                    json!({"or": [{"attributes": {"role_id": RU1}}, {"attributes": {"role_id": RU3}}]}),
                ),
            ),
        ],
        // and/or nested attribute tree
        vec![policy(
            PolicyEffect::Allow,
            vec!["*"],
            Some(json!({"and": [
                {"or": [{"attributes": {"role_id": RU1}}, {"attributes": {"role_id": RU3}}]},
                {"attributes": {"status": "active"}}
            ]})),
        )],
        // status deny with NULL status rows present
        vec![
            policy(PolicyEffect::Allow, vec!["*"], None),
            policy(
                PolicyEffect::Deny,
                vec!["*"],
                Some(json!({"attributes": {"status": "decommissioning"}})),
            ),
        ],
        // region-scoped allow: only rows whose SITE sits in R1 (derived facet
        // resolved by subquery — site-less and region-less rows excluded)
        vec![policy(
            PolicyEffect::Allow,
            vec!["*"],
            Some(json!({"attributes": {"region_id": REGION_R1}})),
        )],
        // allow-all + region deny: rows without a site OR whose site has no
        // region must REMAIN visible (NULL subquery vs negated equality)
        vec![
            policy(PolicyEffect::Allow, vec!["*"], None),
            policy(
                PolicyEffect::Deny,
                vec!["*"],
                Some(json!({"attributes": {"region_id": REGION_R1}})),
            ),
        ],
        // region AND role — derived facet composed with a physical one
        vec![policy(
            PolicyEffect::Allow,
            vec!["*"],
            Some(json!({"and": [
                {"attributes": {"region_id": REGION_R1}},
                {"attributes": {"role_id": RU1}}
            ]})),
        )],
        // resource-id exact
        vec![policy(PolicyEffect::Allow, vec![DEV1], None)],
        // resource-id exact set
        vec![policy(PolicyEffect::Allow, vec![DEV1, DEV2], None)],
        // resource-id glob (matches the c9 tail via prefix)
        vec![policy(
            PolicyEffect::Allow,
            vec!["00000000-0000-7000-8000-0000000000c*"],
            None,
        )],
        // resource-id regex
        vec![policy(PolicyEffect::Allow, vec!["re:.*0000000000c1"], None)],
        // re: patterns the Rust engine rejects must compile to Never (fail-closed)
        // and never reach Postgres — proving no dialect over-disclosure and no
        // 'invalid regular expression' 500. matches() and SQL must both yield 0.
        vec![policy(PolicyEffect::Allow, vec!["re:(?=.*c1$).+"], None)],
        vec![policy(PolicyEffect::Allow, vec!["re:("], None)],
        // an unconstrained allow plus an invalid-re: DENY: the deny leg is Never,
        // so it hides nothing (all rows stay visible) — and no bad regex is sent.
        vec![
            policy(PolicyEffect::Allow, vec!["*"], None),
            policy(PolicyEffect::Deny, vec!["re:(?=x)"], None),
        ],
        // deny by resource id (allow-all minus one id)
        vec![
            policy(PolicyEffect::Allow, vec!["*"], None),
            policy(PolicyEffect::Deny, vec![DEV1], None),
        ],
    ];

    let rows = rows();
    for (index, policies) in policy_sets.iter().enumerate() {
        let predicate = compile_connection_target_visibility(policies, &context());

        let mut expected: Vec<String> = rows
            .iter()
            .filter(|row| predicate.matches(&row_facets(row)))
            .map(|row| row.id.to_string())
            .collect();
        expected.sort();

        let mut builder = QueryBuilder::<Postgres>::new("SELECT id FROM ct_vis_test WHERE TRUE");
        push_visibility_and(&mut builder, &predicate, &COLUMNS);
        builder.push(" ORDER BY id");
        let fetched = builder
            .build()
            .fetch_all(conn.as_mut())
            .await
            .expect("run predicate");
        let mut got: Vec<String> = fetched
            .into_iter()
            .map(|r| r.get::<Uuid, _>("id").to_string())
            .collect();
        got.sort();

        assert_eq!(
            got, expected,
            "policy set #{index}: SQL rendering diverged from the matches() reference"
        );
    }
}
