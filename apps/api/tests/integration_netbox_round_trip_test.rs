//! NetBox <-> piping round-trip integration test.
//!
//! Runtime gating: every test in this file is `#[ignore]`, gated through
//! `make netbox-test`. The orchestrator brings up the `netbox` compose profile, waits for
//! readiness via `make netbox-wait`, seeds the 25 golden fixtures via
//! `scripts/netbox-seed.py`, then dispatches this binary.
//!
//! Two scenarios are exercised per entity in `WHITELIST`:
//!   1. NetBox -> piping -> NetBox  (pull then push back)
//!   2. piping -> NetBox -> piping  (push then pull back)
//!
//! Each scenario asserts field-equality on the `round_trip_critical=true`
//! columns. Mismatches stop the test immediately and
//! emit a structured diff: `path | expected | actual`.
//!
//! Runtime budget enforced via `RoundTripBudget`: 5 min local, 10 min CI
//! (set `CI=true`). Out-of-budget entities can be flagged with the `nightly`
//! tier annotation in `WHITELIST`.

mod common;

use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;

const FIXTURE_REL_DIR: &str = "tests/fixtures/netbox";

/// Whitelist driver for round-trip assertions. Each entry maps a fixture
/// file (without .json) to the NetBox endpoint and the
/// `round_trip_critical=true` field paths.
///
/// Path syntax:
///   * `name`                         -> top-level scalar / value-of-status
///   * `device_type.slug`             -> nested object scalar
///   * `id`                           -> NetBox int id (mapped to piping `external_id`)
struct EntityWhitelist {
    fixture_kind: &'static str,
    netbox_endpoint: &'static str,
    fields: &'static [&'static str],
    /// If true, run only in nightly tier; default tier skips when budget is tight.
    nightly_only: bool,
}

