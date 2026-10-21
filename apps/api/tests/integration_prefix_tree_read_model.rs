//! DB-backed tests for the native IPAM prefix tree read model.
//!
//! These tests exercise `SqlxPrefixRepository::list_tree` against migrated
//! PostgreSQL data. They are ignored by default because they require the local
//! dev database, matching the repository's other DB integration tests.

mod common;

use std::collections::HashMap;

use piping_backend::contexts::inventory::infrastructure::persistence::{
    PrefixRepository, PrefixTreeFilter, SqlxPrefixRepository,
};
use sqlx::PgPool;
use uuid::Uuid;

const TEST_VRF_ID: Uuid = Uuid::from_u128(0x00000000_0000_7002_8000_000000000001);
const ROOT_ID: Uuid = Uuid::from_u128(0x00000000_0000_7002_8000_000000000010);
const PARENT_ID: Uuid = Uuid::from_u128(0x00000000_0000_7002_8000_000000000011);
const CHILD_A_ID: Uuid = Uuid::from_u128(0x00000000_0000_7002_8000_000000000012);
const CHILD_B_ID: Uuid = Uuid::from_u128(0x00000000_0000_7002_8000_000000000013);
const MARK_UTILIZED_ID: Uuid = Uuid::from_u128(0x00000000_0000_7002_8000_000000000014);
const IPV6_ROOT_ID: Uuid = Uuid::from_u128(0x00000000_0000_7002_8000_000000000015);
const LIMIT_ROOT_ID: Uuid = Uuid::from_u128(0x00000000_0000_7002_8000_000000000020);

async fn migrated_pool() -> Result<PgPool, Box<dyn std::error::Error + Send + Sync>> {
    let mut pool = common::build_test_db_pool().await?;
    let schema_exists: bool =
        sqlx::query_scalar("SELECT to_regclass('public.ipam_prefix') IS NOT NULL")
            .fetch_one(&pool)
            .await
            .map_err(|e| format!("check prefix table presence: {e}"))?;
    if schema_exists {
        return Ok(pool);
    }
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;
    pool.close().await;
    pool = common::build_test_db_pool()
        .await
        .map_err(|e| format!("reconnect after schema setup: {e}"))?;
    Ok(pool)
}

async fn cleanup(pool: &PgPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        r#"
        DELETE FROM ipam_ipaddress
        WHERE id >= '00000000-0000-7002-8000-000000001000'::uuid
          AND id <  '00000000-0000-7002-8000-000000002000'::uuid
        "#,
    )
    .execute(pool)
    .await
    .map_err(|e| format!("cleanup test IP addresses: {e}"))?;

    sqlx::query("DELETE FROM ipam_prefix WHERE vrf_id = $1")
        .bind(TEST_VRF_ID)
        .execute(pool)
        .await
        .map_err(|e| format!("cleanup test prefixes: {e}"))?;

    sqlx::query("DELETE FROM ipam_vrf WHERE id = $1")
        .bind(TEST_VRF_ID)
        .execute(pool)
        .await
        .map_err(|e| format!("cleanup test VRF: {e}"))?;
    Ok(())
}

async fn ensure_test_vrf(pool: &PgPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        r#"
        INSERT INTO ipam_vrf (
            id, custom_field_data, name, rd, enforce_unique, description, comments, tags
        )
        VALUES (
            $1, '{}'::jsonb, 'g002-prefix-tree-test', NULL, false,
            'G002 prefix tree read-model test VRF', '', '{}'
        )
        "#,
    )
    .bind(TEST_VRF_ID)
    .execute(pool)
    .await
    .map_err(|e| format!("insert test VRF: {e}"))?;
    Ok(())
}

