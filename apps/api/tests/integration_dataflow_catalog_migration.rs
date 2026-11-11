//! PIP-211 proof of the `0016 -> 0017` dataflow catalog upgrade.
//!
//! Runs against a disposable database created from `PIPING_DATABASE__URL` and
//! force-dropped afterwards. The test seeds the old 0001 shape, snapshots it,
//! applies 0017 once, and compares a normalized relation (flow identity/traffic,
//! selector kind/mode/typed reference/side/position, selected-IP associations).
//! It also proves the migration is fail-closed on an inconsistent pre-state and
//! that the same fixture upgraded through the `sqlx` migrator chain yields the
//! same normalized relation.
//!
//! Policy *rule* projection is not asserted here; the Rust policy suite owns it.
//! Only typed-target and selected-address equivalence at row level is checked.
use std::borrow::Cow;

use serde_json::{Value, json};
use sqlx::{
    PgPool, Postgres,
    migrate::{MigrateDatabase, Migrator},
};
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const MIGRATIONS: [&str; 16] = [
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_interface_networking.sql"),
    include_str!("../migrations/0003_service_port_mappings.sql"),
    include_str!("../migrations/0004_fhrp_auth_key_secret.sql"),
    include_str!("../migrations/0005_module_bay_types.sql"),
    include_str!("../migrations/0006_canonical_tag_ids.sql"),
    include_str!("../migrations/0007_cooling_hardware.sql"),
    include_str!("../migrations/0008_module_type_profile_scope.sql"),
    include_str!("../migrations/0009_circuit_group_assignments.sql"),
    include_str!("../migrations/0010_vlan_translation.sql"),
    include_str!("../migrations/0011_termination_graph.sql"),
    include_str!("../migrations/0012_inventory_transfer_run.sql"),
    include_str!("../migrations/0013_component_template_instantiation.sql"),
    include_str!("../migrations/0014_tag_object_type_scope.sql"),
    include_str!("../migrations/0015_device_bay_template_enabled.sql"),
    include_str!("../migrations/0016_project_workspace.sql"),
];
const CATALOG: &str = include_str!("../migrations/0017_dataflow_catalog.sql");

/// Synthetic 0016 fixture: every endpoint shape the migration must copy, with
/// deterministic ids. All inserts run in one transaction because the old
/// selected-IP invariant is a deferred constraint trigger.
const FIXTURE: &str = r#"
INSERT INTO dcim_region (id, custom_field_data, name, slug, description, lft, rght, tree_id, level, comments)
VALUES ('02110000-0000-7000-8000-000000000001', '{}', 'parent-region', 'parent-region', '', 0, 0, 0, 0, '');
INSERT INTO dcim_region (id, custom_field_data, name, slug, description, lft, rght, tree_id, level, comments, parent_id)
VALUES ('02110000-0000-7000-8000-000000000002', '{}', 'child-region', 'child-region', '', 0, 0, 0, 1, '',
        '02110000-0000-7000-8000-000000000001');
INSERT INTO dcim_site (id, custom_field_data, name, slug, status, facility, description,
                       physical_address, shipping_address, comments, region_id)
VALUES ('02110000-0000-7000-8000-000000000003', '{}', 'fixture-site', 'fixture-site', 'active', '', '', '', '', '',
        '02110000-0000-7000-8000-000000000002');
INSERT INTO dcim_devicerole (id, custom_field_data, name, slug, color, vm_role, description,
                             level, lft, rght, tree_id, comments)
VALUES ('02110000-0000-7000-8000-000000000004', '{}', 'fixture-role', 'fixture-role', '9e9e9e', false, '',
        0, 0, 0, 0, '');
INSERT INTO dcim_manufacturer (id, custom_field_data, name, slug, description, comments)
VALUES ('02110000-0000-7000-8000-000000000005', '{}', 'fixture-mfr', 'fixture-mfr', '', '');
INSERT INTO dcim_devicetype (id, custom_field_data, model, slug, manufacturer_id, part_number,
                             u_height, is_full_depth, front_image, rear_image, comments, description,
                             console_port_template_count, console_server_port_template_count,
                             power_port_template_count, power_outlet_template_count,
                             interface_template_count, front_port_template_count,
                             rear_port_template_count, device_bay_template_count,
                             module_bay_template_count, inventory_item_template_count,
                             device_count, exclude_from_utilization)
