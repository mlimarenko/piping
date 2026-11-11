use std::collections::HashSet;
use std::sync::{Arc, Barrier};
use std::thread;

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use uuid::Uuid;

use super::{
    BuiltGroupCallbackTokenRecord, CreateArtifactInput, CreateRunEventInput,
    CreateRunGroupEventInput, PLAYBOOK_JOB_TOKEN_MAX_TTL_SECONDS, PLAYBOOK_JOB_TOKEN_PREFIX,
    PLAYBOOK_NAME_MAX_CHARS, PLAYBOOK_RUN_EVENT_MESSAGE_MAX_CHARS,
    PLAYBOOK_RUN_EVENT_RAW_MESSAGE_MAX_CHARS, PLAYBOOK_RUN_EVENT_TRUNCATION_MARKER,
    StaticTargetMemberInput, build_group_callback_token_record, callback_event_size_bytes,
    callback_token_auth, dedupe_group_hostless_events, hex_sha256, is_url_safe_job_token_secret,
    job_token_has_scope, normalize_artifact_input, normalize_assignment_scope, normalize_choice,
    normalize_cron_expression, normalize_job_token_scopes, normalize_json_object,
    normalize_lease_owner, normalize_name, normalize_optional_range, normalize_project_scm_url,
    normalize_run_event_message, normalize_run_event_payload, normalize_run_event_record,
    normalize_run_event_type, normalize_run_group_event_record, normalize_run_timeout,
    normalize_selector, normalize_static_member,
    normalize_terminal_batch_run_status, normalize_terminal_group_status,
    normalize_terminal_run_status, normalize_timezone, parse_job_token, validate_metadata,
};
use crate::contexts::playbooks::domain::{
    PlaybookAssignmentScopeKind, PlaybookRunEvent, PlaybookRunEventKind, PlaybookRunStatus,
};
use crate::errors::AppError;

#[test]
fn launch_metadata_rejects_overrides_and_inline_secrets() {
    assert!(super::validate_launch_metadata(json!({"note": "maintenance"})).is_ok());
    assert!(
        super::validate_launch_metadata(json!({"launch_options": {"timeout_seconds": 2}})).is_err()
    );
    assert!(super::validate_launch_metadata(json!({"password": "secret"})).is_err());
    assert!(super::validate_launch_metadata(json!([])).is_err());
    for key in ["audit_context", "audit_actor_kind"] {
        assert!(super::validate_launch_metadata(json!({(key): "forged"})).is_err());
    }
}

#[test]
fn launch_audit_metadata_keeps_only_real_request_correlation() {
    use crate::contexts::audit::domain::{AuditContext, AuditDecision};
    use crate::contexts::shared::request_context::RequestContext;
    let request_id = Uuid::new_v4();
    let context = RequestContext::new(
        Some(Uuid::new_v4()),
        "api_token".into(),
        None,
        request_id.to_string(),
        Default::default(),
    );
    let metadata = super::stamp_launch_audit_metadata(json!({}), Some(&context));
    let audit: AuditContext = serde_json::from_value(metadata["audit_context"].clone()).unwrap();
    assert_eq!(audit.correlation.request_id, Some(request_id));
    assert_eq!(audit.correlation.causation_id, None);
    assert_eq!(audit.decision, Some(AuditDecision::Allowed));
    assert_eq!(metadata["audit_actor_kind"], "api_token");
    let metadata = super::stamp_launch_audit_metadata(json!({}), None);
    let audit: AuditContext = serde_json::from_value(metadata["audit_context"].clone()).unwrap();
    assert_eq!(audit.correlation, Default::default());
    assert!(metadata.get("audit_actor_kind").is_none());
    let invalid_request = RequestContext {
        correlation_id: "opaque-non-uuid-request".into(),
        ..context
    };
    let metadata = super::stamp_launch_audit_metadata(json!({}), Some(&invalid_request));
    let audit: AuditContext = serde_json::from_value(metadata["audit_context"].clone()).unwrap();
    assert_eq!(audit.correlation.request_id, None);
}

#[test]
fn schedule_targets_require_concrete_fixed_identities_and_object_selector() {
    use crate::contexts::playbooks::domain::{ResolvedTarget, ScheduleTargets};
    let target = ResolvedTarget {
        hostname: "host".into(),
        target_id: Some(Uuid::new_v4()),
        source_id: Some(Uuid::new_v4()),
    };
    assert!(
        super::validate_schedule_targets(&ScheduleTargets::Fixed {
            targets: vec![target.clone()],
            target_set_id: None,
        })
        .is_ok()
    );
    assert!(
        super::validate_schedule_targets(&ScheduleTargets::Fixed {
            targets: vec![ResolvedTarget {
                source_id: None,
                ..target
            }],
            target_set_id: None,
        })
        .is_err()
    );
    assert!(
        super::validate_schedule_targets(&ScheduleTargets::Dynamic {
            selector: json!([]),
            target_set_id: None,
        })
        .is_err()
    );
    assert!(
        super::validate_schedule_targets(&ScheduleTargets::Dynamic {
            selector: json!({}),
            target_set_id: None,
        })
        .is_ok()
    );
}

#[test]
fn launch_image_requires_repository_and_lowercase_sha256() {
    let digest = "a".repeat(64);
    let (image, pin) = super::split_pinned_image(&format!(
        "registry.example:5000/team/runner@sha256:{digest}"
    ))
    .unwrap();
    assert_eq!(image, "registry.example:5000/team/runner");
    assert_eq!(pin, format!("sha256:{digest}"));
    for image in [
        format!("@sha256:{digest}"),
        format!("runner@sha256:{}", "A".repeat(64)),
        format!("runner@sha256:{}", "a".repeat(63)),
        format!("bad//runner@sha256:{digest}"),
        format!("registry:bad/runner@sha256:{digest}"),
        format!("runner@sha256:{digest}@extra"),
    ] {
        assert!(super::split_pinned_image(&image).is_err(), "{image}");
    }
}

