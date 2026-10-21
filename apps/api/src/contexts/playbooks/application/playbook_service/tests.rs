use std::collections::HashSet;
use std::sync::{Arc, Barrier};
use std::thread;

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use uuid::Uuid;

use super::{
    BuiltGroupCallbackTokenRecord, CreateArtifactInput, CreateRunEventInput,
    CreateRunGroupEventInput, PLAYBOOK_CONTENT_MAX_BYTES, PLAYBOOK_JOB_TOKEN_MAX_TTL_SECONDS,
    PLAYBOOK_JOB_TOKEN_PREFIX, PLAYBOOK_NAME_MAX_CHARS, PLAYBOOK_RUN_EVENT_MESSAGE_MAX_CHARS,
    PLAYBOOK_RUN_EVENT_RAW_MESSAGE_MAX_CHARS, PLAYBOOK_RUN_EVENT_TRUNCATION_MARKER,
    StaticTargetMemberInput, build_group_callback_token_record, callback_event_size_bytes,
    callback_token_auth, dedupe_group_hostless_events, hex_sha256, is_url_safe_job_token_secret,
    job_template_launch_metadata, job_token_has_scope, join_project_playbook_path,
    normalize_artifact_input, normalize_assignment_scope, normalize_category, normalize_choice,
    normalize_content, normalize_cron_expression, normalize_job_template_credential_refs,
    normalize_job_token_scopes, normalize_json_object, normalize_lease_owner, normalize_name,
    normalize_optional_range, normalize_optional_vars_schema, normalize_playbook_slug,
    normalize_project_path, normalize_project_scm_url, normalize_run_event_message,
    normalize_run_event_payload, normalize_run_event_record, normalize_run_event_type,
    normalize_run_group_event_record, normalize_selector, normalize_service_principal_slug,
    normalize_static_member, normalize_terminal_run_status, normalize_timezone,
    normalize_token_policy, normalize_version_timeout, parse_job_token,
    runtime_policy_from_token_policy, strip_system_metadata, validate_launch_extravars,
    validate_metadata, validate_run_launch_metadata,
};
use crate::contexts::playbooks::domain::{
    PlaybookAssignmentScopeKind, PlaybookJobTemplate, PlaybookRunEvent, PlaybookRunEventKind,
    PlaybookRunStatus, PlaybookRunTriggerKind,
};
use crate::errors::AppError;

#[test]
fn run_launch_metadata_merges_defaults_without_overriding_explicit_values() {
    let schema = json!({
        "type": "object",
        "properties": {
            "region": {"type": "string", "default": "dc-1"},
            "retries": {"type": "integer", "default": 1}
        },
        "required": ["region", "retries"],
        "additionalProperties": false
    });
    let metadata = json!({
        "launch_options": {"extravars": {"retries": 3}},
        "request_id": "req-1"
    });

    let normalized = validate_run_launch_metadata(Some(&schema), metadata).unwrap();

    assert_eq!(
        normalized,
        json!({
            "launch_options": {
                "extravars": {"region": "dc-1", "retries": 3}
            },
            "request_id": "req-1"
        })
    );
    assert_eq!(
        validate_run_launch_metadata(None, json!("opaque")).unwrap(),
        json!("opaque")
    );
    assert!(
        validate_run_launch_metadata(Some(&schema), json!({"launch_options": {"extravars": []}}),)
            .is_err()
    );
    assert!(validate_run_launch_metadata(Some(&schema), json!([])).is_err());
    assert!(
        validate_run_launch_metadata(Some(&schema), json!({"launch_options": "not-an-object"}),)
            .is_err()
    );
}

#[test]
fn event_records_derive_hosts_mask_secrets_and_measure_wire_size() {
    let run_id = Uuid::from_u128(101);
    let record = normalize_run_event_record(
        run_id,
        CreateRunEventInput {
            event_kind: PlaybookRunEventKind::Result,
            event_type: " Runner_On_Ok ".to_string(),
            message: Some("  completed  ".to_string()),
            payload: Some(json!({"host": " node-1 ", "token": "secret", "changed": true})),
        },
    )
    .unwrap();

    assert_eq!(record.run_id, run_id);
    assert_eq!(record.host.as_deref(), Some("node-1"));
    assert_eq!(record.event_kind, PlaybookRunEventKind::Result);
    assert_eq!(record.event_type, "runner_on_ok");
    assert_eq!(record.message.as_deref(), Some("completed"));
    assert_eq!(
        record.payload,
        json!({"host": " node-1 ", "token": "***", "changed": true})
    );

    let run_group_id = Uuid::from_u128(102);
    let group_record = normalize_run_group_event_record(
        run_group_id,
        CreateRunGroupEventInput {
            run_id: Some(run_id),
            host: Some(" explicit-node ".to_string()),
            event_kind: PlaybookRunEventKind::Stdout,
            event_type: "console_line".to_string(),
            message: Some("line".to_string()),
            payload: Some(json!({"host": "payload-node"})),
        },
    )
    .unwrap();
    assert_eq!(group_record.run_group_id, run_group_id);
    assert_eq!(group_record.run_id, Some(run_id));
    assert_eq!(group_record.host.as_deref(), Some("explicit-node"));
    assert_eq!(group_record.event_kind, PlaybookRunEventKind::Stdout);

    let fallback_host = normalize_run_group_event_record(
        run_group_id,
        CreateRunGroupEventInput {
            run_id: None,
            host: Some("   ".to_string()),
            event_kind: PlaybookRunEventKind::System,
            event_type: "status".to_string(),
            message: None,
            payload: Some(json!({"host": " payload-node "})),
        },
    )
    .unwrap();
    assert_eq!(fallback_host.host.as_deref(), Some("payload-node"));

    let small = callback_event_size_bytes(None, "status", None, &json!({})).unwrap();
    let large = callback_event_size_bytes(
        Some("node-1"),
        "status",
        Some("x"),
        &json!({"output": "y".repeat(64)}),
    )
    .unwrap();
    assert!(large > small + 64);
    assert!(
        normalize_run_event_record(
            run_id,
            CreateRunEventInput {
                event_kind: PlaybookRunEventKind::Result,
                event_type: "result".to_string(),
                message: None,
                payload: Some(json!([])),
            },
        )
        .is_err()
    );
}