VALUES ('02110000-0000-7000-8000-000000000006', '{}', 'fixture-model', 'fixture-model',
        '02110000-0000-7000-8000-000000000005', '', 1.0, false, '', '', '', '',
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false);
INSERT INTO dcim_device (id, custom_field_data, name, serial, status, comments, description,
                         role_id, device_type_id, site_id, console_port_count,
                         console_server_port_count, power_port_count, power_outlet_count,
                         interface_count, front_port_count, rear_port_count, device_bay_count,
                         module_bay_count, inventory_item_count)
VALUES ('02110000-0000-7000-8000-000000000007', '{}', 'fixture-device-a', '', 'active', '', '',
        '02110000-0000-7000-8000-000000000004', '02110000-0000-7000-8000-000000000006',
        '02110000-0000-7000-8000-000000000003', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
INSERT INTO dcim_device (id, custom_field_data, name, serial, status, comments, description,
                         role_id, device_type_id, site_id, console_port_count,
                         console_server_port_count, power_port_count, power_outlet_count,
                         interface_count, front_port_count, rear_port_count, device_bay_count,
                         module_bay_count, inventory_item_count)
VALUES ('02110000-0000-7000-8000-000000000008', '{}', 'fixture-device-b', '', 'active', '', '',
        '02110000-0000-7000-8000-000000000004', '02110000-0000-7000-8000-000000000006',
        '02110000-0000-7000-8000-000000000003', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
INSERT INTO virtualization_virtualmachine (id, custom_field_data, name, status, comments, description,
                                           interface_count, virtual_disk_count, serial, start_on_boot,
                                           site_id, role_id)
VALUES ('02110000-0000-7000-8000-000000000009', '{}', 'fixture-vm', 'active', '', '', 0, 0, '', '',
        '02110000-0000-7000-8000-000000000003', '02110000-0000-7000-8000-000000000004');
INSERT INTO ipam_vrf (id, custom_field_data, name, enforce_unique, description, comments)
VALUES ('02110000-0000-7000-8000-00000000000a', '{}', 'fixture-red', false, '', '');
INSERT INTO ipam_vrf (id, custom_field_data, name, enforce_unique, description, comments)
VALUES ('02110000-0000-7000-8000-00000000000b', '{}', 'fixture-blue', false, '', '');
INSERT INTO ipam_ipaddress (id, custom_field_data, address, status, dns_name, description, comments, vrf_id)
VALUES ('02110000-0000-7000-8000-00000000000c', '{}', '192.0.2.10/24', 'active', '', '', '',
        '02110000-0000-7000-8000-00000000000a');
INSERT INTO ipam_ipaddress (id, custom_field_data, address, status, dns_name, description, comments, vrf_id)
VALUES ('02110000-0000-7000-8000-00000000000d', '{}', '192.0.2.11/24', 'active', '', '', '',
        '02110000-0000-7000-8000-00000000000a');
INSERT INTO ipam_ipaddress (id, custom_field_data, address, status, dns_name, description, comments, vrf_id)
VALUES ('02110000-0000-7000-8000-00000000000e', '{}', '198.51.100.10/24', 'active', '', '', '',
        '02110000-0000-7000-8000-00000000000b');
INSERT INTO ipam_ipaddress (id, custom_field_data, address, status, dns_name, description, comments, vrf_id)
VALUES ('02110000-0000-7000-8000-00000000000f', '{}', '192.0.2.20/24', 'active', '', '', '',
        '02110000-0000-7000-8000-00000000000a');
INSERT INTO ipam_ipaddress (id, custom_field_data, address, status, dns_name, description, comments, vrf_id)
VALUES ('02110000-0000-7000-8000-000000000010', '{}', '2001:db8::20/64', 'active', '', '', '',
        '02110000-0000-7000-8000-00000000000a');
INSERT INTO ipam_prefix (id, custom_field_data, prefix, status, is_pool, description, _children,
                         _depth, mark_utilized, comments, vrf_id)
VALUES ('02110000-0000-7000-8000-000000000011', '{}', '198.51.100.0/24', 'active', false, '', 0, 0,
        false, '', '02110000-0000-7000-8000-00000000000b');
INSERT INTO ipam_service (id, custom_field_data, name, description, comments, parent_object_id,
                          parent_object_type_id, port_mappings)
VALUES ('02110000-0000-7000-8000-000000000012', '{}', 'fixture-service', '', '',
        '02110000-0000-7000-8000-000000000007',
        (SELECT id FROM inventory_content_type WHERE app_label = 'dcim' AND model = 'device'),
        ARRAY['tcp/80']);
INSERT INTO ipam_service_ipaddresses (service_id, ipaddress_id)
VALUES ('02110000-0000-7000-8000-000000000012', '02110000-0000-7000-8000-00000000000d');
INSERT INTO tenancy_tenant (id, custom_field_data, name, slug, description, comments)
VALUES ('02110000-0000-7000-8000-000000000013', '{}', 'fixture-tenant', 'fixture-tenant', '', '');
UPDATE dcim_device SET primary_ip4_id = '02110000-0000-7000-8000-00000000000c'
WHERE id = '02110000-0000-7000-8000-000000000007';
UPDATE dcim_device SET primary_ip4_id = '02110000-0000-7000-8000-00000000000e'
WHERE id = '02110000-0000-7000-8000-000000000008';
UPDATE virtualization_virtualmachine
SET primary_ip4_id = '02110000-0000-7000-8000-00000000000f',
    primary_ip6_id = '02110000-0000-7000-8000-000000000010'
WHERE id = '02110000-0000-7000-8000-000000000009';

INSERT INTO inventory_dataflow (id, name, description, status, transport, allow_cross_vrf,
                                destination_ports, source_kind, destination_kind, source_mode,
                                destination_mode, source_device_id)
VALUES ('02110000-0000-7000-8000-000000000101', 'alpha-draft', 'draft flow', 'draft', 'any', false,
        NULL, 'dcim.device', 'all', 'default', 'default',
        '02110000-0000-7000-8000-000000000007');
INSERT INTO inventory_dataflow (id, name, description, status, transport, allow_cross_vrf,
                                destination_ports, source_kind, destination_kind, source_mode,
                                destination_mode, source_device_id, destination_vm_id)
VALUES ('02110000-0000-7000-8000-000000000102', 'beta-active', 'active flow', 'active', 'tcp', true,
        ARRAY[443], 'dcim.device', 'virtualization.virtualmachine', 'selected', 'selected',
        '02110000-0000-7000-8000-000000000007', '02110000-0000-7000-8000-000000000009');
INSERT INTO inventory_dataflow_selected_ip (dataflow_id, side, ipaddress_id) VALUES
    ('02110000-0000-7000-8000-000000000102', 'source', '02110000-0000-7000-8000-00000000000c'),
    ('02110000-0000-7000-8000-000000000102', 'source', '02110000-0000-7000-8000-00000000000d'),
    ('02110000-0000-7000-8000-000000000102', 'destination', '02110000-0000-7000-8000-00000000000f'),
    ('02110000-0000-7000-8000-000000000102', 'destination', '02110000-0000-7000-8000-000000000010');
INSERT INTO inventory_dataflow (id, name, description, status, transport, allow_cross_vrf,
                                destination_ports, source_kind, destination_kind, source_mode,
                                destination_mode, source_service_id)
VALUES ('02110000-0000-7000-8000-000000000103', 'gamma-disabled', 'disabled flow', 'disabled', 'udp',
        false, ARRAY[53], 'ipam.service', 'internet', 'default', 'default',
        '02110000-0000-7000-8000-000000000012');
INSERT INTO inventory_dataflow (id, name, description, status, transport, allow_cross_vrf,
                                destination_ports, source_kind, destination_kind, source_mode,
                                destination_mode, destination_device_id)
VALUES ('02110000-0000-7000-8000-000000000104', 'delta-all', 'all to device', 'active', 'sctp',
        false, NULL, 'all', 'dcim.device', 'default', 'all',
        '02110000-0000-7000-8000-000000000008');
INSERT INTO inventory_dataflow (id, name, description, status, transport, allow_cross_vrf,
                                destination_ports, source_kind, destination_kind, source_mode,
                                destination_mode, source_region_id, destination_prefix_id)
VALUES ('02110000-0000-7000-8000-000000000105', 'epsilon-scope', 'scope to prefix', 'draft', 'any',
        true, ARRAY[8080, 8081], 'dcim.region', 'ipam.prefix', 'default', 'default',
        '02110000-0000-7000-8000-000000000002', '02110000-0000-7000-8000-000000000011');
INSERT INTO inventory_dataflow (id, name, description, status, transport, allow_cross_vrf,
                                destination_ports, source_kind, destination_kind, source_mode,
                                destination_mode, source_ipaddress_id, destination_tenant_id)
VALUES ('02110000-0000-7000-8000-000000000106', 'zeta-cross-vrf', 'cross vrf address', 'active',
        'tcp', false, NULL, 'ipam.ipaddress', 'tenancy.tenant', 'default', 'default',
        '02110000-0000-7000-8000-00000000000e', '02110000-0000-7000-8000-000000000013');
"#;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct FlowTuple {
    id: Uuid,
    name: String,
    description: String,
    status: String,
    transport: String,
    destination_ports: Option<Vec<i32>>,
    allow_cross_vrf: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SelectorTuple {
    dataflow_id: Uuid,
    side: String,
    position: i16,
    object_type: String,
    address_mode: String,
    tag_object_type: Option<String>,
    object_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IpTuple {
    dataflow_id: Uuid,
    side: String,
    ipaddress_id: Uuid,
}

/// Normalized pre/post relation. Generated selector ids are intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Relation {
    flows: Vec<FlowTuple>,
    selectors: Vec<SelectorTuple>,
    ips: Vec<IpTuple>,
}

impl Relation {
    fn lines(&self) -> Vec<String> {
        let mut lines =
            Vec::with_capacity(self.flows.len() + self.selectors.len() + self.ips.len());
        for flow in &self.flows {
            lines.push(format!(
                "flow|{}|{}|{}|{}|{}|{:?}|{}",
                flow.id,
                flow.name,
                flow.description,
                flow.status,
                flow.transport,
                flow.destination_ports,
                flow.allow_cross_vrf
            ));
        }
        for selector in &self.selectors {
            lines.push(format!(
                "selector|{}|{}|{}|{}|{}|{:?}|{:?}",
                selector.dataflow_id,
                selector.side,
                selector.position,
                selector.object_type,
                selector.address_mode,
                selector.tag_object_type,
                selector.object_id
            ));
        }
        for ip in &self.ips {
            lines.push(format!(
                "ip|{}|{}|{}",
                ip.dataflow_id, ip.side, ip.ipaddress_id
            ));
        }
        lines.sort();
        lines
    }

    fn hash(&self) -> u64 {
        // FNV-1a over the canonical lines; equality of `lines` is the real proof,
        // the hash is a compact receipt value.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for line in self.lines() {
            for byte in line.as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash ^= 0xff;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

const SELECTOR_SQL: &str = "SELECT dataflow_id, side, position, object_type, address_mode, \
    tag_object_type, device_id, vm_id, service_id, ipaddress_id, prefix_id, iprange_id, tag_id, \
    addressgroup_id, region_id, site_id, sitegroup_id, devicerole_id, tenant_id, tenantgroup_id, \
    platform_id, manufacturer_id, devicetype_id, rack_id, location_id, cluster_id, clustergroup_id, \
    clustertype_id FROM inventory_dataflow_selector WHERE dataflow_id IS NOT NULL \
    ORDER BY dataflow_id, side, position";

const POST_FLOW_SQL: &str = "SELECT id, name, description, status, transport, destination_ports, \
    allow_cross_vrf, comments, group_id, application_id, owner_id, tenant_id, tags, \
    custom_field_data FROM inventory_dataflow ORDER BY id";

#[derive(Debug, sqlx::FromRow)]
struct PostFlow {
    id: Uuid,
    name: String,
    description: String,
    status: String,
    transport: String,
    destination_ports: Option<Vec<i32>>,
    allow_cross_vrf: bool,
    comments: String,
    group_id: Option<Uuid>,
    application_id: Option<Uuid>,
    owner_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    tags: Vec<String>,
    custom_field_data: Value,
}

#[derive(Debug, sqlx::FromRow)]
struct PostSelectorRow {
    dataflow_id: Uuid,
    side: String,
    position: i16,
    object_type: String,
    address_mode: String,
    tag_object_type: Option<String>,
    device_id: Option<Uuid>,
    vm_id: Option<Uuid>,
    service_id: Option<Uuid>,
    ipaddress_id: Option<Uuid>,
    prefix_id: Option<Uuid>,
    iprange_id: Option<Uuid>,
    tag_id: Option<Uuid>,
    addressgroup_id: Option<Uuid>,
    region_id: Option<Uuid>,
    site_id: Option<Uuid>,
    sitegroup_id: Option<Uuid>,
    devicerole_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    tenantgroup_id: Option<Uuid>,
    platform_id: Option<Uuid>,
    manufacturer_id: Option<Uuid>,
    devicetype_id: Option<Uuid>,
    rack_id: Option<Uuid>,
    location_id: Option<Uuid>,
    cluster_id: Option<Uuid>,
    clustergroup_id: Option<Uuid>,
    clustertype_id: Option<Uuid>,
}

impl PostSelectorRow {
    fn typed_reference(&self) -> Option<Uuid> {
        match self.object_type.as_str() {
            "dcim.device" => self.device_id,
            "virtualization.virtualmachine" => self.vm_id,
            "ipam.service" => self.service_id,
            "ipam.ipaddress" => self.ipaddress_id,
            "ipam.prefix" => self.prefix_id,
            "ipam.iprange" => self.iprange_id,
            "extras.tag" => self.tag_id,
            "inventory.addressgroup" => self.addressgroup_id,
            "dcim.region" => self.region_id,
            "dcim.site" => self.site_id,
            "dcim.sitegroup" => self.sitegroup_id,
            "dcim.devicerole" => self.devicerole_id,
            "tenancy.tenant" => self.tenant_id,
            "tenancy.tenantgroup" => self.tenantgroup_id,
            "dcim.platform" => self.platform_id,
            "dcim.manufacturer" => self.manufacturer_id,
            "dcim.devicetype" => self.devicetype_id,
            "dcim.rack" => self.rack_id,
            "dcim.location" => self.location_id,
            "virtualization.cluster" => self.cluster_id,
            "virtualization.clustergroup" => self.clustergroup_id,
            "virtualization.clustertype" => self.clustertype_id,
            _ => None,
        }
    }
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL 18 database with CREATEDB"]
async fn catalog_upgrade_preserves_relations_and_refuses_inconsistent_pre_state() -> TestResult {
    let upgrade_url = create_database("pip211mig_up").await?;
    let fresh_url = create_database("pip211mig_fresh").await?;

    let outcome = async {
        let upgraded = run_upgrade(&upgrade_url).await?;
        let fresh = run_fresh(&fresh_url).await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((upgraded, fresh))
    }
    .await;

    let cleanup_upgrade = <Postgres as MigrateDatabase>::force_drop_database(&upgrade_url).await;
    let cleanup_fresh = <Postgres as MigrateDatabase>::force_drop_database(&fresh_url).await;

    let (upgraded, fresh) = outcome?;
    cleanup_upgrade?;
    cleanup_fresh?;

    assert_eq!(
        upgraded.hash(),
        fresh.hash(),
        "the upgraded relation must equal the fresh 0001..0017 chain relation"
    );
    assert_eq!(
        upgraded.lines(),
        fresh.lines(),
        "normalized relation (flow tuples, selector kind/mode/ref, selected IP sets) diverged"
    );
    Ok(())
}

async fn create_database(prefix: &str) -> TestResult<String> {
    let mut url = url::Url::parse(&std::env::var("PIPING_DATABASE__URL")?)?;
    url.set_path(&format!("/{prefix}_{}", Uuid::now_v7().simple()));
    let url = url.to_string();
    <Postgres as MigrateDatabase>::create_database(&url).await?;
    Ok(url)
}

async fn run_upgrade(url: &str) -> TestResult<Relation> {
    let pool = PgPool::connect(url).await?;
    for migration in MIGRATIONS {
        sqlx::raw_sql(migration).execute(&pool).await?;
    }
    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(FIXTURE).execute(&mut *transaction).await?;
    transaction.commit().await?;

    let pre = pre_state(&pool).await?;
    assert_eq!(pre.flows.len(), 6, "fixture must seed six flows");
    assert_eq!(pre.ips.len(), 4, "fixture must seed four selected IP rows");

    // --- fail-closed path -------------------------------------------------
    // A `selected` endpoint without selected IPs is only creatable while the old
    // deferred trigger is disabled. 0017's own conservation block must refuse it.
    sqlx::query(
        "ALTER TABLE inventory_dataflow DISABLE TRIGGER inventory_dataflow_selection_consistency",
    )
    .execute(&pool)
    .await?;
    let alpha: Uuid =
        sqlx::query_scalar("SELECT id FROM inventory_dataflow WHERE name = 'alpha-draft'")
            .fetch_one(&pool)
            .await?;
    sqlx::query("UPDATE inventory_dataflow SET source_mode = 'selected' WHERE id = $1")
        .bind(alpha)
        .execute(&pool)
        .await?;
    let invalid_pre = pre_state(&pool).await?;
    assert_ne!(invalid_pre, pre, "the invalid pre-state must be observable");

    let mut transaction = pool.begin().await?;
    let attempt = sqlx::raw_sql(CATALOG).execute(&mut *transaction).await;
    assert!(
        attempt.is_err(),
        "0017 must refuse a selected endpoint without selected IPs"
    );
    transaction.rollback().await?;

    assert!(
        column_exists(&pool, "inventory_dataflow", "source_kind").await?,
        "a failed upgrade must not drop the old selector column"
    );
    assert!(
        table_exists(&pool, "inventory_dataflow_selected_ip").await?,
        "a failed upgrade must not drop the old junction"
    );
    assert!(
        !table_exists(&pool, "inventory_dataflow_selector").await?,
        "a failed upgrade must not leave partial DDL"
    );
    assert!(
        !table_exists(&pool, "inventory_application").await?,
        "a failed upgrade must not leave partial catalog tables"
    );
    assert_eq!(
        pre_state(&pool).await?,
        invalid_pre,
        "a failed upgrade must not rewrite flow rows"
    );

    sqlx::query(
        "ALTER TABLE inventory_dataflow ENABLE TRIGGER inventory_dataflow_selection_consistency",
    )
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE inventory_dataflow SET source_mode = 'default' WHERE id = $1")
        .bind(alpha)
        .execute(&pool)
        .await?;
    assert_eq!(
        pre_state(&pool).await?,
        pre,
        "the restored fixture must match the original snapshot"
    );

    // --- the real upgrade -------------------------------------------------
    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(CATALOG).execute(&mut *transaction).await?;
    transaction.commit().await?;

    let post = post_state(&pool).await?;
    assert_upgraded(&pool, &pre, &post).await?;
    pool.close().await;
    Ok(post)
}

/// Same fixture, migrated through `sqlx` instead of `raw_sql`: apply 0001..0016
/// with a truncated migrator, seed the fixture, then let the full migrator run
/// the pending 0017.
async fn run_fresh(url: &str) -> TestResult<Relation> {
    let pool = PgPool::connect(url).await?;
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut baseline = Migrator::new(directory.as_path()).await?;
    baseline.migrations = Cow::Owned(
        baseline
            .migrations
            .iter()
            .filter(|migration| migration.version <= 16)
            .cloned()
            .collect::<Vec<_>>(),
    );
    baseline.run(&pool).await?;

    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(FIXTURE).execute(&mut *transaction).await?;
    transaction.commit().await?;
    let pre = pre_state(&pool).await?;

    Migrator::new(directory.as_path()).await?.run(&pool).await?;
    let post = post_state(&pool).await?;
    assert_eq!(
        pre.lines(),
        post.lines(),
        "the migrator chain must not change the fixture relation"
    );
    pool.close().await;
    Ok(post)
}

async fn pre_state(pool: &PgPool) -> TestResult<Relation> {
    let rows: Vec<(Uuid, Value)> =
        sqlx::query_as("SELECT id, to_jsonb(f) FROM inventory_dataflow f ORDER BY id")
            .fetch_all(pool)
            .await?;
    let mut flows = Vec::new();
    let mut selectors = Vec::new();
    for (id, row) in rows {
        flows.push(FlowTuple {
            id,
            name: text(&row, "name")?,
            description: text(&row, "description")?,
            status: text(&row, "status")?,
            transport: text(&row, "transport")?,
            destination_ports: ports(&row)?,
            allow_cross_vrf: row["allow_cross_vrf"]
                .as_bool()
                .ok_or("allow_cross_vrf is not a boolean")?,
        });
        for (side, kind_key, mode_key) in [
            ("source", "source_kind", "source_mode"),
            ("destination", "destination_kind", "destination_mode"),
        ] {
            let object_type = text(&row, kind_key)?;
            selectors.push(SelectorTuple {
                dataflow_id: id,
                side: side.to_string(),
                position: 1,
                object_type: object_type.clone(),
                address_mode: text(&row, mode_key)?,
                tag_object_type: None,
                object_id: side_reference(&row, side, &object_type)?,
            });
        }
    }
    let ips = sqlx::query_as::<_, (Uuid, String, Uuid)>(
        "SELECT dataflow_id, side, ipaddress_id FROM inventory_dataflow_selected_ip \
         ORDER BY dataflow_id, side, ipaddress_id",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(dataflow_id, side, ipaddress_id)| IpTuple {
        dataflow_id,
        side,
        ipaddress_id,
    })
    .collect();
    Ok(Relation {
        flows,
        selectors,
        ips,
    })
}

async fn post_state(pool: &PgPool) -> TestResult<Relation> {
    let flow_rows: Vec<PostFlow> = sqlx::query_as(POST_FLOW_SQL).fetch_all(pool).await?;
    let selector_rows: Vec<PostSelectorRow> = sqlx::query_as(SELECTOR_SQL).fetch_all(pool).await?;
    let ips = sqlx::query_as::<_, (Uuid, String, Uuid)>(
        "SELECT s.dataflow_id, s.side, ip.ipaddress_id \
         FROM inventory_dataflow_selector_ip ip \
         JOIN inventory_dataflow_selector s ON s.id = ip.selector_id \
         WHERE s.dataflow_id IS NOT NULL ORDER BY 1, 2, 3",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(dataflow_id, side, ipaddress_id)| IpTuple {
        dataflow_id,
        side,
        ipaddress_id,
    })
    .collect();
    Ok(Relation {
        flows: flow_rows
            .into_iter()
            .map(|flow| FlowTuple {
                id: flow.id,
                name: flow.name,
                description: flow.description,
                status: flow.status,
                transport: flow.transport,
                destination_ports: flow.destination_ports,
                allow_cross_vrf: flow.allow_cross_vrf,
            })
            .collect(),
        selectors: selector_rows
            .iter()
            .map(|row| SelectorTuple {
                dataflow_id: row.dataflow_id,
                side: row.side.clone(),
                position: row.position,
                object_type: row.object_type.clone(),
                address_mode: row.address_mode.clone(),
                tag_object_type: row.tag_object_type.clone(),
                object_id: row.typed_reference(),
            })
            .collect(),
        ips,
    })
}

async fn assert_upgraded(pool: &PgPool, pre: &Relation, post: &Relation) -> TestResult {
    // Exactly two selectors per flow, one per side, at position 1.
    let mut per_flow: std::collections::BTreeMap<Uuid, Vec<&SelectorTuple>> =
        std::collections::BTreeMap::new();
    for selector in &post.selectors {
        per_flow
            .entry(selector.dataflow_id)
            .or_default()
            .push(selector);
    }
    assert_eq!(
        per_flow.len(),
        pre.flows.len(),
        "every flow must keep its selector set"
    );
    for flow in &pre.flows {
        let rows = per_flow
            .get(&flow.id)
            .ok_or("flow lost every selector after upgrade")?;
        assert_eq!(
            rows.len(),
            2,
            "flow {} must have exactly two selectors",
            flow.id
        );
        assert_eq!(
            rows.iter().filter(|row| row.side == "source").count(),
            1,
            "flow {} must have one source selector",
            flow.id
        );
        assert_eq!(
            rows.iter().filter(|row| row.side == "destination").count(),
            1,
            "flow {} must have one destination selector",
            flow.id
        );
        for row in rows {
            assert_eq!(
                row.position, 1,
                "position must stay 1 for a single endpoint"
            );
            assert!(
                row.tag_object_type.is_none(),
                "old flows cannot gain a tag scope"
            );
        }
    }

    // New catalog metadata defaults; no old flow gains an owner or container.
    let flow_rows: Vec<PostFlow> = sqlx::query_as(POST_FLOW_SQL).fetch_all(pool).await?;
    for flow in &flow_rows {
        assert_eq!(
            flow.comments, "",
            "flow {} comments must default empty",
            flow.id
        );
        assert!(
            flow.group_id.is_none(),
            "flow {} must not gain a group",
            flow.id
        );
        assert!(
            flow.application_id.is_none(),
            "flow {} must not gain an application",
            flow.id
        );
        assert!(
            flow.owner_id.is_none(),
            "flow {} must not gain an owner",
            flow.id
        );
        assert!(
            flow.tenant_id.is_none(),
            "flow {} must not gain a tenant",
            flow.id
        );
        assert!(
            flow.tags.is_empty(),
            "flow {} tags must stay empty",
            flow.id
        );
        assert_eq!(
            flow.custom_field_data,
            json!({}),
            "flow {} custom_field_data must default empty",
            flow.id
        );
    }

    // Row-level policy semantics for every old active flow: the new selector
    // mapping yields the same typed targets and the same selected addresses.
    for flow in pre.flows.iter().filter(|flow| flow.status == "active") {
        let expected: std::collections::BTreeSet<_> = pre
            .selectors
            .iter()
            .filter(|selector| selector.dataflow_id == flow.id)
            .cloned()
            .collect();
        let actual: std::collections::BTreeSet<_> = post
            .selectors
            .iter()
            .filter(|selector| selector.dataflow_id == flow.id)
            .cloned()
            .collect();
        assert_eq!(actual, expected, "active flow {} selector mapping", flow.id);
        let expected_ips: std::collections::BTreeSet<_> = pre
            .ips
            .iter()
            .filter(|ip| ip.dataflow_id == flow.id)
            .cloned()
            .collect();
        let actual_ips: std::collections::BTreeSet<_> = post
            .ips
            .iter()
            .filter(|ip| ip.dataflow_id == flow.id)
            .cloned()
            .collect();
        assert_eq!(
            actual_ips, expected_ips,
            "active flow {} selected addresses",
            flow.id
        );
    }

    // Whole-relation conservation, including identity/status/traffic tuples.
    assert_eq!(
        pre.lines(),
        post.lines(),
        "0017 changed the normalized flow relation"
    );
    Ok(())
}

fn text(row: &Value, key: &str) -> TestResult<String> {
    Ok(row
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing text field {key}"))?
        .to_string())
}

fn ports(row: &Value) -> TestResult<Option<Vec<i32>>> {
    match row.get("destination_ports") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => Ok(Some(serde_json::from_value(value.clone())?)),
    }
}

fn side_reference(row: &Value, side: &str, object_type: &str) -> TestResult<Option<Uuid>> {
    let Some(suffix) = reference_suffix(object_type) else {
        return Ok(None);
    };
    let key = format!("{side}_{suffix}");
    match row.get(&key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let raw = value
                .as_str()
                .ok_or_else(|| format!("{key} is not a uuid string"))?;
            Ok(Some(Uuid::parse_str(raw)?))
        }
    }
}

fn reference_suffix(object_type: &str) -> Option<&'static str> {
    match object_type {
        "dcim.device" => Some("device_id"),
        "virtualization.virtualmachine" => Some("vm_id"),
        "ipam.service" => Some("service_id"),
        "ipam.ipaddress" => Some("ipaddress_id"),
        "ipam.prefix" => Some("prefix_id"),
        "dcim.region" => Some("region_id"),
        "dcim.site" => Some("site_id"),
        "dcim.sitegroup" => Some("sitegroup_id"),
        "dcim.devicerole" => Some("devicerole_id"),
        "tenancy.tenant" => Some("tenant_id"),
        "tenancy.tenantgroup" => Some("tenantgroup_id"),
        "dcim.platform" => Some("platform_id"),
        "dcim.manufacturer" => Some("manufacturer_id"),
        "dcim.devicetype" => Some("devicetype_id"),
        "dcim.rack" => Some("rack_id"),
        "dcim.location" => Some("location_id"),
        "virtualization.cluster" => Some("cluster_id"),
        "virtualization.clustergroup" => Some("clustergroup_id"),
        "virtualization.clustertype" => Some("clustertype_id"),
        "all" | "internet" => None,
        _ => None,
    }
}

async fn column_exists(pool: &PgPool, table: &str, column: &str) -> TestResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = $1 AND column_name = $2)",
    )
    .bind(table)
    .bind(column)
    .fetch_one(pool)
    .await?)
}

async fn table_exists(pool: &PgPool, table: &str) -> TestResult<bool> {
    Ok(sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(format!("public.{table}"))
        .fetch_one(pool)
        .await?)
}
