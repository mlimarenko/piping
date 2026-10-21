//! Playbook persistence repository.

use crate::contexts::shared::delete_guard::{BlockerParentJoin, BlockerTableMetadata};
use crate::contexts::shared::pagination::PageResult;
use crate::errors::AppError;
use sqlx::PgPool;

mod callback_tx;
mod cancel_tx;
mod event_rows;
mod list_columns;
mod preview;
mod records;
mod repository;
mod row_mappers;
mod run_metadata;
mod scheduling;
mod selector;
mod sql_strings;
mod sqlx_impl;
mod version_lookup;

pub use list_columns::{
    PLAYBOOK_ASSIGNMENT_COLUMNS, PLAYBOOK_RUN_COLUMNS, PLAYBOOK_RUN_GROUP_COLUMNS,
    PLAYBOOK_SCHEDULE_COLUMNS, PLAYBOOK_SERVICE_PRINCIPAL_COLUMNS, PLAYBOOK_TARGET_SET_COLUMNS,
    PlaybookAssignmentListFilter, PlaybookRunListFilter, PlaybookScheduleListFilter,
    PlaybookServicePrincipalListFilter, PlaybookTargetSetListFilter,
};
pub use records::{
    AppendRunGroupEventBatchOutcome, CallbackTokenAuth, CompleteRunBatchItemRecord,
    CompleteRunGroupBatchRecord, CompleteRunRecord, CompletedRunGroupBatchRecord,
    CompletedRunRecord, CreateArtifactRecord, CreateAssignmentRecord,
    CreateExecutionEnvironmentRecord, CreateJobTemplateRecord, CreateJobTokenRecord,
    CreatePlaybookRecord, CreatePlaybookVersionRecord, CreateProjectRecord, CreateRunEventRecord,
    CreateRunGroupCallbackTokenRecord, CreateRunGroupEventRecord, CreateRunGroupRecord,
    CreateScheduleRecord, CreateServicePrincipalRecord, CreateTargetSetRecord,
    CreatedRunGroupRecord, DueScheduleRecord, FinalizeRunGroupCancelRecord,
    FinalizedRunGroupCancelRecord, LeaseQueuedRunsRecord, LeaseScheduleOccurrencesRecord,
    LeasedScheduleOccurrenceRecord, MarkRunRunningRecord, MarkRunsRunningRecord,
    ReleaseScheduleOccurrenceRecord, RequeueStartedRunsRecord, RevokeServicePrincipalRecord,
    RunGroupWithRuns, ScheduleOccurrenceAdvanceRecord, SetActiveRunTimeoutsRecord,
    StartLeasedRunRecord, StartLeasedRunsRecord, StartedRunRecord, StaticTargetMemberRecord,
    TimedOutRunsRecord, TimeoutStuckRunsRecord, UpdateAssignmentRecord, UpdatePlaybookRecord,
    UpdateScheduleRecord, UpdateTargetSetRecord,
};
pub use repository::PlaybookRepository;

fn page_result<T>(
    items: Vec<T>,
    total: i64,
    limit: u32,
    offset: u32,
    resource: &str,
) -> Result<PageResult<T>, AppError> {
    let total = u64::try_from(total)
        .map_err(|_| AppError::Internal(format!("{resource} total count overflow")))?;
    Ok(PageResult {
        items,
        total,
        limit,
        offset,
    })
}

fn playbook_blocker_metadata(table: &str) -> BlockerTableMetadata {
    match table {
        "playbook_run_group" => BlockerTableMetadata {
            parent_join: Some(BlockerParentJoin {
                fk: "playbook_id",
                table: "playbook",
            }),
            entity_key: Some("Playbook Run Groups"),
        },
        "playbook_run" => BlockerTableMetadata {
            parent_join: Some(BlockerParentJoin {
                fk: "playbook_id",
                table: "playbook",
            }),
            entity_key: Some("Playbook Runs"),
        },
        "playbook_schedule" => BlockerTableMetadata {
            parent_join: None,
            entity_key: Some("Playbook Schedules"),
        },
        _ => BlockerTableMetadata::default(),
    }
}

#[derive(Clone)]
pub struct SqlxPlaybookRepository {
    pool: PgPool,
}