fn assert_identifier_normalizers_reject_invalid_input() {
    let max_name = "é".repeat(PLAYBOOK_NAME_MAX_CHARS);
    assert_eq!(normalize_name(format!(" {max_name} ")).unwrap(), max_name);
    assert!(normalize_name(" ".to_string()).is_err());
    assert!(normalize_name("é".repeat(PLAYBOOK_NAME_MAX_CHARS + 1)).is_err());

    assert_eq!(
        normalize_service_principal_slug(" _Ops Team_ ").unwrap(),
        "ops-team"
    );
    assert!(normalize_service_principal_slug("ab").is_err());
    assert!(normalize_service_principal_slug(&"a".repeat(81)).is_err());
    assert_eq!(
        normalize_choice(" ALWAYS ", &["always", "never"], "pull_policy").unwrap(),
        "always"
    );
    assert!(normalize_choice("sometimes", &["always", "never"], "pull_policy").is_err());
}

fn assert_project_path_normalizers_reject_traversal() {
    assert_eq!(
        normalize_project_path(Some(" roles/web/main.yml ".to_string()), "path").unwrap(),
        Some("roles/web/main.yml".to_string())
    );
    assert_eq!(normalize_project_path(None, "path").unwrap(), None);
    assert_eq!(
        join_project_playbook_path(Some(" roles/web "), "tasks/main.yml").unwrap(),
        "roles/web/tasks/main.yml"
    );
    assert_eq!(
        join_project_playbook_path(None, "site.yml").unwrap(),
        "site.yml"
    );
    for invalid in [
        "/etc/passwd",
        "roles\\main.yml",
        "roles//main.yml",
        "roles/./main.yml",
        "roles/../main.yml",
    ] {
        assert!(
            normalize_project_path(Some(invalid.to_string()), "path").is_err(),
            "{invalid} must be rejected"
        );
    }
    assert!(join_project_playbook_path(Some("roles"), "../site.yml").is_err());
    assert!(join_project_playbook_path(None, " ").is_err());
}

fn assert_metadata_normalizers_reject_invalid_input() {
    assert_eq!(
        strip_system_metadata(&json!({
            "system": true,
            "factory": {"revision": 7},
            "labels": ["safe"]
        })),
        json!({"labels": ["safe"]})
    );
    assert_eq!(strip_system_metadata(&json!([])), json!({}));
    assert_eq!(normalize_optional_vars_schema(None).unwrap(), None);
    assert_eq!(
        normalize_optional_vars_schema(Some(json!(null))).unwrap(),
        None
    );
    assert_eq!(
        normalize_optional_vars_schema(Some(json!({"type": "object"}))).unwrap(),
        Some(json!({"type": "object"}))
    );
    assert!(normalize_optional_vars_schema(Some(json!([]))).is_err());
    assert!(normalize_optional_vars_schema(Some(json!({"api_token": "inline"}))).is_err());
}

fn assert_selector_normalizers_reject_invalid_input() {
    assert_eq!(normalize_selector(None).unwrap(), json!({}));
    assert_eq!(
        normalize_selector(Some(json!({"source_kind": ["device"]}))).unwrap(),
        json!({"source_kind": ["device"]})
    );
    assert!(normalize_selector(Some(json!([]))).is_err());
    assert!(normalize_selector(Some(json!({"password": "inline"}))).is_err());
}

fn assert_content_normalizer_enforces_byte_bounds() {
    assert_eq!(normalize_content("  ---\n  ".to_string()).unwrap(), "  ---");
    assert!(normalize_content(" \n\t ".to_string()).is_err());
    assert!(normalize_content("x".repeat(PLAYBOOK_CONTENT_MAX_BYTES)).is_ok());
    assert!(normalize_content("x".repeat(PLAYBOOK_CONTENT_MAX_BYTES + 1)).is_err());
}

#[test]
fn boundary_normalizers_reject_empty_traversing_and_oversized_inputs() {
    assert_identifier_normalizers_reject_invalid_input();
    assert_project_path_normalizers_reject_traversal();
    assert_metadata_normalizers_reject_invalid_input();
    assert_selector_normalizers_reject_invalid_input();
    assert_content_normalizer_enforces_byte_bounds();
}