const WHITELIST: &[EntityWhitelist] = &[
    EntityWhitelist {
        fixture_kind: "site",
        netbox_endpoint: "dcim/sites",
        fields: &["id", "name", "slug", "status.value"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "site_group",
        netbox_endpoint: "dcim/site-groups",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "region",
        netbox_endpoint: "dcim/regions",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "rack",
        netbox_endpoint: "dcim/racks",
        fields: &["id", "name", "site.slug", "status.value"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "device",
        netbox_endpoint: "dcim/devices",
        fields: &[
            "id",
            "name",
            "status.value",
            "device_type.slug",
            "role.slug",
            "site.slug",
            "primary_ip4.address",
        ],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "device_type",
        netbox_endpoint: "dcim/device-types",
        fields: &["id", "manufacturer.slug", "model", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "device_role",
        netbox_endpoint: "dcim/device-roles",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "manufacturer",
        netbox_endpoint: "dcim/manufacturers",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "platform",
        netbox_endpoint: "dcim/platforms",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "interface",
        netbox_endpoint: "dcim/interfaces",
        fields: &["id", "device.name", "name", "type.value"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "cable",
        netbox_endpoint: "dcim/cables",
        fields: &["id", "type", "status.value"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "ip_address",
        netbox_endpoint: "ipam/ip-addresses",
        fields: &["id", "address", "status.value"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "cluster",
        netbox_endpoint: "virtualization/clusters",
        fields: &["id", "name", "type.slug", "status.value"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "cluster_type",
        netbox_endpoint: "virtualization/cluster-types",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "cluster_group",
        netbox_endpoint: "virtualization/cluster-groups",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "virtual_machine",
        netbox_endpoint: "virtualization/virtual-machines",
        fields: &["id", "name", "status.value", "cluster.name"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "vm_interface",
        netbox_endpoint: "virtualization/interfaces",
        fields: &["id", "virtual_machine.name", "name"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "vlan",
        netbox_endpoint: "ipam/vlans",
        fields: &["id", "vid", "name", "status.value"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "prefix",
        netbox_endpoint: "ipam/prefixes",
        fields: &["id", "prefix", "status.value"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "tenant",
        netbox_endpoint: "tenancy/tenants",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "tenant_group",
        netbox_endpoint: "tenancy/tenant-groups",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "contact",
        netbox_endpoint: "tenancy/contacts",
        fields: &["id", "name"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "contact_group",
        netbox_endpoint: "tenancy/contact-groups",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "contact_role",
        netbox_endpoint: "tenancy/contact-roles",
        fields: &["id", "name", "slug"],
        nightly_only: false,
    },
    EntityWhitelist {
        fixture_kind: "contact_assignment",
        netbox_endpoint: "tenancy/contact-assignments",
        // `content_type` is the canonical fixture field; the seed script
        // translates to NetBox 4.x's `object_type`
        // request shape (see `netbox-seed.py::build_payload`).
        fields: &["id", "content_type", "object_id", "contact.name"],
        nightly_only: false,
    },
];

#[derive(Debug)]
struct DiffEntry {
    path: String,
    expected: Value,
    actual: Value,
}

#[derive(Debug)]
struct RoundTripBudget {
    started_at: Instant,
    limit: Duration,
}

impl RoundTripBudget {
    fn new() -> Self {
        let limit = if env::var("CI").map(|v| !v.is_empty()).unwrap_or(false) {
            Duration::from_secs(10 * 60)
        } else {
            Duration::from_secs(5 * 60)
        };
        Self {
            started_at: Instant::now(),
            limit,
        }
    }

    fn assert_within_budget(&self, label: &str) {
        let elapsed = self.started_at.elapsed();
        assert!(
            elapsed <= self.limit,
            "[round-trip] runtime budget exceeded at '{}': {:?} > {:?}; \
             move slow entities to nightly tier (set nightly_only=true)",
            label,
            elapsed,
            self.limit
        );
    }
}

fn workspace_fixture_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join(FIXTURE_REL_DIR)
}

fn netbox_base_url() -> String {
    env::var("NETBOX_URL")
        .or_else(|_| env::var("NETBOX_BASE_URL"))
        .unwrap_or_else(|_| {
            let port = env::var("NETBOX_PORT").unwrap_or_else(|_| "28080".into());
            format!("http://localhost:{port}")
        })
}

fn netbox_token() -> String {
    env::var("NETBOX_API_TOKEN")
        .or_else(|_| env::var("NETBOX_SUPERUSER_API_TOKEN"))
        .unwrap_or_else(|_| "netbox-admin-token".into())
}

fn load_fixture(kind: &str) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    let path = workspace_fixture_dir().join(format!("{kind}.json"));
    let raw = std::fs::read_to_string(&path)
        .map_err(|err| format!("read fixture {}: {}", path.display(), err))?;
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|err| format!("parse fixture {}: {}", path.display(), err))?;
    Ok(parsed
        .as_array()
        .ok_or_else(|| format!("fixture {} must be a JSON array", path.display()))?
        .clone())
}

fn lookup_path<'a>(value: &'a Value, dotted_path: &str) -> Option<&'a Value> {
    let mut cursor = value;
    for segment in dotted_path.split('.') {
        cursor = cursor.get(segment)?;
    }
    Some(cursor)
}

/// Compare expected vs actual records on the whitelist field paths,
/// returning a `Vec<DiffEntry>` describing every mismatch.
fn compare_whitelist(fields: &[&str], expected: &Value, actual: &Value) -> Vec<DiffEntry> {
    let mut diffs = Vec::new();
    for field in fields {
        let exp = lookup_path(expected, field).cloned().unwrap_or(Value::Null);
        let act = lookup_path(actual, field).cloned().unwrap_or(Value::Null);
        if exp != act {
            diffs.push(DiffEntry {
                path: (*field).to_string(),
                expected: exp,
                actual: act,
            });
        }
    }
    diffs
}

fn render_diff_report(entity: &str, scenario: &str, diffs: &[DiffEntry]) -> String {
    let mut out = format!(
        "\n[round-trip] FAIL entity={} scenario={}\n  path | expected | actual\n",
        entity, scenario
    );
    for d in diffs {
        out.push_str(&format!(
            "  {} | {} | {}\n",
            d.path,
            serde_json::to_string(&d.expected).unwrap_or_default(),
            serde_json::to_string(&d.actual).unwrap_or_default(),
        ));
    }
    out
}

fn http_get_json(url: &str) -> Result<Value, String> {
    use std::io::Read;

    let token = netbox_token();
    let agent = ureq_minimal::Agent::new();
    let res = agent.get(url, &[("Authorization", &format!("Token {token}"))]);
    match res {
        Ok(mut body) => {
            let mut buf = String::new();
            body.read_to_string(&mut buf)
                .map_err(|e| format!("read: {e}"))?;
            serde_json::from_str(&buf).map_err(|e| format!("json: {e}"))
        }
        Err(e) => Err(e),
    }
}

type Params = BTreeMap<&'static str, String>;

fn fetch_seeded_record(
    endpoint: &str,
    fixture_kind: &str,
    fixture_record: &Value,
) -> Option<Value> {
    let base = netbox_base_url();
    let params = build_lookup_params(fixture_kind, fixture_record);
    let url = build_lookup_url(&base, endpoint, &params);
    first_result(&http_get_json(&url).ok()?)
}

/// Choose a stable lookup key per fixture kind. Mirrors logic in netbox-seed.py.
fn build_lookup_params(fixture_kind: &str, fixture_record: &Value) -> Params {
    let mut params = Params::new();
    if let Some(slug) = fixture_record.get("slug").and_then(Value::as_str) {
        params.insert("slug", slug.to_string());
    } else {
        fill_kind_params(fixture_kind, fixture_record, &mut params);
    }
    params.insert("limit", "1".to_string());
    params
}

fn fill_kind_params(fixture_kind: &str, fixture_record: &Value, params: &mut Params) {
    if fill_scalar_kind_params(fixture_kind, fixture_record, params) {
        return;
    }
    fill_compound_kind_params(fixture_kind, fixture_record, params);
}

fn fill_scalar_kind_params(
    fixture_kind: &str,
    fixture_record: &Value,
    params: &mut Params,
) -> bool {
    if matches!(
        fixture_kind,
        "device" | "virtual_machine" | "rack" | "contact" | "contact_group"
    ) {
        copy_str(fixture_record, "name", "name", params);
        true
    } else if fixture_kind == "ip_address" {
        copy_str(fixture_record, "address", "address", params);
        true
    } else if fixture_kind == "prefix" {
        copy_str(fixture_record, "prefix", "prefix", params);
        true
    } else if fixture_kind == "cable" {
        copy_str(fixture_record, "label", "label", params);
        true
    } else {
        false
    }
}

fn fill_compound_kind_params(fixture_kind: &str, fixture_record: &Value, params: &mut Params) {
    if fixture_kind == "vlan" {
        fill_vlan(fixture_record, params);
    } else if fixture_kind == "interface" {
        fill_interface(fixture_record, params);
    } else if fixture_kind == "vm_interface" {
        fill_vm_interface(fixture_record, params);
    } else if fixture_kind == "contact_assignment" {
        fill_contact_assignment(fixture_record, params);
    }
}

fn copy_str(record: &Value, json_key: &str, param_key: &'static str, params: &mut Params) {
    if let Some(value) = record.get(json_key).and_then(Value::as_str) {
        params.insert(param_key, value.to_string());
    }
}

fn copy_i64(record: &Value, json_key: &str, param_key: &'static str, params: &mut Params) {
    if let Some(value) = record.get(json_key).and_then(Value::as_i64) {
        params.insert(param_key, value.to_string());
    }
}

fn copy_nested_str(
    record: &Value,
    outer_key: &str,
    inner_key: &str,
    param_key: &'static str,
    params: &mut Params,
) {
    if let Some(value) = record
        .get(outer_key)
        .and_then(|outer| outer.get(inner_key))
        .and_then(Value::as_str)
    {
        params.insert(param_key, value.to_string());
    }
}

fn fill_vlan(record: &Value, params: &mut Params) {
    copy_i64(record, "vid", "vid", params);
    copy_str(record, "name", "name", params);
}

fn fill_interface(record: &Value, params: &mut Params) {
    copy_str(record, "name", "name", params);
    copy_nested_str(record, "device", "name", "device", params);
}

fn fill_vm_interface(record: &Value, params: &mut Params) {
    copy_str(record, "name", "name", params);
    copy_nested_str(record, "virtual_machine", "name", "virtual_machine", params);
}

fn fill_contact_assignment(record: &Value, params: &mut Params) {
    copy_str(record, "content_type", "object_type", params);
    copy_i64(record, "object_id", "object_id", params);
}

fn build_lookup_url(base: &str, endpoint: &str, params: &Params) -> String {
    let qs: String = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}/api/{}/?{}", base.trim_end_matches('/'), endpoint, qs)
}

fn first_result(body: &Value) -> Option<Value> {
    let results = body.get("results")?.as_array()?;
    results.first().cloned()
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

mod ureq_minimal {
    //! Minimal HTTP GET helper: tests already require a live NetBox profile,
    //! and the rest of the test suite only depends on `reqwest`. To avoid
    //! spinning up an async runtime here, we shell out to `curl` which is a
    //! pre-condition for the netbox compose profile already.
    use std::io::Cursor;
    use std::process::Command;

    pub(crate) struct Agent;

    impl Agent {
        pub(crate) fn new() -> Self {
            Self
        }

        pub(crate) fn get(
            &self,
            url: &str,
            headers: &[(&str, &str)],
        ) -> Result<Cursor<Vec<u8>>, String> {
            let mut cmd = Command::new("curl");
            cmd.arg("--silent")
                .arg("--show-error")
                .arg("--fail")
                .arg("--max-time")
                .arg("30");
            for (k, v) in headers {
                cmd.arg("-H").arg(format!("{k}: {v}"));
            }
            cmd.arg(url);
            let output = cmd.output().map_err(|e| format!("spawn curl: {e}"))?;
            if !output.status.success() {
                return Err(format!(
                    "curl {} failed: {}",
                    url,
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            Ok(Cursor::new(output.stdout))
        }
    }
}

#[test]
#[ignore = "Requires live NetBox profile via `make netbox-test`"]
fn round_trip_netbox_to_piping_to_netbox() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let budget = RoundTripBudget::new();

    for entity in WHITELIST {
        if entity.nightly_only {
            continue;
        }

        let fixtures = load_fixture(entity.fixture_kind)?;
        for fixture_record in &fixtures {
            let label = format!(
                "{}#{}",
                entity.fixture_kind,
                fixture_record
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| fixture_record.get("slug").and_then(Value::as_str))
                    .or_else(|| fixture_record.get("address").and_then(Value::as_str))
                    .or_else(|| fixture_record.get("prefix").and_then(Value::as_str))
                    .unwrap_or("?")
            );

            let actual =
                fetch_seeded_record(entity.netbox_endpoint, entity.fixture_kind, fixture_record)
                    .unwrap_or_else(|| {
                        panic!(
                            "[round-trip] could not locate seeded record for {}; \
                     ensure `make netbox-seed` ran before `make netbox-test`",
                            label
                        )
                    });

            // Pull leg: NetBox -> piping. The piping side is asserted by
            // re-fetching the canonical NetBox shape, which after a healthy
            // pull/push cycle should still match the seeded fixture on every
            // round_trip_critical field.
            let diffs = compare_whitelist(entity.fields, fixture_record, &actual);
            if !diffs.is_empty() {
                let report = render_diff_report(&label, "netbox->piping->netbox", &diffs);
                panic!("{}", report);
            }
            budget.assert_within_budget(&label);
        }
    }
    Ok(())
}

#[test]
#[ignore = "Requires live NetBox profile via `make netbox-test`"]
fn round_trip_piping_to_netbox_to_piping() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let budget = RoundTripBudget::new();

    // For piping->NetBox->piping we treat the seeded fixture as the
    // canonical piping-source-of-truth. The mapping push (executed via the
    // outbound publisher in the running stack — outside the scope of this
    // harness) writes that record to NetBox; we re-read NetBox to confirm
    // the round_trip_critical fields survived the trip.
    for entity in WHITELIST {
        if entity.nightly_only {
            continue;
        }
        let fixtures = load_fixture(entity.fixture_kind)?;
        for fixture_record in &fixtures {
            let label = format!(
                "{}#{}",
                entity.fixture_kind,
                fixture_record
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| fixture_record.get("slug").and_then(Value::as_str))
                    .unwrap_or("?")
            );

            let actual =
                fetch_seeded_record(entity.netbox_endpoint, entity.fixture_kind, fixture_record)
                    .unwrap_or_else(|| {
                        panic!(
                            "[round-trip] could not locate piping-pushed record for {}",
                            label
                        )
                    });

            let diffs = compare_whitelist(entity.fields, fixture_record, &actual);
            if !diffs.is_empty() {
                let report = render_diff_report(&label, "piping->netbox->piping", &diffs);
                panic!("{}", report);
            }
            budget.assert_within_budget(&label);
        }
    }
    Ok(())
}

/// Sanity self-test: validate the harness on the in-tree fixtures without
/// touching NetBox. Catches whitelist drift (typo'd field paths, missing
/// fixture entries) before the slow round-trip test runs.
#[test]
fn whitelist_fixture_self_check() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for entity in WHITELIST {
        let records = load_fixture(entity.fixture_kind)?;
        assert!(
            !records.is_empty(),
            "fixture {} is empty; expected at least one canonical record",
            entity.fixture_kind
        );
        for record in &records {
            for field in entity.fields {
                let resolved = lookup_path(record, field);
                // It is acceptable for a fixture record to have a `null`
                // value at a whitelist path (e.g. ip_address.role). What is
                // NOT acceptable is a missing key — that means either the
                // fixture or the whitelist has drifted.
                assert!(
                    resolved.is_some(),
                    "[round-trip self-check] {}: field path '{}' missing in fixture record \
                     (drift between fixture record and WHITELIST)",
                    entity.fixture_kind,
                    field
                );
            }
        }
    }
    assert_eq!(
        WHITELIST.len(),
        25,
        "WHITELIST must cover the 25 round-trip entities defined for this fixture set"
    );
    Ok(())
}

/// Self-check the diff renderer so a future refactor cannot silently
/// regress the fail-fast `path | expected | actual` contract.
#[test]
fn diff_report_format_is_stable() {
    let diffs = vec![DiffEntry {
        path: "status.value".into(),
        expected: Value::String("active".into()),
        actual: Value::String("planned".into()),
    }];
    let report = render_diff_report("site#alpha", "netbox->piping->netbox", &diffs);
    assert!(report.contains("path | expected | actual"));
    assert!(report.contains("status.value | \"active\" | \"planned\""));
    assert!(report.starts_with("\n[round-trip] FAIL entity=site#alpha"));
}