impl SqlxPlaybookRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[cfg(test)]
mod tests {
    use super::callback_tx::terminal_failure_event_type;
    use super::run_metadata::{
        launch_options_metadata, merge_playbook_launch_defaults, run_connection_from_metadata,
        run_credential_decision_from_metadata, run_credential_metadata,
        run_timeout_policy_metadata, run_timeout_seconds_for_target_count,
        run_timeout_seconds_from_metadata,
    };
    use super::selector::{
        prune_role_from_selector, push_dynamic_target_order, push_target_selector_predicates,
        selector_bool, selector_connection_protocols, selector_connection_target_kinds,
        selector_name_list, selector_runtime_capabilities, selector_sort, selector_source_kinds,
        selector_string_list, selector_text, selector_uuid_list,
    };
    use super::{CreateRunGroupRecord, page_result};
    use crate::contexts::playbooks::domain::{PlaybookConnection, PlaybookRunTriggerKind};
    use crate::errors::AppError;
    use serde_json::json;
    use sqlx::{Postgres, QueryBuilder};
    use uuid::Uuid;

    #[test]
    fn page_result_preserves_page_metadata_and_rejects_negative_totals() {
        let page = page_result(vec!["first", "second"], 4, 2, 2, "playbooks").unwrap();

        assert_eq!(page.items, vec!["first", "second"]);
        assert_eq!(page.total, 4);
        assert_eq!(page.limit, 2);
        assert_eq!(page.offset, 2);
        assert!(matches!(
            page_result::<()>(Vec::new(), -1, 10, 0, "playbooks"),
            Err(AppError::Internal(message)) if message == "playbooks total count overflow"
        ));
    }

    #[test]
    fn selector_facets_preserve_names_and_enforce_boolean_types() {
        let selector = json!({
            "site": [" Frankfurt ", "DC-West", "   "],
            "enabled": false
        });

        assert_eq!(
            selector_name_list(&selector, "site").unwrap(),
            vec!["Frankfurt", "DC-West"]
        );
        assert_eq!(
            selector_name_list(&json!({}), "site").unwrap(),
            Vec::<String>::new()
        );
        assert!(selector_name_list(&json!({"site": "Frankfurt"}), "site").is_err());
        assert!(selector_name_list(&json!({"site": [7]}), "site").is_err());
        assert_eq!(selector_bool(&selector, "enabled").unwrap(), Some(false));
        assert_eq!(selector_bool(&selector, "missing").unwrap(), None);
        assert!(selector_bool(&json!({"enabled": "false"}), "enabled").is_err());
    }

    #[test]
    fn run_connection_uses_valid_override_and_falls_back_to_playbook_default() {
        assert_eq!(
            run_connection_from_metadata(PlaybookConnection::Ssh, &json!({})).unwrap(),
            PlaybookConnection::Ssh
        );
        assert_eq!(
            run_connection_from_metadata(
                PlaybookConnection::Ssh,
                &json!({"launch_options": {"connection": " WinRM "}}),
            )
            .unwrap(),
            PlaybookConnection::Winrm
        );
        assert!(
            run_connection_from_metadata(
                PlaybookConnection::Local,
                &json!({"launch_options": {"connection": "telnet"}}),
            )
            .is_err()
        );
        assert_eq!(
            run_connection_from_metadata(
                PlaybookConnection::Local,
                &json!({"launch_options": {"connection": 7}}),
            )
            .unwrap(),
            PlaybookConnection::Local
        );
    }

    #[test]
    fn prune_role_from_selector_removes_deleted_role_name_and_id() {
        let role_id = Uuid::from_u128(42);
        let selector = json!({
            "role": ["web", "deleted", 7],
            "role_ids": [role_id.to_string(), Uuid::from_u128(7).to_string()],
            "site": ["dc1"]
        });

        let pruned = prune_role_from_selector(&selector, role_id, "deleted");

        assert_eq!(
            pruned,
            json!({
                "role": ["web", 7],
                "role_ids": [Uuid::from_u128(7).to_string()],
                "site": ["dc1"]
            })
        );
    }

    #[test]
    fn prune_role_from_selector_removes_empty_role_keys() {
        let role_id = Uuid::from_u128(42);
        let selector = json!({
            "role": "deleted",
            "role_ids": [role_id.to_string()],
            "status": ["active"]
        });

        let pruned = prune_role_from_selector(&selector, role_id, "deleted");

        assert_eq!(pruned, json!({ "status": ["active"] }));
    }