fn assert_callback_token_matches_record(
    token: &BuiltGroupCallbackTokenRecord,
    now: DateTime<Utc>,
) -> (Uuid, String) {
    let (token_id, secret) = parse_job_token(&token.raw_token).expect("valid callback token");
    assert_eq!(token_id, token.record.id);
    assert_eq!(secret.len(), 43);
    assert_eq!(
        token.record.token_hash,
        hex_sha256(token.raw_token.as_bytes())
    );
    assert_eq!(
        token.record.token_prefix,
        format!("{PLAYBOOK_JOB_TOKEN_PREFIX}_{}", &secret[..8])
    );
    assert_eq!(token.record.expires_at, now + Duration::seconds(1));
    assert_eq!(token.record.metadata, json!({}));

    let auth = callback_token_auth(&token.raw_token, now).unwrap();
    assert_eq!(auth.token_id, token.record.id);
    assert_eq!(auth.token_hash, token.record.token_hash);
    assert_eq!(auth.now, now);
    (token_id, secret.to_string())
}

fn assert_capped_callback_token_ttl_and_invalid_token(now: DateTime<Utc>) {
    let capped = build_group_callback_token_record(Some(i64::MAX), now).unwrap();
    assert_eq!(
        capped.record.expires_at,
        now + Duration::seconds(PLAYBOOK_JOB_TOKEN_MAX_TTL_SECONDS)
    );
    assert!(build_group_callback_token_record(None, DateTime::<Utc>::MAX_UTC).is_err());
    assert!(matches!(
        callback_token_auth("not-a-token", now),
        Err(AppError::Auth(message)) if message == "TOKEN_INVALID"
    ));
}

#[test]
fn callback_tokens_are_unique_and_self_consistent_under_concurrency() {
    let now = Utc::now();
    let barrier = Arc::new(Barrier::new(16));
    let handles = (0..16)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                build_group_callback_token_record(Some(0), now)
                    .expect("concurrent token generation must succeed")
            })
        })
        .collect::<Vec<_>>();
    let built = handles
        .into_iter()
        .map(|handle| handle.join().expect("token thread must not panic"))
        .collect::<Vec<_>>();

    let mut ids = HashSet::new();
    let mut secrets = HashSet::new();
    for token in &built {
        let (token_id, secret) = assert_callback_token_matches_record(token, now);
        ids.insert(token_id);
        secrets.insert(secret);
    }
    assert_eq!(ids.len(), built.len());
    assert_eq!(secrets.len(), built.len());

    assert_capped_callback_token_ttl_and_invalid_token(now);
}

#[test]
fn normalize_playbook_slug_is_agent_friendly() {
    assert_eq!(
        normalize_playbook_slug(" Linux Inventory Collect ").unwrap(),
        "linux-inventory-collect"
    );
    assert_eq!(
        normalize_playbook_slug("inventory_collect").unwrap(),
        "inventory_collect"
    );
    assert!(normalize_playbook_slug("!!").is_err());
}

#[test]
fn normalize_category_is_url_safe_and_defaults_to_general() {
    assert_eq!(normalize_category(None).unwrap(), "general");
    assert_eq!(
        normalize_category(Some(" Linux Inventory ".to_string())).unwrap(),
        "linux-inventory"
    );
    assert!(normalize_category(Some("!!".to_string())).is_err());
}