async fn insert_prefix(
    pool: &PgPool,
    id: Uuid,
    prefix: &str,
    status: &str,
    mark_utilized: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        r#"
        INSERT INTO ipam_prefix (
            id, custom_field_data, prefix, status, is_pool, description,
            vrf_id, _children, _depth, mark_utilized, comments, tags
        )
        VALUES (
            $1, '{}'::jsonb, $2::cidr, $3, false, 'G002 prefix tree read-model test',
            $4, 0, 0, $5, '', '{}'
        )
        "#,
    )
    .bind(id)
    .bind(prefix)
    .bind(status)
    .bind(TEST_VRF_ID)
    .bind(mark_utilized)
    .execute(pool)
    .await
    .map_err(|e| format!("insert test prefix: {e}"))?;
    Ok(())
}

async fn seed_tree_fixture(pool: &PgPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    cleanup(pool).await?;
    ensure_test_vrf(pool).await?;

    insert_prefix(pool, ROOT_ID, "10.250.0.0/16", "container", false).await?;
    insert_prefix(pool, PARENT_ID, "10.250.10.0/24", "active", false).await?;
    insert_prefix(pool, CHILD_A_ID, "10.250.10.0/25", "active", false).await?;
    insert_prefix(pool, CHILD_B_ID, "10.250.10.128/25", "reserved", false).await?;
    insert_prefix(pool, MARK_UTILIZED_ID, "10.250.20.0/30", "active", true).await?;
    insert_prefix(pool, IPV6_ROOT_ID, "fd00:250::/64", "active", false).await?;

    sqlx::query(
        r#"
        INSERT INTO ipam_ipaddress (
            id, custom_field_data, address, status, dns_name, description, vrf_id, comments, tags
        )
        VALUES
            ('00000000-0000-7002-8000-000000001001'::uuid, '{}'::jsonb,
             '10.250.10.1/32'::inet, 'active', '', 'G002 prefix tree read-model test', $1, '', '{}'),
            ('00000000-0000-7002-8000-000000001002'::uuid, '{}'::jsonb,
             '10.250.10.2/32'::inet, 'active', '', 'G002 prefix tree read-model test', $1, '', '{}'),
            ('00000000-0000-7002-8000-000000001003'::uuid, '{}'::jsonb,
             'fd00:250::1/128'::inet, 'active', '', 'G002 prefix tree read-model test', $1, '', '{}')
        "#,
    )
    .bind(TEST_VRF_ID)
    .execute(pool)
    .await
    .map_err(|e| format!("insert utilization IP addresses: {e}"))?;
    Ok(())
}