    #[test]
    fn run_credential_metadata_records_safe_refs_and_policy_only() {
        let user_id = Uuid::from_u128(7);
        let service_principal_id = Uuid::from_u128(9);
        let base = CreateRunGroupRecord {
            playbook_id: Uuid::from_u128(1),
            target_set_id: Uuid::from_u128(2),
            schedule_id: None,
            scheduled_for: None,
            trigger_kind: PlaybookRunTriggerKind::Manual,
            requested_by_user_id: Some(user_id),
            service_principal_id: None,
            metadata: json!({}),
            target_limit: 50,
            secret_env_authorization: None,
        };

        let decision = run_credential_decision_from_metadata(&json!({})).unwrap();
        let metadata = run_credential_metadata(&base, &decision);
        assert_eq!(metadata["subject_kind"], "user");
        assert_eq!(metadata["subject_id"], user_id.to_string());
        assert_eq!(metadata["account"]["kind"], "auto");
        assert_eq!(metadata["account"]["id"], json!(null));
        assert_eq!(metadata["account"]["entry_id"], json!(null));
        assert_eq!(metadata["account"]["merge_policy"], "context_first");
        assert_eq!(metadata["resolution"]["status"], "pending");
        assert_eq!(metadata["resolution"]["selection"], "auto");
        assert_eq!(metadata["resolution"]["source"], "launch_options");
        assert_eq!(metadata["resolution"]["materialization"], "worker_runtime");
        assert_eq!(metadata["secret_material"], "runtime_only");

        let scheduled = CreateRunGroupRecord {
            schedule_id: Some(Uuid::from_u128(3)),
            scheduled_for: Some(chrono::Utc::now()),
            trigger_kind: PlaybookRunTriggerKind::Schedule,
            requested_by_user_id: None,
            service_principal_id: Some(service_principal_id),
            ..base
        };
        let metadata = run_credential_metadata(&scheduled, &decision);
        assert_eq!(metadata["subject_kind"], "service_principal");
        assert_eq!(metadata["subject_id"], service_principal_id.to_string());
    }

    #[test]
    fn ansible_terminal_failure_events_drive_run_progress() {
        for event_type in [
            "runner_on_fatal",
            "runner_on_failed",
            "runner_on_unreachable",
        ] {
            assert!(
                terminal_failure_event_type(event_type),
                "{event_type} must mark its host run failed before final completion"
            );
        }
        for event_type in ["stdout", "runner_on_ok", "playbook_on_stats", "run_started"] {
            assert!(
                !terminal_failure_event_type(event_type),
                "{event_type} must not terminally fail its host run"
            );
        }
    }

    #[test]
    fn run_credential_decision_records_references_without_materializing_credentials() {
        let account_id = Uuid::from_u128(0xfeed);
        let decision = run_credential_decision_from_metadata(&json!({
            "launch_options": {
                "credential_choice": "shared_credential_set",
                "credential_account_id": account_id.to_string(),
                "credential_account_entry_id": "root",
                "credential_merge_policy": "personal_first"
            }
        }))
        .unwrap();

        assert_eq!(decision.account_kind, "shared_credential_set");
        assert_eq!(decision.account_id, Some(account_id));
        assert_eq!(decision.account_entry_id.as_deref(), Some("root"));
        assert_eq!(decision.merge_policy, "personal_first");
        assert_eq!(decision.resolution["status"], "pending");
        assert_eq!(decision.resolution["selection"], "explicit");
        assert_eq!(decision.resolution["account_kind"], "shared_credential_set");
        assert_eq!(decision.resolution["account_id"], account_id.to_string());
        assert_eq!(decision.resolution["entry_id"], "root");
        assert_eq!(decision.resolution["merge_policy"], "personal_first");
        assert_eq!(decision.resolution["source"], "launch_options");
    }

    #[test]
    fn launch_options_metadata_copies_object_only() {
        assert_eq!(
            launch_options_metadata(&json!({
                "launch_options": {
                    "timeout_seconds": 42,
                    "dry_run": true,
                    "extravars": { "package": "htop" }
                }
            }))
            .unwrap(),
            json!({
                "timeout_seconds": 42,
                "dry_run": true,
                "extravars": { "package": "htop" }
            })
        );
        assert!(launch_options_metadata(&json!({ "launch_options": [] })).is_none());
        assert!(launch_options_metadata(&json!({})).is_none());
    }

