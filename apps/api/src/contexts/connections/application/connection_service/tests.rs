use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};
use uuid::Uuid;

use super::entry_selector::{EntrySelectorKind, parse_connection_entry_selector};
use super::proxy_payload::{
    collect_secret_link_ids, parse_secret_link_id, payload_host_key_endpoints,
    replace_secret_link_values, resolve_and_inject_host_key_pins,
};
use super::routing::{
    is_unresolved_host_template, resolve_primary_endpoint, resolve_proxy_host,
    substitute_config_placeholders,
};
use super::sessions::persist_observed_host_keys;
use super::{
    ConnectionProxyRoute, UserCredentialSetMergePolicy, filter_client_credentials,
    merge_required_connconf_params,
};
use crate::contexts::connections::domain::{ConnectionLaunchParams, ConnectionProtocol};
use crate::contexts::connections::infrastructure::SqlxConnectionRepository;
use crate::contexts::shared::host_key_pins;
use crate::errors::AppError;
use crate::integrations::proxy::shared_secret_provider::ProxyCredential;
use crate::integrations::proxy::{
    ProxyClient, ProxyMetricsResponse, ProxyMetricsSample, ProxySession,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

#[test]
fn detects_unresolved_host_templates() {
    assert!(is_unresolved_host_template("{{primary_ip4}}"));
    assert!(is_unresolved_host_template(" {{ primary_ip6 }} "));
    assert!(!is_unresolved_host_template("10.10.1.5"));
}

#[test]
fn returns_validation_error_when_template_host_is_unresolved() {
    let error = resolve_proxy_host(Some("{{primary_ip4}}".to_string()))
        .expect_err("unresolved template host must fail");
    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("placeholder"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn keeps_explicit_host_value() {
    let host = resolve_proxy_host(Some("  10.10.10.10  ".to_string()))
        .expect("explicit host should be used");
    assert_eq!(host, "10.10.10.10");
}

#[test]
fn returns_validation_error_when_host_is_missing() {
    let error = resolve_proxy_host(None).expect_err("missing host must fail");
    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("required"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn substitutes_known_placeholders_from_context() {
    let context = json!({
        "primary_ip4": "10.10.1.230",
        "host": {
            "name": "operator-pc-1"
        }
    });
    let mut value = json!({
        "host": "{{primary_ip4}}",
        "label": "RDP {{host.name}}"
    });

    substitute_config_placeholders(&mut value, &context).expect("substitution succeeds");

    assert_eq!(
        value.get("host").and_then(|v| v.as_str()),
        Some("10.10.1.230")
    );
    assert_eq!(
        value.get("label").and_then(|v| v.as_str()),
        Some("RDP operator-pc-1")
    );
}

#[test]
fn overlays_endpoint_aliases_from_target_namespace() {
    let namespace = json!({
        "primary_ip4": "10.5.0.5",
        "primary_ip6": "2001:db8::5"
    });
    let mut context = json!({
        "primary_ip4": null,
        "primary_ip6": ""
    });

    let Value::Object(map) = &mut context else {
        panic!("test context must be an object");
    };
    super::overlay_endpoint_aliases_from_target_namespace(map, &namespace);

    assert_eq!(context["primary_ip4"], "10.5.0.5");
    assert_eq!(context["primary_ip6"], "2001:db8::5");
}

#[test]
fn keeps_unknown_placeholders_unresolved() {
    let context = json!({
        "primary_ip4": "10.10.1.230"
    });
    let mut value = json!({
        "host": "{{missing_field}}"
    });

    substitute_config_placeholders(&mut value, &context).expect("substitution succeeds");

    assert_eq!(
        value.get("host").and_then(|v| v.as_str()),
        Some("{{missing_field}}")
    );
}

#[test]
fn does_not_substitute_primary_ip_placeholder_from_host_name() {
    let context = json!({
        "host": {
            "name": "spb-hv1"
        }
    });
    let mut value = json!({
        "host": "{{primary_ip4}}"
    });

    substitute_config_placeholders(&mut value, &context).expect("substitution succeeds");

    assert_eq!(
        value.get("host").and_then(|v| v.as_str()),
        Some("{{primary_ip4}}")
    );
}

#[test]
fn does_not_substitute_top_level_placeholder_from_host_nested_field() {
    let context = json!({
        "host": {
            "primary_ip4": "10.10.1.230"
        }
    });
    let mut value = json!({
        "host": "{{primary_ip4}}"
    });

    substitute_config_placeholders(&mut value, &context).expect("substitution succeeds");

    assert_eq!(
        value.get("host").and_then(|v| v.as_str()),
        Some("{{primary_ip4}}")
    );
}

#[test]
fn rejects_unsupported_host_placeholder_key() {
    let error = resolve_proxy_host(Some("{{address}}".to_string()))
        .expect_err("unsupported endpoint placeholder must fail");
    match error {
        AppError::Validation(message) => {
            assert!(
                message.starts_with("endpoint_unsupported_placeholder:"),
                "unexpected message: {message}"
            );
            assert!(
                message.contains("primary_ip4"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn rejects_unknown_host_placeholder_key() {
    let error = resolve_proxy_host(Some("{{site.name}}".to_string()))
        .expect_err("unsupported placeholder must fail");
    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("not supported"),
                "unexpected message: {message}"
            );
            assert!(
                message.contains("primary_ip6"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn does_not_substitute_unsupported_endpoint_placeholder_from_context() {
    let context = json!({
        "address": "10.10.1.230"
    });
    let mut value = json!({
        "host": "{{address}}",
        "label": "Address {{address}}"
    });

    substitute_config_placeholders(&mut value, &context).expect("substitution succeeds");

    assert_eq!(
        value.get("host").and_then(|v| v.as_str()),
        Some("{{address}}")
    );
    assert_eq!(
        value.get("label").and_then(|v| v.as_str()),
        Some("Address 10.10.1.230")
    );
}

#[test]
fn unresolved_canonical_host_placeholder_reports_field_name() {
    let error = resolve_proxy_host(Some("{{primary_ip4}}".to_string()))
        .expect_err("unresolved canonical placeholder must fail");
    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("primary_ip4"),
                "unexpected message: {message}"
            );
            assert!(
                message.contains("could not be resolved"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn unresolved_canonical_host_placeholder_with_spaces_reports_field_name() {
    let error = resolve_proxy_host(Some("{{ primary_ip6 }}".to_string()))
        .expect_err("unresolved canonical placeholder must fail");
    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("primary_ip6"),
                "unexpected message: {message}"
            );
            assert!(
                message.contains("could not be resolved"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn resolves_primary_endpoint_prefers_ipv4() {
    let host_config = json!({
        "primary_ip4": "10.10.1.230",
        "primary_ip6": "2001:db8::1"
    });
    assert_eq!(
        resolve_primary_endpoint(&host_config),
        Some("10.10.1.230".to_string())
    );
}

#[test]
fn resolves_primary_endpoint_falls_back_to_ipv6() {
    let host_config = json!({
        "primary_ip4": "",
        "primary_ip6": "2001:db8::1"
    });
    assert_eq!(
        resolve_primary_endpoint(&host_config),
        Some("2001:db8::1".to_string())
    );
}

#[test]
fn resolves_primary_endpoint_from_target_namespace_when_aliases_missing() {
    let host_config = json!({
        "primary_ip4": null,
        "target": {
            "primary_ip4": "172.18.0.8",
            "primary_ip6": "2001:db8::8"
        }
    });
    assert_eq!(
        resolve_primary_endpoint(&host_config),
        Some("172.18.0.8".to_string())
    );
}

#[test]
fn resolves_primary_endpoint_returns_none_when_unavailable() {
    let host_config = json!({
        "name": "host-01"
    });
    assert_eq!(resolve_primary_endpoint(&host_config), None);
}

#[test]
fn resolves_explicit_host_when_placeholder_is_not_used() {
    let host = resolve_proxy_host(Some("10.10.1.230".to_string()))
        .expect("non-placeholder host should pass");
    assert_eq!(host, "10.10.1.230");
}

#[test]
fn keeps_nested_non_endpoint_placeholder_substitution_available() {
    let context = json!({
        "site": {
            "name": "spb"
        }
    });
    let mut value = json!({
        "label": "Site {{site.name}}"
    });

    substitute_config_placeholders(&mut value, &context).expect("substitution succeeds");

    assert_eq!(
        value.get("label").and_then(|v| v.as_str()),
        Some("Site spb")
    );
}

#[test]
fn parses_secret_link_id() {
    let secret_id =
        Uuid::parse_str("01234567-89ab-7def-8123-456789abcdef").expect("secret id parses");
    let raw_uuid = parse_secret_link_id("01234567-89ab-7def-8123-456789abcdef").expect("raw uuid");
    assert_eq!(raw_uuid, Some(secret_id));

    let non_secret = parse_secret_link_id("ssh-password").expect("non secret should pass");
    assert_eq!(non_secret, None);
}

#[test]
fn replaces_nested_secret_links_with_values() {
    let secret_id =
        Uuid::parse_str("01234567-89ab-7def-8123-456789abcdef").expect("secret id parses");
    let mut payload = json!({
        "credentials": {
            "password": secret_id.to_string(),
            "username": "root"
        },
        "connections": [
            {
                "private_key": secret_id.to_string()
            }
        ]
    });

    let mut ids = HashSet::new();
    let allowed = HashSet::from([secret_id]);
    collect_secret_link_ids(&payload, &allowed, &mut ids);
    assert_eq!(ids.len(), 1);
    assert!(ids.contains(&secret_id));

    let mut resolved = HashMap::new();
    resolved.insert(secret_id, "resolved-secret".to_string());
    replace_secret_link_values(&mut payload, &resolved);

    assert_eq!(
        payload
            .pointer("/credentials/password")
            .and_then(|value| value.as_str()),
        Some("resolved-secret")
    );
    assert_eq!(
        payload
            .pointer("/connections/0/private_key")
            .and_then(|value| value.as_str()),
        Some("resolved-secret")
    );
}

#[test]
fn rdp_client_credentials_preserve_domain_and_minimize_fields() {
    let credentials = json!({
        "username": "operator",
        "domain": "GRAD",
        "password": "secret",
        "private_key": "not-for-rdp",
        "ignored": true
    })
    .as_object()
    .cloned()
    .expect("credentials object");

    let filtered = filter_client_credentials(&ConnectionProtocol::Rdp, credentials);

    assert_eq!(
        filtered.get("username").and_then(|value| value.as_str()),
        Some("operator")
    );
    assert_eq!(
        filtered.get("domain").and_then(|value| value.as_str()),
        Some("GRAD")
    );
    assert_eq!(
        filtered.get("password").and_then(|value| value.as_str()),
        Some("secret")
    );
    assert!(!filtered.contains_key("private_key"));
    assert!(!filtered.contains_key("ignored"));
}

fn params(value: Value) -> ConnectionLaunchParams {
    value.as_object().cloned().expect("params object")
}

#[test]
fn personal_credentials_do_not_create_protocol_availability_without_connconf() {
    let user_params = params(json!({
        "credentials": {
            "username": "operator",
            "password": "secret"
        }
    }));

    let error = merge_required_connconf_params(
        &ConnectionProtocol::Rdp,
        None,
        Some(user_params),
        UserCredentialSetMergePolicy::FillMissing,
    )
    .expect_err("personal credentials alone must not launch a protocol");

    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("No default rdp connection config"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn personal_credentials_overlay_existing_connconf_defaults() {
    let host_defaults = params(json!({
        "host": "{{primary_ip4}}",
        "port": 3389,
        "credentials": {
            "username": "host-default"
        }
    }));
    let user_params = params(json!({
        "credentials": {
            "username": "operator",
            "password": "secret"
        }
    }));

    let merged = merge_required_connconf_params(
        &ConnectionProtocol::Rdp,
        Some(host_defaults),
        Some(user_params),
        UserCredentialSetMergePolicy::OverrideExisting,
    )
    .expect("connconf defaults allow personal overlay");

    assert_eq!(merged["host"], json!("{{primary_ip4}}"));
    assert_eq!(merged["port"], json!(3389));
    assert_eq!(merged["credentials"]["username"], json!("operator"));
    assert_eq!(merged["credentials"]["password"], json!("secret"));
}

#[test]
fn rejects_user_entry_selector_without_credential_set_id() {
    let error = parse_connection_entry_selector("user:config-context:ssh:1")
        .expect_err("selector without set id must fail");
    match error {
        AppError::Validation(message) => {
            assert!(
                message.contains("must include credential set id"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn parses_user_entry_selector_with_credential_set_id() {
    let set_id = Uuid::parse_str("01957fce-a4f7-7a80-9ac2-f6dc041ce772").expect("set id parses");
    let raw = format!("user:config-context:ssh:0:{set_id}");
    let selector = parse_connection_entry_selector(&raw)
        .expect("selector parse should succeed")
        .expect("selector should be present");

    assert_eq!(selector.kind, EntrySelectorKind::User);
    assert_eq!(selector.source_ref, "config-context");
    assert_eq!(selector.protocol, "ssh");
    assert_eq!(selector.index, 0);
    assert_eq!(selector.credential_set_id, Some(set_id));
}

mod launch_account_selection {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::super::connconf_resolve::map_effective_params;
    use super::super::select_launch_account;
    use crate::contexts::connection_config::domain::{
        ConnConfAssignment, ConnConfProtocol, ConnConfRecord, ConnConfScope, ConnConfSource,
    };
    use crate::contexts::connection_config::{
        LaunchAccount, ResolveInput, resolve_effective_config, resolve_launch_accounts,
    };
    use crate::errors::AppError;
    use chrono::Utc;

    const REGION: u128 = 100;
    const DEVICE: u128 = 200;

    fn account(record: u128, username: &str) -> LaunchAccount {
        LaunchAccount {
            record_id: Uuid::from_u128(record),
            record_name: format!("rec-{record}"),
            scope_type: Some(ConnConfScope::Device),
            scope_id: Some(Uuid::from_u128(DEVICE)),
            params: json!({ "host": "10.0.0.1", "credentials": { "username": username } }),
            attribution: BTreeMap::new(),
        }
    }

    // Selecting different indices from the ordered account list maps to different
    // launch credentials.username (the connect path honors the SELECTED account).
    #[test]
    fn selecting_index_zero_vs_one_yields_different_username() {
        let accounts = vec![account(2, "root"), account(3, "admin")];

        let selected_zero = select_launch_account(&accounts, Some(0))
            .expect("index 0 in range")
            .expect("account present");
        let mapped_zero = map_effective_params(&selected_zero.params, &selected_zero.attribution)
            .expect("mapped");
        assert_eq!(mapped_zero.fields["credentials"]["username"], json!("root"));

        let selected_one = select_launch_account(&accounts, Some(1))
            .expect("index 1 in range")
            .expect("account present");
        let mapped_one =
            map_effective_params(&selected_one.params, &selected_one.attribution).expect("mapped");
        assert_eq!(mapped_one.fields["credentials"]["username"], json!("admin"));
    }

    // Default launch (no selector) deterministically picks account index 0.
    #[test]
    fn default_selection_picks_index_zero() {
        let accounts = vec![account(2, "root"), account(3, "admin")];
        let selected = select_launch_account(&accounts, None)
            .expect("default selection ok")
            .expect("account present");
        assert_eq!(selected.record_id, Uuid::from_u128(2));
    }

    // An out-of-range selected index is a Validation error, never a panic.
    #[test]
    fn out_of_range_index_is_validation_error() {
        let accounts = vec![account(2, "root")];
        let error = select_launch_account(&accounts, Some(5)).expect_err("out of range must fail");
        match error {
            AppError::Validation(message) => {
                assert!(
                    message.contains("out of range"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    // Empty account list -> Ok(None) under default selection. Callers treat that
    // as "no connconf config applies"; personal credential sets must not create
    // protocol availability by themselves.
    #[test]
    fn empty_accounts_default_yields_none() {
        let accounts: Vec<LaunchAccount> = Vec::new();
        let selected = select_launch_account(&accounts, None).expect("ok");
        assert!(selected.is_none());
    }

    fn record(id: u128, name: &str, params: Value) -> ConnConfRecord {
        ConnConfRecord {
            id: Uuid::from_u128(id),
            name: name.to_string(),
            description: String::new(),
            protocol: ConnConfProtocol::Ssh,
            base_record_id: None,
            enabled: true,
            owner_user_id: None,
            params,
            custom_field_data: json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn assignment(
        record_id: u128,
        scope: ConnConfScope,
        scope_id: Option<u128>,
        weight: i32,
    ) -> ConnConfAssignment {
        ConnConfAssignment {
            id: Uuid::from_u128(record_id + 9_000_000),
            record_id: Uuid::from_u128(record_id),
            scope_type: scope,
            scope_id: scope_id.map(Uuid::from_u128),
            weight,
            enabled: true,
            created_at: Utc::now(),
        }
    }

    fn device_scopes() -> Vec<(ConnConfScope, Option<Uuid>)> {
        vec![
            (ConnConfScope::Global, None),
            (ConnConfScope::Region, Some(Uuid::from_u128(REGION))),
            (ConnConfScope::Device, Some(Uuid::from_u128(DEVICE))),
        ]
    }

    // NON-REGRESSION: when there are no identity accounts, the resolver returns
    // exactly ONE fallback account equal to today's merged config, so default
    // selection of index 0 reproduces the prior single-config connect behavior.
    #[test]
    fn zero_identity_fallback_default_equals_merged_config() {
        let region = record(1, "region", json!({ "host": "10.0.0.1", "port": 22 }));
        let device = record(2, "device", json!({ "port": 2222 }));
        let records = vec![region, device];
        let assignments = vec![
            assignment(1, ConnConfScope::Region, Some(REGION), 0),
            assignment(2, ConnConfScope::Device, Some(DEVICE), 0),
        ];
        let scopes = device_scopes();
        let input = ResolveInput {
            protocol: ConnConfProtocol::Ssh,
            device_scopes: &scopes,
            assignments: &assignments,
            records: &records,
        };

        // The account list collapses to a single fallback account...
        let accounts = resolve_launch_accounts(&input).expect("resolve accounts");
        assert_eq!(accounts.len(), 1, "single fallback account");

        // ...and default selection (index 0) of that account maps to the SAME
        // launch fields as today's merged-config path.
        let selected = select_launch_account(&accounts, None)
            .expect("default ok")
            .expect("account present");
        let from_account =
            map_effective_params(&selected.params, &selected.attribution).expect("mapped account");

        let merged = resolve_effective_config(&input).expect("merged");
        let from_merged =
            map_effective_params(&merged.params, &merged.attribution).expect("mapped merged");

        assert_eq!(
            from_account.fields, from_merged.fields,
            "default account fields must equal merged-config fields"
        );
        // Device port wins the merge in both paths.
        assert_eq!(from_account.fields["port"], json!(2222));
    }

    // SECRET FENCE: a secret UUID contributed only by account A's record must NOT
    // be resolvable when connecting as account B — B's attribution does not
    // include A's record, so the fence has no contributing record to bind it.
    #[test]
    fn secret_attributed_to_account_a_absent_when_connecting_as_b() {
        let secret_uuid = "44444444-4444-4444-4444-444444444444";
        // Two identity accounts, each carrying its OWN password secret leaf,
        // attributed to its OWN record.
        let mut attr_a: BTreeMap<String, ConnConfSource> = BTreeMap::new();
        attr_a.insert(
            "credentials.password".to_string(),
            ConnConfSource {
                record_id: Uuid::from_u128(2),
                record_name: "account-a".to_string(),
                scope_type: Some(ConnConfScope::Device),
                scope_id: Some(Uuid::from_u128(DEVICE)),
            },
        );
        let account_a = LaunchAccount {
            record_id: Uuid::from_u128(2),
            record_name: "account-a".to_string(),
            scope_type: Some(ConnConfScope::Device),
            scope_id: Some(Uuid::from_u128(DEVICE)),
            params: json!({
                "host": "10.0.0.1",
                "credentials": { "username": "a", "password": secret_uuid }
            }),
            attribution: attr_a,
        };
        let account_b = LaunchAccount {
            record_id: Uuid::from_u128(3),
            record_name: "account-b".to_string(),
            scope_type: Some(ConnConfScope::Device),
            scope_id: Some(Uuid::from_u128(DEVICE)),
            // B has no secret leaf and no attribution to A's record.
            params: json!({ "host": "10.0.0.1", "credentials": { "username": "b" } }),
            attribution: BTreeMap::new(),
        };
        let accounts = vec![account_a, account_b];

        // Connecting as account A (index 0): the secret UUID is attributed to A's
        // record (2), so the fence is offered that record as the binding authority.
        let selected_a = select_launch_account(&accounts, Some(0))
            .expect("index 0")
            .expect("account a");
        let mapped_a =
            map_effective_params(&selected_a.params, &selected_a.attribution).expect("mapped a");
        let parsed = Uuid::parse_str(secret_uuid).expect("uuid");
        let attrs_a = mapped_a
            .secret_attributions
            .get(&parsed)
            .expect("secret attributed when connecting as A");
        assert_eq!(attrs_a[0].record_id, Uuid::from_u128(2));

        // Connecting as account B (index 1): B contributes NO secret leaf and no
        // attribution, so the fence sees no contributing record for that UUID and
        // would reject it. The mapped attribution is empty.
        let selected_b = select_launch_account(&accounts, Some(1))
            .expect("index 1")
            .expect("account b");
        let mapped_b =
            map_effective_params(&selected_b.params, &selected_b.attribution).expect("mapped b");
        assert!(
            !mapped_b.secret_attributions.contains_key(&parsed),
            "account A's secret must not be attributed when connecting as B"
        );
    }
}

const HOST_KEY_TEST_BLOB: &str =
    "AAAAC3NzaC1lZDI1NTE5AAAAIKdwFDb7uXDNTayo3mt/sJyW1Hut13OuWSjKUWvi64be";

fn ssh_known_hosts_line(host: &str, port: u16, blob: &str) -> String {
    format!("[{host}]:{port} ssh-ed25519 {blob}")
}

/// Pool for the `#[ignore]`d tests below, which need the real
/// `ssh_host_key_pin` table. Absent configuration is a failure, not a skip:
/// silently returning nothing would report success while asserting nothing.
async fn ignored_test_pool() -> PgPool {
    let database_url = std::env::var("PIPING_DATABASE__URL").expect(
        "these tests are #[ignore]d because they need PostgreSQL; run them with \
         PIPING_DATABASE__URL set",
    );
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to test PostgreSQL");
    crate::storage::database::migrations::run_migrations(&pool)
        .await
        .expect("run migrations");
    pool
}

#[test]
fn payload_host_key_endpoints_covers_primary_and_jump_hops() {
    let payload = json!({
        "host": "10.0.0.5",
        "credentials": { "username": "operator" },
        "jump": [
            { "host": "bastion.example.org", "port": 2200 },
            { "host": "inner.example.org" },
            { "port": 2222 }
        ]
    });

    assert_eq!(
        payload_host_key_endpoints(&payload),
        vec![
            ("10.0.0.5".to_string(), 22),
            ("bastion.example.org".to_string(), 2200),
            ("inner.example.org".to_string(), 22),
        ],
        "the primary endpoint and every complete jump hop must be pinned; an incomplete hop is skipped"
    );
    assert!(
        payload_host_key_endpoints(&json!("not-an-object")).is_empty(),
        "a non-object payload has no endpoints"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL with PIPING_DATABASE__URL"]
async fn resolve_seed_lines_injected_into_proxy_payload() {
    let pool = ignored_test_pool().await;
    let repository = SqlxConnectionRepository::new(pool.clone());

    let host = "pwa14-payload.example.org";
    let jump_host = "pwa14-payload-jump.example.org";
    let port: u16 = 2222;
    let jump_port: u16 = 2200;
    // The test owns these endpoints; a prior run may have left pins behind.
    let _ = host_key_pins::delete_pins(&pool, host, port).await;
    let _ = host_key_pins::delete_pins(&pool, jump_host, jump_port).await;

    let payload = || {
        json!({
            "connection_type": "ssh",
            "host": host,
            "port": port,
            "credentials": { "username": "operator" },
            "jump": [ { "host": jump_host, "port": jump_port } ],
        })
    };

    // Absent pins: the key is present and empty, so downstream readers see a
    // stable shape rather than a missing field.
    let injected = resolve_and_inject_host_key_pins(payload(), &repository)
        .await
        .expect("inject without pins");
    assert_eq!(
        injected.get(host_key_pins::PROXY_PAYLOAD_KNOWN_HOSTS_LINES),
        Some(&json!([])),
        "an unpinned endpoint must still write an empty known_hosts_lines array"
    );

    // Pins for the primary endpoint AND the jump hop land verbatim.
    let primary_line = ssh_known_hosts_line(host, port, HOST_KEY_TEST_BLOB);
    let jump_line = ssh_known_hosts_line(jump_host, jump_port, HOST_KEY_TEST_BLOB);
    host_key_pins::upsert_lines(&pool, std::slice::from_ref(&primary_line))
        .await
        .expect("seed primary pin");
    host_key_pins::upsert_lines(&pool, std::slice::from_ref(&jump_line))
        .await
        .expect("seed jump pin");

    let injected = resolve_and_inject_host_key_pins(payload(), &repository)
        .await
        .expect("inject with pins");
    let lines = injected
        .get(host_key_pins::PROXY_PAYLOAD_KNOWN_HOSTS_LINES)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        lines,
        vec![Value::String(primary_line), Value::String(jump_line)],
        "the payload must carry exactly the pinned primary and jump lines"
    );
}

#[derive(Debug, Clone)]
struct ObservedKeysProxyClient {
    observed_host_keys: Vec<String>,
}

impl ProxyClient for ObservedKeysProxyClient {
    async fn create_connection(
        &self,
        _payload: Value,
        _proxy_base_url: Option<&str>,
        _credential: Option<&ProxyCredential>,
    ) -> Result<ProxySession, AppError> {
        Err(AppError::Internal(
            "create_connection is not used by this test".to_string(),
        ))
    }

    async fn terminate_connection(
        &self,
        _proxy_session_id: Uuid,
        _proxy_base_url: Option<&str>,
        _credential: Option<&ProxyCredential>,
    ) -> Result<(), AppError> {
        Ok(())
    }

    async fn get_metrics(
        &self,
        _proxy_session_id: Uuid,
        _proxy_base_url: Option<&str>,
        _credential: Option<&ProxyCredential>,
    ) -> Result<ProxyMetricsSample, AppError> {
        Ok(ProxyMetricsSample {
            metrics: ProxyMetricsResponse {
                connection_id: "pwa14-test".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
                overall_status: "up".to_string(),
                links: Vec::new(),
                observed_host_keys: self.observed_host_keys.clone(),
            },
            request_latency_ms: None,
        })
    }
}

/// Exercises the persist helper `disconnect_session` calls. It does NOT cover
/// the disconnect orchestration itself (that the method reads metrics before
/// `terminate_connection`): building a full `ConnectionService` needs every
/// collaborator stubbed, so the free function is tested directly.
#[tokio::test]
#[ignore = "requires PostgreSQL with PIPING_DATABASE__URL"]
async fn disconnect_session_persists_observed_host_keys() {
    let pool = ignored_test_pool().await;
    let repository = SqlxConnectionRepository::new(pool.clone());

    let host = "pwa14-disconnect.example.org";
    let port: u16 = 2222;
    let _ = host_key_pins::delete_pins(&pool, host, port).await;

    let line = ssh_known_hosts_line(host, port, HOST_KEY_TEST_BLOB);
    let client = ObservedKeysProxyClient {
        observed_host_keys: vec![line.clone()],
    };
    let route = ConnectionProxyRoute {
        proxy_base_url: None,
        credential: None,
    };

    persist_observed_host_keys(&repository, &client, Uuid::now_v7(), &route).await;

    let pins = host_key_pins::fetch_pins(&pool, host, port)
        .await
        .expect("fetch pins");
    assert_eq!(
        pins.len(),
        1,
        "the proxy-observed key must be persisted as exactly one pin"
    );
    assert_eq!(pins[0].known_hosts_line, line);
}