async fn seed_limit_fixture(pool: &PgPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    cleanup(pool).await?;
    ensure_test_vrf(pool).await?;
    insert_prefix(pool, LIMIT_ROOT_ID, "10.251.0.0/16", "container", false).await?;

    for ordinal in 0u128..510u128 {
        let id = Uuid::from_u128(0x00000000_0000_7002_8000_000000010000 + ordinal);
        let third_octet = 128 + (ordinal / 256);
        let fourth_octet = ordinal % 256;
        let prefix = format!("10.251.{third_octet}.{fourth_octet}/32");
        insert_prefix(pool, id, &prefix, "active", false).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires database (run with local dev Postgres)"]
async fn prefix_tree_read_model_shapes_filters_and_utilization_from_sql()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    seed_tree_fixture(&pool).await?;

    let repository = SqlxPrefixRepository::new(pool.clone());
    let tree = repository
        .list_tree(
            PrefixTreeFilter {
                root_id: Some(ROOT_ID),
                vrf_id: vec![TEST_VRF_ID],
                family: Some(4),
                ..Default::default()
            },
            Some(50),
        )
        .await
        .expect("list IPv4 prefix tree");

    assert_eq!(tree.limit, 50);
    assert_eq!(tree.applied.root_id, Some(ROOT_ID));
    assert_eq!(tree.applied.root_prefix.as_deref(), Some("10.250.0.0/16"));
    assert_eq!(tree.total, 5);

    let nodes: HashMap<Uuid, _> = tree.nodes.iter().map(|node| (node.id, node)).collect();
    let root = nodes.get(&ROOT_ID).expect("root node");
    assert_eq!(root.parent_id, None);
    assert_eq!(root.parent_prefix, None);
    assert_eq!(root.depth, 0);
    assert_eq!(root.child_count, 2);
    assert_eq!(root.breadcrumbs, vec!["10.250.0.0/16"]);

    let parent = nodes.get(&PARENT_ID).expect("parent node");
    assert_eq!(parent.parent_id, Some(ROOT_ID));
    assert_eq!(parent.parent_prefix.as_deref(), Some("10.250.0.0/16"));
    assert_eq!(parent.depth, 1);
    assert_eq!(parent.child_count, 2);
    assert_eq!(
        parent.breadcrumbs,
        vec!["10.250.0.0/16".to_string(), "10.250.10.0/24".to_string()]
    );

    let child = nodes.get(&CHILD_A_ID).expect("child node");
    assert_eq!(child.parent_id, Some(PARENT_ID));
    assert_eq!(child.depth, 2);
    assert_eq!(child.child_count, 0);
    assert_eq!(child.utilized_addresses, 2);
    assert_eq!(child.usable_addresses, Some(126));

    let marked = nodes.get(&MARK_UTILIZED_ID).expect("mark-utilized node");
    assert!(marked.mark_utilized);
    assert_eq!(marked.usable_addresses, Some(2));
    assert_eq!(marked.utilization_percent, Some(100.0));

    let active_only = repository
        .list_tree(
            PrefixTreeFilter {
                root_id: Some(ROOT_ID),
                vrf_id: vec![TEST_VRF_ID],
                family: Some(4),
                status: vec!["active".to_string()],
                ..Default::default()
            },
            Some(50),
        )
        .await
        .expect("list active IPv4 prefix tree");
    assert_eq!(active_only.total, 3);
    assert!(active_only.nodes.iter().all(|node| node.status == "active"));

    let root_prefix_filtered = repository
        .list_tree(
            PrefixTreeFilter {
                root_prefix: Some("10.250.10.0/24".to_string()),
                vrf_id: vec![TEST_VRF_ID],
                family: Some(4),
                ..Default::default()
            },
            Some(50),
        )
        .await
        .expect("list root-prefix-filtered tree");
    assert_eq!(root_prefix_filtered.total, 3);
    assert!(
        root_prefix_filtered
            .nodes
            .iter()
            .all(|node| node.prefix.starts_with("10.250.10."))
    );

    let ipv6_only = repository
        .list_tree(
            PrefixTreeFilter {
                root_id: Some(IPV6_ROOT_ID),
                vrf_id: vec![TEST_VRF_ID],
                family: Some(6),
                ..Default::default()
            },
            Some(50),
        )
        .await
        .expect("list IPv6 prefix tree");
    assert_eq!(ipv6_only.total, 1);
    let ipv6 = ipv6_only.nodes.first().expect("IPv6 node");
    assert_eq!(ipv6.family, 6);
    assert_eq!(ipv6.usable_addresses, None);
    assert_eq!(ipv6.utilization_percent, None);

    cleanup(&pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires database (run with local dev Postgres)"]
async fn prefix_tree_read_model_caps_requested_limit_at_500()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    seed_limit_fixture(&pool).await?;

    let repository = SqlxPrefixRepository::new(pool.clone());
    let tree = repository
        .list_tree(
            PrefixTreeFilter {
                root_id: Some(LIMIT_ROOT_ID),
                vrf_id: vec![TEST_VRF_ID],
                family: Some(4),
                ..Default::default()
            },
            Some(999),
        )
        .await
        .expect("list capped prefix tree");

    assert_eq!(tree.limit, 500);
    assert_eq!(tree.total, 511);
    assert_eq!(tree.nodes.len(), 500);

    cleanup(&pool).await?;
    Ok(())
}