    #[test]
    fn playbook_launch_defaults_fill_missing_run_options() {
        let merged = merge_playbook_launch_defaults(
            &json!({
                "launch_options": {
                    "compact_events": true,
                    "strategy": "free",
                    "ansible_timeout": 12
                }
            }),
            &json!({
                "trigger": "manual",
                "launch_options": {
                    "strategy": "linear",
                    "timeout_seconds": 300
                }
            }),
        );

        assert_eq!(
            merged,
            json!({
                "trigger": "manual",
                "launch_options": {
                    "compact_events": true,
                    "strategy": "linear",
                    "ansible_timeout": 12,
                    "timeout_seconds": 300
                }
            })
        );
    }

    #[test]
    fn factory_playbook_launch_defaults_do_not_override_explicit_run_options() {
        let merged = merge_playbook_launch_defaults(
            &json!({
                "system": true,
                "factory": true,
                "launch_options": {
                    "forks": 128,
                    "compact_events": true,
                    "strategy": "free",
                    "ansible_timeout": 12
                }
            }),
            &json!({
                "trigger": "manual",
                "launch_options": {
                    "forks": 256,
                    "compact_events": false,
                    "strategy": "linear",
                    "timeout_seconds": 300,
                    "credential_choice": "personal_default"
                }
            }),
        );

        assert_eq!(
            merged,
            json!({
                "trigger": "manual",
                "launch_options": {
                    "forks": 256,
                    "compact_events": false,
                    "strategy": "linear",
                    "ansible_timeout": 12,
                    "timeout_seconds": 300,
                    "credential_choice": "personal_default"
                }
            })
        );
    }

    #[test]
    fn playbook_launch_defaults_create_missing_options_object() {
        let merged = merge_playbook_launch_defaults(
            &json!({
                "launch_options": {
                    "compact_events": true,
                    "strategy": "free"
                }
            }),
            &json!({ "trigger": "manual" }),
        );

        assert_eq!(
            merged,
            json!({
                "trigger": "manual",
                "launch_options": {
                    "compact_events": true,
                    "strategy": "free"
                }
            })
        );
    }

    #[test]
    fn run_timeout_seconds_from_metadata_accepts_override_and_default() {
        assert_eq!(
            run_timeout_seconds_from_metadata(
                &json!({ "launch_options": { "timeout_seconds": 42 } }),
                1800,
            )
            .unwrap(),
            42
        );
        assert_eq!(
            run_timeout_seconds_from_metadata(&json!({}), 1800).unwrap(),
            1800
        );
        assert_eq!(
            run_timeout_seconds_from_metadata(
                &json!({ "launch_options": { "timeout_seconds": null } }),
                1800,
            )
            .unwrap(),
            1800
        );
    }

    #[test]
    fn run_timeout_seconds_from_metadata_rejects_invalid_override() {
        for metadata in [
            json!({ "launch_options": { "timeout_seconds": 0 } }),
            json!({ "launch_options": { "timeout_seconds": 86_401 } }),
            json!({ "launch_options": { "timeout_seconds": "fast" } }),
        ] {
            assert!(run_timeout_seconds_from_metadata(&metadata, 1800).is_err());
        }
    }

    #[test]
    fn run_timeout_seconds_for_target_count_scales_default_by_fork_waves() {
        let metadata = json!({ "launch_options": { "forks": 256 } });

        assert_eq!(
            run_timeout_seconds_for_target_count(&metadata, 1_800, 256).unwrap(),
            1_800
        );
        assert_eq!(
            run_timeout_seconds_for_target_count(&metadata, 1_800, 257).unwrap(),
            3_600
        );
        assert_eq!(
            run_timeout_seconds_for_target_count(&metadata, 1_800, 1_697).unwrap(),
            12_600
        );
    }

    #[test]
    fn run_timeout_seconds_for_target_count_scales_explicit_override_by_fork_waves() {
        let metadata = json!({ "launch_options": { "forks": 256, "timeout_seconds": 300 } });

        assert_eq!(
            run_timeout_seconds_for_target_count(&metadata, 1_800, 1_697).unwrap(),
            2_100
        );
    }

    #[test]
    fn run_timeout_seconds_for_target_count_caps_scaled_default() {
        let metadata = json!({ "launch_options": { "forks": 1 } });

        assert_eq!(
            run_timeout_seconds_for_target_count(&metadata, 1_800, 100).unwrap(),
            86_400
        );
    }

    #[test]
    fn run_timeout_seconds_for_target_count_rejects_invalid_forks() {
        for metadata in [
            json!({ "launch_options": { "forks": 0 } }),
            json!({ "launch_options": { "forks": "256" } }),
        ] {
            assert!(run_timeout_seconds_for_target_count(&metadata, 1_800, 100).is_err());
        }
    }