#[test]
fn revision_changes_reject_traversal_and_escaping_symlinks() {
    use crate::contexts::playbooks::domain::{ProjectEntry, ProjectEntryContent, RevisionChange};

    assert!(super::validate_revision_changes(&[]).is_err());
    assert!(
        super::validate_revision_changes(&[RevisionChange::Delete {
            path: "../outside".into(),
        }])
        .is_err()
    );
    assert!(
        super::validate_revision_changes(&[RevisionChange::Rename {
            from: "play.yml".into(),
            to: "../outside".into(),
        }])
        .is_err()
    );
    assert!(
        super::validate_revision_changes(&[RevisionChange::Put {
            entry: ProjectEntry {
                path: "link".into(),
                content: ProjectEntryContent::Symlink {
                    target: "../outside".into()
                },
            },
        }])
        .is_err()
    );
    assert!(
        super::validate_revision_changes(&[RevisionChange::Put {
            entry: ProjectEntry {
                path: "bin/run".into(),
                content: ProjectEntryContent::File {
                    bytes: vec![0, 255],
                    executable: true
                },
            },
        }])
        .is_ok()
    );
}

#[test]
fn workspace_template_validates_execution_inputs() {
    use crate::contexts::playbooks::domain::{
        InventorySelection, JobTemplate, LaunchOptions, RevisionSelection,
    };

    let mut template = JobTemplate {
        id: Uuid::new_v4(),
        name: "deploy".into(),
        project_id: Uuid::new_v4(),
        workdir: ".".into(),
        playbook_paths: vec!["deploy.yml".into()],
        revision: RevisionSelection::Current,
        inventory: InventorySelection::Native {
            sources: vec!["inventory.yml".into()],
            ssh_record_id: None,
        },
        vars_schema: Default::default(),
        options: LaunchOptions {
            argv: vec![],
            extra_vars: json!({}),
            limit: None,
            env: Default::default(),
            secret_refs: Default::default(),
            timeout_seconds: 60,
        },
        execution_environment_id: Uuid::new_v4(),
    };
    assert!(super::validate_workspace_job_template(&template).is_ok());
    template.playbook_paths = vec!["../outside.yml".into()];
    assert!(super::validate_workspace_job_template(&template).is_err());
    template.playbook_paths = vec!["deploy.yml".into()];
    template
        .options
        .env
        .insert("PIPING_CALLBACK_TOKEN".into(), "value".into());
    assert!(super::validate_workspace_job_template(&template).is_err());
    template.options.env.clear();
    template
        .options
        .env
        .insert("DEPLOY_KEY".into(), "value".into());
    template
        .options
        .secret_refs
        .insert("DEPLOY_KEY".into(), Uuid::new_v4());
    assert!(super::validate_workspace_job_template(&template).is_err());
    template.options.env.clear();
    template.options.timeout_seconds = 0;
    assert!(super::validate_workspace_job_template(&template).is_err());
    template.options.timeout_seconds = 60;
    template.inventory = InventorySelection::Native {
        sources: vec![],
        ssh_record_id: None,
    };
    assert!(super::validate_workspace_job_template(&template).is_err());
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
        normalize_choice(" ALWAYS ", &["always", "never"], "pull_policy").unwrap(),
        "always"
    );
    assert!(normalize_choice("sometimes", &["always", "never"], "pull_policy").is_err());
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

#[test]
fn boundary_normalizers_reject_empty_traversing_and_oversized_inputs() {
    assert_identifier_normalizers_reject_invalid_input();
    assert_selector_normalizers_reject_invalid_input();
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
fn normalize_run_timeout_bounds_execution_snapshot() {
    assert_eq!(normalize_run_timeout(None).unwrap(), 1800);
    assert_eq!(normalize_run_timeout(Some(1)).unwrap(), 1);
    assert_eq!(normalize_run_timeout(Some(86400)).unwrap(), 86400);
    assert!(normalize_run_timeout(Some(0)).is_err());
    assert!(normalize_run_timeout(Some(86401)).is_err());
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
    assert!(normalize_terminal_run_status(PlaybookRunStatus::Unreported).is_err());
    assert!(normalize_terminal_run_status(PlaybookRunStatus::Running).is_err());
    assert!(normalize_terminal_run_status(PlaybookRunStatus::Queued).is_err());
    assert!(normalize_terminal_batch_run_status(PlaybookRunStatus::Unreported).is_ok());
    assert!(normalize_terminal_batch_run_status(PlaybookRunStatus::Succeeded).is_ok());
    assert!(normalize_terminal_batch_run_status(PlaybookRunStatus::Running).is_err());
}

#[test]
fn normalize_terminal_group_status_rejects_an_unreported_process_verdict() {
    for status in [
        PlaybookRunStatus::Succeeded,
        PlaybookRunStatus::Failed,
        PlaybookRunStatus::Canceled,
        PlaybookRunStatus::TimedOut,
    ] {
        assert!(normalize_terminal_group_status(status).is_ok());
    }
    assert!(normalize_terminal_group_status(PlaybookRunStatus::Unreported).is_err());
    assert!(normalize_terminal_group_status(PlaybookRunStatus::Running).is_err());
}