#[test]
fn validate_launch_extravars_rejects_invalid_merged_values() {
    let schema = json!({
        "type": "object",
        "required": ["retries", "config"],
        "additionalProperties": false,
        "properties": {
            "retries": { "type": "integer", "minimum": 1, "maximum": 3 },
            "mode": { "type": "string", "enum": ["safe", "fast"], "minLength": 4, "maxLength": 4 },
            "config": {
                "type": "object",
                "required": ["enabled"],
                "additionalProperties": false,
                "properties": { "enabled": { "type": "boolean" } }
            },
            "targets": {
                "type": "array",
                "items": { "type": "string", "minLength": 1, "maxLength": 4 }
            }
        }
    });

    assert!(
        validate_launch_extravars(
            Some(&schema),
            &json!({
                "retries": 2,
                "mode": "safe",
                "config": { "enabled": true },
                "targets": ["node"]
            })
        )
        .is_ok()
    );

    for invalid in [
        json!({ "retries": 2, "mode": "safe", "config": { "enabled": true }, "extra": true }),
        json!({ "retries": 4, "mode": "safe", "config": { "enabled": true } }),
        json!({ "retries": 2.5, "mode": "safe", "config": { "enabled": true } }),
        json!({ "retries": 2, "mode": "slow", "config": { "enabled": true } }),
        json!({ "retries": 2, "mode": "safe", "config": { "enabled": true, "extra": true } }),
        json!({ "retries": 2, "mode": "safe", "config": {}, "targets": ["oversized"] }),
        json!({ "retries": 2, "mode": "safe" }),
    ] {
        assert!(
            validate_launch_extravars(Some(&schema), &invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn validate_launch_extravars_rejects_non_object_values_and_invalid_schema_keywords() {
    assert!(validate_launch_extravars(Some(&json!({ "type": "object" })), &json!([])).is_err());
    assert!(validate_launch_extravars(Some(&json!({ "type": "unknown" })), &json!({})).is_err());
    assert!(
        validate_launch_extravars(Some(&json!({ "properties": "invalid" })), &json!({})).is_err()
    );
    assert!(
        validate_launch_extravars(
            Some(&json!({ "type": "string", "pattern": "^safe$" })),
            &json!("safe")
        )
        .is_err()
    );
}

#[test]
fn job_template_launch_metadata_preserves_template_identity_and_merges_launch_inputs() {
    let now = Utc::now();
    let template = PlaybookJobTemplate {
        id: Uuid::from_u128(0x10),
        name: "Inventory template".to_string(),
        description: None,
        playbook_id: Some(Uuid::from_u128(0x20)),
        project_id: None,
        playbook_path: None,
        inventory_target_set_id: Some(Uuid::from_u128(0x30)),
        execution_environment_id: Some(Uuid::from_u128(0x40)),
        credential_refs: json!([]),
        extra_vars: json!({
            "template_extra": "kept",
            "override_me": "from-template-extra"
        }),
        launch_options: json!({
            "limit": "template-limit",
            "verbosity": 1,
            "extravars": {
                "launch_extra": "kept",
                "override_me": "from-template-launch"
            }
        }),
        ask_variables_on_launch: false,
        ask_limit_on_launch: false,
        ask_tags_on_launch: false,
        ask_skip_tags_on_launch: false,
        ask_credential_on_launch: false,
        ask_execution_environment_on_launch: false,
        enabled: true,
        metadata: json!({}),
        created_by_user_id: None,
        updated_by_user_id: None,
        created_at: now,
        updated_at: now,
    };

    let metadata = job_template_launch_metadata(
        &template,
        Some(json!({
            "source": "client-value",
            "note": "preserved",
            "launch_options": {
                "limit": "client-limit",
                "extravars": {
                    "client_extra": "kept",
                    "override_me": "from-client"
                }
            }
        })),
    )
    .expect("metadata");

    assert_eq!(metadata["source"], json!("playbooks.job_template"));
    assert_eq!(metadata["note"], json!("preserved"));
    assert_eq!(metadata["job_template_id"], json!(template.id.to_string()));
    assert_eq!(metadata["job_template_name"], json!("Inventory template"));
    assert_eq!(metadata["job_template"]["id"], json!(template.id));
    assert_eq!(
        metadata["job_template"]["playbook_id"],
        json!(template.playbook_id)
    );
    assert_eq!(
        metadata["job_template"]["execution_environment_id"],
        json!(template.execution_environment_id)
    );
    assert_eq!(metadata["job_template"]["source_kind"], json!("playbook"));
    assert_eq!(metadata["launch_options"]["limit"], json!("client-limit"));
    assert_eq!(metadata["launch_options"]["verbosity"], json!(1));
    assert_eq!(
        metadata["launch_options"]["extravars"],
        json!({
            "launch_extra": "kept",
            "template_extra": "kept",
            "client_extra": "kept",
            "override_me": "from-client"
        })
    );
}

#[test]
fn normalize_cron_expression_accepts_only_saffron_parseable_5_field_crons() {
    assert_eq!(
        normalize_cron_expression("  */5   * * * *  ".to_string()).unwrap(),
        "*/5 * * * *"
    );
    assert_eq!(
        normalize_cron_expression("0 0 1 JAN MON".to_string()).unwrap(),
        "0 0 1 JAN MON"
    );
    // Quartz `?` is normalized to `*` (semantically equivalent in dom/dow)
    // so it both validates and executes instead of fire-once-then-disable.
    assert_eq!(
        normalize_cron_expression("0 0 * * ?".to_string()).unwrap(),
        "0 0 * * *"
    );
    assert_eq!(
        normalize_cron_expression("0 0 ? * MON".to_string()).unwrap(),
        "0 0 * * MON"
    );
    assert!(normalize_cron_expression("* * * *".to_string()).is_err());
    // 6- and 7-field (seconds / seconds+year) crons are NOT executable by
    // saffron, so they must be rejected at create time, not accepted here.
    assert!(normalize_cron_expression("0 * * * * *".to_string()).is_err());
    assert!(normalize_cron_expression("0 * * * * * 2026".to_string()).is_err());
    assert!(normalize_cron_expression("* * * * * * * *".to_string()).is_err());
    assert!(normalize_cron_expression("* * * * *; rm -rf /".to_string()).is_err());
    assert!(normalize_cron_expression("nonsense here please ok".to_string()).is_err());
    assert!(normalize_cron_expression("x".repeat(201)).is_err());
}

#[test]
fn normalize_timezone_accepts_only_real_iana_timezones() {
    assert_eq!(normalize_timezone(None).unwrap(), "UTC");
    assert_eq!(
        normalize_timezone(Some(" Europe/Moscow ".to_string())).unwrap(),
        "Europe/Moscow"
    );
    assert_eq!(
        normalize_timezone(Some("Etc/GMT+3".to_string())).unwrap(),
        "Etc/GMT+3"
    );
    assert!(normalize_timezone(Some("".to_string())).is_err());
    assert!(normalize_timezone(Some("/UTC".to_string())).is_err());
    assert!(normalize_timezone(Some("Europe//Moscow".to_string())).is_err());
    assert!(normalize_timezone(Some("Europe/Moscow;echo".to_string())).is_err());
    // Charset-valid but not a real IANA zone: rejected via chrono_tz so the
    // scheduler can never fail to resolve it at runtime.
    assert!(normalize_timezone(Some("Mars/Phobos".to_string())).is_err());
    assert!(normalize_timezone(Some("Not-A-Zone".to_string())).is_err());
}

#[test]
fn normalize_assignment_scope_and_default_ranges_are_explicit() {
    assert_eq!(
        normalize_assignment_scope(PlaybookAssignmentScopeKind::Global, None).unwrap(),
        (PlaybookAssignmentScopeKind::Global, None)
    );
    assert!(
        normalize_assignment_scope(PlaybookAssignmentScopeKind::Global, Some(Uuid::now_v7()))
            .is_err()
    );
    let scope_id = Uuid::now_v7();
    assert_eq!(
        normalize_assignment_scope(PlaybookAssignmentScopeKind::Site, Some(scope_id)).unwrap(),
        (PlaybookAssignmentScopeKind::Site, Some(scope_id))
    );
    assert!(normalize_assignment_scope(PlaybookAssignmentScopeKind::Role, None).is_err());
    assert_eq!(
        normalize_optional_range(Some(50_000), "default_target_limit", 1, 50_000).unwrap(),
        Some(50_000)
    );
    assert!(normalize_optional_range(Some(50_001), "default_target_limit", 1, 50_000).is_err());
}

#[test]
fn parse_job_token_requires_pjt_prefix_uuid_and_secret() {
    let id = Uuid::from_u128(7);
    let raw = format!("{PLAYBOOK_JOB_TOKEN_PREFIX}_{id}_AbCdEf123_-");
    assert_eq!(parse_job_token(&raw).unwrap(), (id, "AbCdEf123_-"));
    assert!(parse_job_token("pat_00000000-0000-0000-0000-000000000007_secret").is_none());
    assert!(parse_job_token(&format!("pjt_{id}_")).is_none());
    assert!(parse_job_token(&format!("pjt_{id}_has.dot")).is_none());
    assert!(parse_job_token(&format!(" pjt_{id}_secret")).is_none());
}

#[test]
fn job_token_secret_candidate_is_urlsafe_base64url_shape() {
    assert!(is_url_safe_job_token_secret("AbCdEf123_-"));
    assert!(!is_url_safe_job_token_secret(""));
    assert!(!is_url_safe_job_token_secret("contains/slash"));
    assert!(!is_url_safe_job_token_secret("contains.dot"));
    assert!(!is_url_safe_job_token_secret("contains space"));
}

#[test]
fn job_token_scope_match_is_case_insensitive() {
    assert!(job_token_has_scope(
        &json!(["Artifact:Create"]),
        "artifact:create"
    ));
    assert!(!job_token_has_scope(
        &json!(["run:update"]),
        "artifact:create"
    ));
}

#[test]
fn normalize_job_token_scopes_allows_lifecycle_and_inventory_rest_scopes_only() {
    assert_eq!(
        normalize_job_token_scopes(None).unwrap(),
        json!(["artifact:create"])
    );
    assert_eq!(
        normalize_job_token_scopes(Some(vec![
            " Artifact:Create ".to_string(),
            "event:append".to_string(),
            "artifact:create".to_string(),
        ]))
        .unwrap(),
        json!(["artifact:create", "event:append"])
    );
    assert_eq!(
        normalize_job_token_scopes(Some(vec![
            "inventory:read".to_string(),
            "inventory:create".to_string(),
            "inventory:update".to_string(),
        ]))
        .unwrap(),
        json!(["inventory:read", "inventory:create", "inventory:update"])
    );
    assert!(normalize_job_token_scopes(Some(vec!["".to_string()])).is_err());
    assert!(normalize_job_token_scopes(Some(vec!["run:delete".to_string()])).is_err());
    assert!(normalize_job_token_scopes(Some(vec!["artifact:create".to_string(); 17])).is_err());
}

#[test]
fn normalize_token_policy_defaults_and_validates_shape() {
    assert_eq!(
        normalize_token_policy(None).unwrap(),
        json!({"ttl_seconds": 3600, "scopes": ["artifact:create"]})
    );
    assert!(normalize_token_policy(Some(json!([]))).is_err());
    assert!(normalize_token_policy(Some(json!({"ttl_seconds": 0}))).is_err());
    assert!(normalize_token_policy(Some(json!({"scopes": [""]}))).is_err());
    assert!(normalize_token_policy(Some(json!({"scopes": ["artifact:create"]}))).is_ok());
    assert!(normalize_token_policy(Some(json!({"raw_token": "pjt_secret"}))).is_err());
}

#[test]
fn token_policy_runtime_maps_inventory_scopes_and_secret_env() {
    let policy = normalize_token_policy(Some(json!({
        "ttl_seconds": 3600,
        "scopes": ["artifact:create"],
        "runtime": {
            "piping_api": { "enabled": true, "inventory": ["read", "create", "update", "read"] },
            "secrets": { "env": { "EXTERNAL_API_TOKEN": "external_api_token" } }
        }
    })))
    .unwrap();
    let runtime = runtime_policy_from_token_policy(&policy).unwrap();

    assert!(runtime.expose_piping_api);
    assert_eq!(
        runtime.inventory_scopes,
        vec!["inventory:read", "inventory:create", "inventory:update"]
    );
    assert_eq!(
        runtime.secret_env.get("EXTERNAL_API_TOKEN"),
        Some(&"external_api_token".to_string())
    );
}

#[test]
fn token_policy_runtime_rejects_unsafe_env_and_permissions() {
    assert!(
        normalize_token_policy(Some(json!({
            "runtime": { "secrets": { "env": { "PIPING_API_TOKEN": "external_api_token" } } }
        })))
        .is_err()
    );
    assert!(
        normalize_token_policy(Some(json!({
            "runtime": { "secrets": { "env": { "EXTERNAL_TOKEN": "external_api_token" } } }
        })))
        .is_ok()
    );
    assert!(
        normalize_token_policy(Some(json!({
            "runtime": { "piping_api": { "enabled": true, "inventory": ["delete"] } }
        })))
        .is_err()
    );
    assert!(
        normalize_token_policy(Some(json!({
            "runtime": { "piping_api": { "inventory": ["read"] } }
        })))
        .is_err()
    );
    assert!(
        normalize_token_policy(Some(json!({
            "runtime": { "secrets": { "env": { "lowercase": "external_api_token" } } }
        })))
        .is_err()
    );
}

#[test]
fn user_controlled_json_rejects_plaintext_secret_keys_but_allows_credential_refs() {
    assert!(
        validate_metadata(json!({"nested": {"password": "plain"}})).is_err(),
        "metadata must not persist plaintext password keys"
    );
    assert!(
        validate_metadata(json!({"token_value": "pjt_raw"})).is_err(),
        "metadata must not persist raw token keys"
    );
    assert!(
        normalize_json_object(
            Some(json!({"artifact": {"credential_private_key": "plain"}})),
            "default_input",
        )
        .is_err(),
        "default input must not persist plaintext private-key fields"
    );
    assert!(
        normalize_json_object(
            Some(json!({
                "launch_options": {
                    "credential_choice": "connection_config",
                    "credential_account_id": "019e8400-0000-7000-8000-000000000001",
                    "credential_account_entry_id": "config:connconf:ssh:0",
                    "credential_merge_policy": "context_first"
                }
            })),
            "metadata",
        )
        .is_ok(),
        "credential selection references are safe and must remain configurable"
    );
}

#[test]
fn user_controlled_json_rejects_common_env_secret_names() {
    assert!(
        normalize_json_object(
            Some(json!({ "AWS_SECRET_ACCESS_KEY": "inline-secret" })),
            "env",
        )
        .is_err()
    );
    assert!(
        normalize_json_object(Some(json!({ "OPENAI_API_KEY": "inline-secret" })), "env").is_err()
    );
    assert!(normalize_json_object(Some(json!({ "apiKey": "inline-secret" })), "env").is_err());
    assert!(
        normalize_json_object(Some(json!({ "secretAccessKey": "inline-secret" })), "env").is_err()
    );
    assert!(normalize_json_object(Some(json!({ "token": "inline-secret" })), "env").is_err());
    assert!(normalize_json_object(Some(json!({ "SAFE_MODE": "1" })), "env").is_ok());
}

#[test]
fn normalize_job_template_credential_refs_accepts_references_only() {
    let account_id = Uuid::now_v7();
    let normalized = normalize_job_template_credential_refs(Some(json!([
        {
            "account_kind": "shared_credential_set",
            "account_id": account_id.to_string(),
            "purpose": "machine"
        }
    ])))
    .unwrap();
    assert_eq!(
        normalized,
        json!([
            {
                "account_kind": "shared_credential_set",
                "account_id": account_id.to_string(),
                "purpose": "machine"
            }
        ])
    );
    assert!(normalize_job_template_credential_refs(Some(json!([]))).is_ok());
    assert!(
        normalize_job_template_credential_refs(Some(json!([
            { "account_kind": "shared_credential_set", "password": "secret" }
        ])))
        .is_err()
    );
    assert!(
        normalize_job_template_credential_refs(Some(json!([
            {
                "account_kind": "shared_credential_set",
                "account_id": account_id.to_string(),
                "username": "operator"
            }
        ])))
        .is_err()
    );
    assert!(
        normalize_job_template_credential_refs(Some(json!([
            { "account_kind": "shared_credential_set", "account_id": "not-a-uuid" }
        ])))
        .is_err()
    );
}

#[test]
fn normalize_project_scm_url_allows_safe_git_urls_only() {
    assert_eq!(
        normalize_project_scm_url(Some(" https://git.example/repo.git ".to_string()), "git")
            .unwrap(),
        Some("https://git.example/repo.git".to_string())
    );
    assert_eq!(
        normalize_project_scm_url(Some("ssh://git@git.example/repo.git".to_string()), "git")
            .unwrap(),
        Some("ssh://git@git.example/repo.git".to_string())
    );
    assert!(
        normalize_project_scm_url(Some("http://git.example/repo.git".to_string()), "git").is_err()
    );
    assert!(normalize_project_scm_url(Some("file:///etc/passwd".to_string()), "git").is_err());
    assert!(
        normalize_project_scm_url(
            Some("https://token@git.example/repo.git".to_string()),
            "git"
        )
        .is_err()
    );
}

#[test]
fn normalize_version_timeout_bounds_execution_snapshot() {
    assert_eq!(normalize_version_timeout(None).unwrap(), 1800);
    assert_eq!(normalize_version_timeout(Some(1)).unwrap(), 1);
    assert_eq!(normalize_version_timeout(Some(86400)).unwrap(), 86400);
    assert!(normalize_version_timeout(Some(0)).is_err());
    assert!(normalize_version_timeout(Some(86401)).is_err());
}

#[test]
fn normalize_manual_run_trigger_kind_accepts_interactive_sources_only() {
    assert_eq!(
        super::normalize_manual_run_trigger_kind(None).unwrap(),
        PlaybookRunTriggerKind::Manual
    );
    for trigger_kind in [
        PlaybookRunTriggerKind::Manual,
        PlaybookRunTriggerKind::Api,
        PlaybookRunTriggerKind::Mcp,
        PlaybookRunTriggerKind::Bulk,
    ] {
        assert_eq!(
            super::normalize_manual_run_trigger_kind(Some(trigger_kind)).unwrap(),
            trigger_kind
        );
    }
    assert!(
        super::normalize_manual_run_trigger_kind(Some(PlaybookRunTriggerKind::Schedule)).is_err()
    );
}

#[test]
fn normalize_scheduled_run_trigger_kind_accepts_schedule_only() {
    assert_eq!(
        super::normalize_scheduled_run_trigger_kind(None).unwrap(),
        PlaybookRunTriggerKind::Schedule
    );
    assert_eq!(
        super::normalize_scheduled_run_trigger_kind(Some(PlaybookRunTriggerKind::Schedule))
            .unwrap(),
        PlaybookRunTriggerKind::Schedule
    );
    for trigger_kind in [
        PlaybookRunTriggerKind::Manual,
        PlaybookRunTriggerKind::Api,
        PlaybookRunTriggerKind::Mcp,
        PlaybookRunTriggerKind::Bulk,
    ] {
        assert!(
            super::normalize_scheduled_run_trigger_kind(Some(trigger_kind)).is_err(),
            "{trigger_kind:?} must not be accepted by scheduled run groups"
        );
    }
}

#[test]
fn normalize_artifact_input_requires_content_and_hashes_payload() {
    let run_id = Uuid::from_u128(9);
    let artifact = normalize_artifact_input(
        run_id,
        CreateArtifactInput {
            artifact_type: " linux_inventory ".to_string(),
            name: " inventory.json ".to_string(),
            content_type: None,
            content_json: Some(json!({"host":"node-1"})),
            content_text: None,
        },
    )
    .unwrap();
    assert_eq!(artifact.run_id, run_id);
    assert_eq!(artifact.artifact_type, "linux_inventory");
    assert_eq!(artifact.name, "inventory.json");
    assert_eq!(artifact.content_type, "application/json");
    assert_eq!(artifact.content_sha256.len(), 64);
    assert!(artifact.content_size_bytes > 0);

    assert!(
        normalize_artifact_input(
            run_id,
            CreateArtifactInput {
                artifact_type: "linux_inventory".to_string(),
                name: "inventory.json".to_string(),
                content_type: None,
                content_json: None,
                content_text: None,
            },
        )
        .is_err()
    );
    assert!(
        normalize_artifact_input(
            run_id,
            CreateArtifactInput {
                artifact_type: "linux_inventory".to_string(),
                name: "inventory.json".to_string(),
                content_type: None,
                content_json: Some(json!([])),
                content_text: None,
            },
        )
        .is_err()
    );
    assert!(
        normalize_artifact_input(
            run_id,
            CreateArtifactInput {
                artifact_type: "linux_inventory".to_string(),
                name: "inventory.json".to_string(),
                content_type: None,
                content_json: Some(json!({"host":"node-1"})),
                content_text: Some("raw".to_string()),
            },
        )
        .is_err()
    );
    assert!(
        normalize_artifact_input(
            run_id,
            CreateArtifactInput {
                artifact_type: "linux_inventory".to_string(),
                name: "inventory.json".to_string(),
                content_type: None,
                content_json: Some(json!({"accounts": [{"private_key": "plain"}]})),
                content_text: None,
            },
        )
        .is_err(),
        "artifact JSON must reject plaintext private-key fields"
    );
}

#[test]
fn normalize_run_event_type_accepts_machine_names_only() {
    assert_eq!(
        normalize_run_event_type(" Run.Started ".to_string()).unwrap(),
        "run.started"
    );
    assert_eq!(
        normalize_run_event_type("stdout:chunk-1".to_string()).unwrap(),
        "stdout:chunk-1"
    );
    assert!(normalize_run_event_type("".to_string()).is_err());
    assert!(normalize_run_event_type("human readable".to_string()).is_err());
    assert!(normalize_run_event_type("x".repeat(121)).is_err());
}

#[test]
fn run_event_message_is_truncated_with_marker_instead_of_rejected() {
    let long = "a".repeat(PLAYBOOK_RUN_EVENT_MESSAGE_MAX_CHARS + 100);
    let normalized = normalize_run_event_message(Some(long), "runner_on_ok").expect("message kept");
    assert_eq!(
        normalized.chars().count(),
        PLAYBOOK_RUN_EVENT_MESSAGE_MAX_CHARS + PLAYBOOK_RUN_EVENT_TRUNCATION_MARKER.chars().count()
    );
    assert!(normalized.ends_with(PLAYBOOK_RUN_EVENT_TRUNCATION_MARKER));

    let short = normalize_run_event_message(Some("  ok: [host-1]  ".to_string()), "runner_on_ok");
    assert_eq!(short.as_deref(), Some("ok: [host-1]"));
    assert_eq!(
        normalize_run_event_message(Some("   ".to_string()), "runner_on_ok"),
        None
    );
    assert_eq!(normalize_run_event_message(None, "runner_on_ok"), None);
}

#[test]
fn raw_console_line_message_keeps_derived_cap_overflow_and_clips_only_at_64kib() {
    // A line longer than the derived-event cap must survive verbatim on the
    // raw 1:1 stream.
    let over_derived_cap = "b".repeat(PLAYBOOK_RUN_EVENT_MESSAGE_MAX_CHARS + 1);
    assert_eq!(
        normalize_run_event_message(Some(over_derived_cap.clone()), "console_line").as_deref(),
        Some(over_derived_cap.as_str())
    );

    let extreme = "c".repeat(PLAYBOOK_RUN_EVENT_RAW_MESSAGE_MAX_CHARS + 1);
    let clipped = normalize_run_event_message(Some(extreme), "console_line").expect("message kept");
    assert!(clipped.ends_with(PLAYBOOK_RUN_EVENT_TRUNCATION_MARKER));
    assert_eq!(
        clipped.chars().count(),
        PLAYBOOK_RUN_EVENT_RAW_MESSAGE_MAX_CHARS
            + PLAYBOOK_RUN_EVENT_TRUNCATION_MARKER.chars().count()
    );
}

#[test]
fn run_event_payload_masks_secret_values_instead_of_rejecting_batch() {
    let payload = json!({
        "host": "cash-1",
        "res": {
            "token": "super-secret",
            "ansible_facts": {"api_key": "k", "os_family": "Debian"},
            "results": [{"password": "p", "name": "unit"}],
        },
    });
    let normalized =
        normalize_run_event_payload(Some(payload)).expect("legit ansible payload accepted");
    assert_eq!(
        normalized,
        json!({
            "host": "cash-1",
            "res": {
                "token": "***",
                "ansible_facts": {"api_key": "***", "os_family": "Debian"},
                "results": [{"password": "***", "name": "unit"}],
            },
        })
    );
    // Missing payload still defaults to an empty object.
    assert_eq!(normalize_run_event_payload(None).unwrap(), json!({}));
    // Structural validation still rejects non-objects.
    assert!(normalize_run_event_payload(Some(json!("text"))).is_err());
    assert!(normalize_run_event_payload(Some(json!([1, 2]))).is_err());
}

#[test]
fn group_event_stream_dedupes_cross_run_hostless_events_only() {
    let group_id = Uuid::from_u128(1);
    let run_a = Uuid::from_u128(2);
    let run_b = Uuid::from_u128(3);
    let now = Utc::now();

    let duplicate = PlaybookRunEvent {
        id: Uuid::from_u128(10),
        run_group_id: group_id,
        run_id: Some(run_a),
        host: None,
        event_sequence: 1,
        event_kind: PlaybookRunEventKind::Stdout,
        event_type: "playbook_on_start".to_string(),
        message: Some("PLAY [Inventory]".to_string()),
        payload: json!({"event":"playbook_on_start","uuid":"same"}),
        created_at: now,
    };
    let mut second_run_duplicate = duplicate.clone();
    second_run_duplicate.id = Uuid::from_u128(11);
    second_run_duplicate.run_id = Some(run_b);
    second_run_duplicate.event_sequence = 2;

    let mut same_run_repeat = duplicate.clone();
    same_run_repeat.id = Uuid::from_u128(14);
    same_run_repeat.event_sequence = 5;

    let mut host_a = duplicate.clone();
    host_a.id = Uuid::from_u128(12);
    host_a.event_sequence = 3;
    host_a.host = Some("cash-1".to_string());
    host_a.event_type = "runner_ok".to_string();
    host_a.message = Some("ok: [cash-1]".to_string());
    host_a.payload = json!({"host":"cash-1"});

    let mut host_b = host_a.clone();
    host_b.id = Uuid::from_u128(13);
    host_b.event_sequence = 4;
    host_b.host = Some("cash-2".to_string());
    host_b.message = Some("ok: [cash-2]".to_string());
    host_b.payload = json!({"host":"cash-2"});

    let events = dedupe_group_hostless_events(vec![
        duplicate,
        second_run_duplicate,
        same_run_repeat,
        host_a,
        host_b,
    ]);

    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "playbook_on_start")
            .count(),
        2
    );
    assert_eq!(
        events.iter().filter(|event| event.host.is_some()).count(),
        2
    );
}

#[test]
fn normalize_lease_owner_trims_and_rejects_empty_or_long_values() {
    assert_eq!(
        normalize_lease_owner(" worker-1 ".to_string()).unwrap(),
        "worker-1"
    );
    assert!(normalize_lease_owner(" ".to_string()).is_err());
    assert!(normalize_lease_owner("x".repeat(121)).is_err());
}

#[test]
fn normalize_static_member_allows_connection_targets_only() {
    let target_id = Uuid::from_u128(17);
    let member = normalize_static_member(StaticTargetMemberInput {
        target_kind: " Connection_Target ".to_string(),
        target_id,
        source_kind: " Device ".to_string(),
        source_id: None,
        sort_order: Some(3),
    })
    .unwrap();
    assert_eq!(member.target_kind, "connection_target");
    assert_eq!(member.target_id, target_id);
    assert_eq!(member.source_kind, "device");
    assert_eq!(member.source_id, target_id);
    assert_eq!(member.sort_order, 3);

    for target_kind in ["device", "virtual_machine", "network_device", "host_agent"] {
        assert!(
            normalize_static_member(StaticTargetMemberInput {
                target_kind: target_kind.to_string(),
                target_id,
                source_kind: "device".to_string(),
                source_id: None,
                sort_order: None,
            })
            .is_err(),
            "{target_kind} must not be executable playbook target kind"
        );
    }
    assert!(
        normalize_static_member(StaticTargetMemberInput {
            target_kind: "connection_target".to_string(),
            target_id,
            source_kind: "host".to_string(),
            source_id: None,
            sort_order: None,
        })
        .is_err()
    );
}

#[test]
fn normalize_terminal_run_status_allows_only_terminal_values() {
    assert!(normalize_terminal_run_status(PlaybookRunStatus::Succeeded).is_ok());
    assert!(normalize_terminal_run_status(PlaybookRunStatus::Failed).is_ok());
    assert!(normalize_terminal_run_status(PlaybookRunStatus::Canceled).is_ok());
    assert!(normalize_terminal_run_status(PlaybookRunStatus::TimedOut).is_ok());
    assert!(normalize_terminal_run_status(PlaybookRunStatus::Running).is_err());
    assert!(normalize_terminal_run_status(PlaybookRunStatus::Queued).is_err());
}