    #[test]
    fn run_timeout_policy_metadata_records_scaled_group_timeout() {
        let metadata = json!({ "launch_options": { "forks": 256 } });

        let policy =
            run_timeout_policy_metadata(&metadata, 1_800, 1_697, 12_600).expect("valid policy");

        assert_eq!(policy["source"], "run_group");
        assert_eq!(policy["base_timeout_seconds"], 1_800);
        assert_eq!(policy["timeout_seconds"], 12_600);
        assert_eq!(policy["target_count"], 1_697);
        assert_eq!(policy["requested_forks"], 256);
        assert_eq!(policy["effective_forks"], 256);
        assert_eq!(policy["waves"], 7);
        assert_eq!(policy["explicit_override"], false);
    }

    #[test]
    fn run_timeout_policy_metadata_marks_explicit_timeout_override() {
        let metadata = json!({ "launch_options": { "forks": 256, "timeout_seconds": 300 } });

        let policy =
            run_timeout_policy_metadata(&metadata, 1_800, 1_697, 2_100).expect("valid policy");

        assert_eq!(policy["base_timeout_seconds"], 300);
        assert_eq!(policy["timeout_seconds"], 2_100);
        assert_eq!(policy["waves"], 7);
        assert_eq!(policy["explicit_override"], true);
    }

