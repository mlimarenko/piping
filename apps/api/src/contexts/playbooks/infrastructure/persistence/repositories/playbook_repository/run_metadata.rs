use super::CreateRunGroupRecord;
use crate::contexts::playbooks::domain::{PlaybookConnection, parse_playbook_connection};
use crate::errors::AppError;
use serde_json::Value;
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub(super) struct RunCredentialDecision {
    pub(super) account_kind: String,
    pub(super) account_id: Option<Uuid>,
    pub(super) account_entry_id: Option<String>,
    pub(super) merge_policy: String,
    pub(super) resolution: Value,
}

pub(super) const PLAYBOOK_RUN_TIMEOUT_MIN_SECONDS: i64 = 1;
pub(super) const PLAYBOOK_RUN_TIMEOUT_MAX_SECONDS: i64 = 86_400;

pub(super) fn run_credential_decision_from_metadata(
    metadata: &Value,
) -> Result<RunCredentialDecision, AppError> {
    let launch_options = metadata.get("launch_options").and_then(Value::as_object);
    let raw_choice = launch_options
        .and_then(|options| options.get("credential_choice"))
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .trim()
        .to_ascii_lowercase();
    let merge_policy = launch_options
        .and_then(|options| {
            options
                .get("credential_merge_policy")
                .or_else(|| options.get("credentialMergePolicy"))
        })
        .and_then(Value::as_str)
        .unwrap_or("context_first")
        .trim()
        .to_ascii_lowercase();
    if !matches!(merge_policy.as_str(), "context_first" | "personal_first") {
        return Err(AppError::Validation(
            "launch_options.credential_merge_policy must be one of: context_first, personal_first"
                .to_string(),
        ));
    }

    let account_entry_id = launch_options
        .and_then(|options| {
            options
                .get("credential_account_entry_id")
                .or_else(|| options.get("credential_entry_id"))
                .or_else(|| options.get("entryId"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.chars().count() > 240 {
                Err(AppError::Validation(
                    "launch_options.credential_account_entry_id is too long".to_string(),
                ))
            } else {
                Ok(value.to_string())
            }
        })
        .transpose()?;

    let explicit_kind = launch_options
        .and_then(|options| {
            options
                .get("credential_account_kind")
                .or_else(|| options.get("account_kind"))
        })
        .and_then(Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase());
    let explicit_id = launch_options
        .and_then(|options| {
            options
                .get("credential_account_id")
                .or_else(|| options.get("account_id"))
                .or_else(|| options.get("credential_set_id"))
        })
        .and_then(Value::as_str)
        .map(|value| Uuid::parse_str(value.trim()))
        .transpose()
        .map_err(|_| {
            AppError::Validation("launch_options.credential_account_id must be a UUID".to_string())
        })?;

    let (account_kind, account_id, selection) = match raw_choice.as_str() {
        "" | "auto" => ("auto".to_string(), None, "auto"),
        "none" => ("none".to_string(), None, "none"),
        "connection_config" | "shared_credential_set" => {
            let Some(account_id) = explicit_id else {
                return Err(AppError::Validation(format!(
                    "launch_options.credential_account_id is required for credential_choice={raw_choice}"
                )));
            };
            (raw_choice, Some(account_id), "explicit")
        }
        "selected" | "explicit" => {
            let Some(account_kind) = explicit_kind else {
                return Err(AppError::Validation(
                    "launch_options.credential_account_kind is required for explicit credential choice"
                        .to_string(),
                ));
            };
            if !matches!(
                account_kind.as_str(),
                "connection_config" | "shared_credential_set"
            ) {
                return Err(AppError::Validation(
                    "launch_options.credential_account_kind must be connection_config or shared_credential_set".to_string(),
                ));
            }
            let Some(account_id) = explicit_id else {
                return Err(AppError::Validation(
                    "launch_options.credential_account_id is required for explicit credential choice"
                        .to_string(),
                ));
            };
            (account_kind, Some(account_id), "explicit")
        }
        other => {
            return Err(AppError::Validation(format!(
                "launch_options.credential_choice is not supported: {other}"
            )));
        }
    };

    Ok(RunCredentialDecision {
        account_kind: account_kind.clone(),
        account_id,
        account_entry_id: account_entry_id.clone(),
        merge_policy: merge_policy.clone(),
        resolution: json!({
            "status": "pending",
            "selection": selection,
            "account_kind": account_kind,
            "account_id": account_id.map(|id| id.to_string()),
            "entry_id": account_entry_id,
            "merge_policy": merge_policy,
            "source": "launch_options"
        }),
    })
}

pub(super) fn launch_options_metadata(metadata: &Value) -> Option<Value> {
    metadata
        .get("launch_options")
        .filter(|value| value.is_object())
        .cloned()
}

pub(super) fn merge_playbook_launch_defaults(
    playbook_metadata: &Value,
    record_metadata: &Value,
) -> Value {
    let Some(default_options) = playbook_metadata
        .get("launch_options")
        .and_then(Value::as_object)
    else {
        return record_metadata.clone();
    };
    if default_options.is_empty() {
        return record_metadata.clone();
    }

    let mut metadata = record_metadata.clone();
    let Value::Object(metadata_object) = &mut metadata else {
        return metadata;
    };
    let launch_options = metadata_object
        .entry("launch_options".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(launch_options) = launch_options.as_object_mut() else {
        return metadata;
    };
    for (name, value) in default_options {
        launch_options
            .entry(name.clone())
            .or_insert_with(|| value.clone());
    }
    metadata
}

pub(super) fn run_connection_from_metadata(
    default_connection: PlaybookConnection,
    metadata: &Value,
) -> Result<PlaybookConnection, AppError> {
    metadata
        .get("launch_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("connection"))
        .and_then(Value::as_str)
        .map(parse_playbook_connection)
        .transpose()
        .map(|connection| connection.unwrap_or(default_connection))
}

pub(super) fn run_timeout_seconds_from_metadata(
    metadata: &Value,
    version_timeout_seconds: i32,
) -> Result<i32, AppError> {
    let Some(raw_timeout) = metadata
        .get("launch_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("timeout_seconds"))
    else {
        return Ok(version_timeout_seconds);
    };
    if raw_timeout.is_null() {
        return Ok(version_timeout_seconds);
    }
    let Some(timeout) = raw_timeout.as_i64() else {
        return Err(AppError::Validation(
            "launch_options.timeout_seconds must be an integer".to_string(),
        ));
    };
    if !(PLAYBOOK_RUN_TIMEOUT_MIN_SECONDS..=PLAYBOOK_RUN_TIMEOUT_MAX_SECONDS).contains(&timeout) {
        return Err(AppError::Validation(format!(
            "launch_options.timeout_seconds must be between {PLAYBOOK_RUN_TIMEOUT_MIN_SECONDS} and {PLAYBOOK_RUN_TIMEOUT_MAX_SECONDS}"
        )));
    }
    i32::try_from(timeout).map_err(|_| {
        AppError::Validation("launch_options.timeout_seconds is out of range".to_string())
    })
}

pub(super) fn launch_options_has_timeout_override(metadata: &Value) -> bool {
    metadata
        .get("launch_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("timeout_seconds"))
        .is_some_and(|value| !value.is_null())
}

pub(super) fn launch_options_positive_integer(
    metadata: &Value,
    key: &str,
) -> Result<Option<i64>, AppError> {
    let Some(raw_value) = metadata
        .get("launch_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get(key))
    else {
        return Ok(None);
    };
    if raw_value.is_null() {
        return Ok(None);
    }
    let Some(value) = raw_value.as_i64() else {
        return Err(AppError::Validation(format!(
            "launch_options.{key} must be an integer"
        )));
    };
    if value < 1 {
        return Err(AppError::Validation(format!(
            "launch_options.{key} must be at least 1"
        )));
    }
    Ok(Some(value))
}

pub(super) fn run_timeout_seconds_for_target_count(
    metadata: &Value,
    version_timeout_seconds: i32,
    target_count: usize,
) -> Result<i32, AppError> {
    let base_timeout_seconds =
        run_timeout_seconds_from_metadata(metadata, version_timeout_seconds)?;
    if target_count <= 1 {
        return Ok(base_timeout_seconds);
    }
    let Some(requested_forks) = launch_options_positive_integer(metadata, "forks")? else {
        return Ok(base_timeout_seconds);
    };

    let effective_forks = requested_forks.min(i64::try_from(target_count).map_err(|_| {
        AppError::Validation("Target set is too large for one run group".to_string())
    })?);
    let waves = (i64::try_from(target_count).map_err(|_| {
        AppError::Validation("Target set is too large for one run group".to_string())
    })? + effective_forks
        - 1)
        / effective_forks;
    let scaled_timeout = i64::from(base_timeout_seconds).saturating_mul(waves);
    Ok(scaled_timeout.min(PLAYBOOK_RUN_TIMEOUT_MAX_SECONDS) as i32)
}

pub(super) fn run_timeout_policy_metadata(
    metadata: &Value,
    version_timeout_seconds: i32,
    target_count: usize,
    timeout_seconds: i32,
) -> Result<Value, AppError> {
    let base_timeout_seconds =
        run_timeout_seconds_from_metadata(metadata, version_timeout_seconds)?;
    let target_count_i64 = i64::try_from(target_count).map_err(|_| {
        AppError::Validation("Target set is too large for one run group".to_string())
    })?;
    let requested_forks = launch_options_positive_integer(metadata, "forks")?;
    let effective_target_count = target_count_i64.max(1);
    let effective_forks = requested_forks.map(|forks| forks.min(effective_target_count));
    let explicit_override = launch_options_has_timeout_override(metadata);
    let waves = if let Some(forks) = effective_forks {
        (effective_target_count + forks - 1) / forks
    } else {
        1
    };

    Ok(json!({
        "source": "run_group",
        "base_timeout_seconds": base_timeout_seconds,
        "timeout_seconds": timeout_seconds,
        "target_count": target_count_i64,
        "requested_forks": requested_forks,
        "effective_forks": effective_forks,
        "waves": waves,
        "explicit_override": explicit_override,
    }))
}

pub(super) fn run_credential_metadata(
    record: &CreateRunGroupRecord,
    decision: &RunCredentialDecision,
) -> Value {
    let (subject_kind, subject_id) = if let Some(service_principal_id) = record.service_principal_id
    {
        ("service_principal", Some(service_principal_id))
    } else if let Some(requested_by_user_id) = record.requested_by_user_id {
        ("user", Some(requested_by_user_id))
    } else {
        ("system", None)
    };

    serde_json::json!({
        "subject_kind": subject_kind,
        "subject_id": subject_id.map(|id| id.to_string()),
        "account": {
            "kind": &decision.account_kind,
            "id": decision.account_id.map(|id| id.to_string()),
            "entry_id": decision.account_entry_id.as_deref(),
            "merge_policy": &decision.merge_policy
        },
        "resolution": {
            "status": "pending",
            "selection": decision
                .resolution
                .get("selection")
                .and_then(Value::as_str)
                .unwrap_or("auto"),
            "source": decision
                .resolution
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("launch_options"),
            "materialization": "worker_runtime"
        },
        "secret_material": "runtime_only"
    })
}