    #[test]
    fn selector_uuid_list_accepts_uuid_arrays() {
        let id = Uuid::from_u128(42);
        let selector = json!({ "site_ids": [id.to_string()] });
        assert_eq!(selector_uuid_list(&selector, "site_ids").unwrap(), vec![id]);
        assert!(
            selector_uuid_list(&selector, "region_ids")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn selector_uuid_list_rejects_non_array_or_non_uuid_values() {
        let selector = json!({ "site_ids": "not-array" });
        assert!(selector_uuid_list(&selector, "site_ids").is_err());
        let selector = json!({ "site_ids": ["not-uuid"] });
        assert!(selector_uuid_list(&selector, "site_ids").is_err());
    }

    #[test]
    fn selector_string_list_normalizes_non_empty_strings() {
        let selector = json!({ "target_kind": ["Device", " ", "virtual_machine"] });
        assert_eq!(
            selector_string_list(&selector, "target_kind").unwrap(),
            vec!["device".to_string(), "virtual_machine".to_string()]
        );
    }

    #[test]
    fn selector_connection_target_kinds_rejects_direct_inventory_execution_kinds() {
        let selector = json!({ "target_kind": [" Connection_Target "] });
        assert_eq!(
            selector_connection_target_kinds(&selector).unwrap(),
            vec!["connection_target".to_string()]
        );

        for target_kind in ["device", "virtual_machine", "network_device", "host_agent"] {
            let selector = json!({ "target_kind": [target_kind] });
            assert!(
                selector_connection_target_kinds(&selector).is_err(),
                "{target_kind} must not be an executable playbook selector kind"
            );
        }
    }

    #[test]
    fn selector_source_kind_is_source_filter_not_execution_kind() {
        let selector = json!({ "source_kind": [" Device ", "virtual_machine"] });
        assert_eq!(
            selector_source_kinds(&selector).unwrap(),
            vec!["device".to_string(), "virtual_machine".to_string()]
        );
        assert!(selector_source_kinds(&json!({ "source_kind": ["host"] })).is_err());
    }

    #[test]
    fn selector_runtime_ansible_adds_bound_address_predicate() {
        let selector = json!({ "runtime": ["ansible"] });
        assert_eq!(
            selector_runtime_capabilities(&selector).unwrap(),
            vec!["ansible".to_string()]
        );
        assert!(selector_runtime_capabilities(&json!({ "runtime": ["linux_ssh"] })).is_err());
        assert_eq!(
            selector_connection_protocols(&json!({ "connection_protocol": ["SSH"] })).unwrap(),
            vec!["ssh".to_string()]
        );
        assert!(selector_connection_protocols(&json!({ "connection_protocol": ["rdp"] })).is_err());

        let mut builder = QueryBuilder::<Postgres>::new("");
        push_target_selector_predicates(&mut builder, &selector).unwrap();
        let sql = builder.sql();
        assert!(sql.contains("connection_targets.address IS NOT NULL"));
        assert!(sql.contains("connection_targets.address_bound = TRUE"));
        assert!(sql.contains("public.connconf_assignment ca"));
        assert!(sql.contains("public.connconf_record cr"));
        assert!(sql.contains("cr.protocol = ANY("));
        assert!(sql.contains("ca.scope_type = 'device'"));
        assert!(sql.contains("ca.scope_type = 'vm'"));
        assert!(
            !sql.contains("ca.scope_type = 'connection_target'"),
            "preview must mirror the connection-config resolver scope model"
        );
    }

    #[test]
    fn selector_connection_protocol_adds_ssh_eligibility_predicate_without_runtime() {
        let selector = json!({ "connection_protocol": ["ssh"] });

        let mut builder = QueryBuilder::<Postgres>::new("");
        push_target_selector_predicates(&mut builder, &selector).unwrap();
        let sql = builder.sql();

        assert!(sql.contains("connection_targets.address IS NOT NULL"));
        assert!(sql.contains("connection_targets.address_bound = TRUE"));
        assert!(sql.contains("public.connconf_assignment ca"));
        assert!(sql.contains("public.connconf_record cr"));
        assert!(sql.contains("cr.protocol = ANY("));
    }

    #[test]
    fn target_selector_name_keyed_facets_filter_by_name_columns() {
        // Regression for bug-1: the structured selector builder filters by display
        // name (site/role/...), so the predicate builder must emit the `*_name`
        // columns. Without the name-facet loop these facets were silently dropped
        // and a dynamic set resolved to nearly all hosts.
        let selector = json!({ "site": ["Site A"], "role": ["Касса"] });

        let mut builder = QueryBuilder::<Postgres>::new("");
        push_target_selector_predicates(&mut builder, &selector).unwrap();
        let sql = builder.sql();
        assert!(sql.contains("connection_targets.site_name = ANY("));
        assert!(sql.contains("connection_targets.role_name = ANY("));
    }

    #[test]
    fn target_selector_supports_safe_targeting_predicates() {
        let id = Uuid::from_u128(99);
        let selector = json!({
            "include_target_ids": [id.to_string()],
            "include_source_ids": [id.to_string()],
            "exclude_target_ids": [id.to_string()],
            "exclude_source_ids": [id.to_string()],
            "device_type_ids": [id.to_string()],
            "tag_slugs": ["linux"],
            "address_bound": true,
            "has_address": true,
        });
        assert_eq!(
            selector_bool(&selector, "address_bound").unwrap(),
            Some(true)
        );

        let mut builder = QueryBuilder::<Postgres>::new("");
        push_target_selector_predicates(&mut builder, &selector).unwrap();
        let sql = builder.sql();
        assert!(sql.contains("connection_targets.target_id = ANY("));
        assert!(sql.contains("connection_targets.source_id = ANY("));
        assert!(sql.contains("NOT (connection_targets.target_id = ANY("));
        assert!(sql.contains("NOT (connection_targets.source_id = ANY("));
        assert!(sql.contains("connection_targets.device_type_id = ANY("));
        assert!(sql.contains("connection_targets.tags @>"));
        assert!(sql.contains("connection_targets.address_bound ="));
        assert!(sql.contains("connection_targets.address IS NOT NULL"));
        assert!(!sql.contains(&id.to_string()));
    }

    #[test]
    fn selector_sort_is_whitelisted_and_deterministic() {
        assert_eq!(
            selector_sort(&json!({"sort": "site", "sort_direction": "desc"})).unwrap(),
            ("site_name", "DESC")
        );
        assert!(selector_sort(&json!({"sort": "name; drop table playbook"})).is_err());
        assert!(selector_sort(&json!({"sort_direction": "sideways"})).is_err());

        let mut builder = QueryBuilder::<Postgres>::new("");
        push_dynamic_target_order(&mut builder, &json!({"sort": "source_kind"})).unwrap();
        let sql = builder.sql();
        assert!(sql.contains("ORDER BY connection_targets.source_kind ASC"));
        assert!(sql.contains("connection_targets.target_id ASC"));
    }

    #[test]
    fn selector_text_trims_empty_values() {
        assert_eq!(
            selector_text(&json!({"search":" edge "}), "search").unwrap(),
            Some("edge".to_string())
        );
        assert_eq!(
            selector_text(&json!({"search":"   "}), "search").unwrap(),
            None
        );
        assert!(selector_text(&json!({"search": 1}), "search").is_err());
    }
}
