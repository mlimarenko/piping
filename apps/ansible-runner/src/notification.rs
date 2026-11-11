//! Bounded, untrusted structured Ansible notification projection.
//!
//! The project can write the job spool. Nothing here trusts its identity,
//! ordering or size: hosts are bound to the supervisor dispatch targets, run
//! identities and the batch id are supervisor-owned, known secrets are redacted
//! before durable delivery, and every bound is re-applied after redaction.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use piping_contracts::playbook_runner::{RunnerEventKind, RunnerGroupEvent, RunnerGroupEventBatch};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) const MAX_NOTIFICATION_BYTES: usize = 256 * 1024;
const MAX_EVENTS: usize = 1000;
const MAX_HOSTS: usize = 512;
const MAX_TASKS: usize = 1000;
const MAX_LINE_BYTES: usize = 4096;
const MAX_HOST_CHARS: usize = 120;
const MAX_TASK_CHARS: usize = 200;
const MAX_COUNT: u64 = 1_000_000_000;
const NOTIFY_DIR: &str = "notify";
const NOTIFY_FILE: &str = "result.ndjson";
const REDACTED: &str = "[REDACTED]";
// Linux UAPI `O_NOFOLLOW`; the runner is a Linux OCI process.
const O_NOFOLLOW: i32 = 0o400000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskResult {
    Ok,
    Changed,
    Failed,
    Ignored,
    Skipped,
    Unreachable,
}

impl TaskResult {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "ok" => Some(Self::Ok),
            "changed" => Some(Self::Changed),
            "failed" => Some(Self::Failed),
            "ignored" => Some(Self::Ignored),
            "skipped" => Some(Self::Skipped),
            "unreachable" => Some(Self::Unreachable),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Changed => "changed",
            Self::Failed => "failed",
            Self::Ignored => "ignored",
            Self::Skipped => "skipped",
            Self::Unreachable => "unreachable",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HostSummary {
    pub ok: u64,
    pub changed: u64,
    pub unreachable: u64,
    pub failed: u64,
    pub skipped: u64,
    pub rescued: u64,
    pub ignored: u64,
}

impl HostSummary {
    fn add(&mut self, other: &Self) {
        self.ok = self.ok.saturating_add(other.ok).min(MAX_COUNT);
        self.changed = self.changed.saturating_add(other.changed).min(MAX_COUNT);
        self.unreachable = self
            .unreachable
            .saturating_add(other.unreachable)
            .min(MAX_COUNT);
        self.failed = self.failed.saturating_add(other.failed).min(MAX_COUNT);
        self.skipped = self.skipped.saturating_add(other.skipped).min(MAX_COUNT);
        self.rescued = self.rescued.saturating_add(other.rescued).min(MAX_COUNT);
        self.ignored = self.ignored.saturating_add(other.ignored).min(MAX_COUNT);
    }

    pub(crate) fn succeeded(&self) -> bool {
        self.failed == 0 && self.unreachable == 0
    }
}

#[derive(Debug)]
struct TaskEvent {
    run_id: Option<Uuid>,
    host: Option<String>,
    task: String,
    result: TaskResult,
}

#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub hosts: BTreeMap<String, HostSummary>,
    tasks: Vec<TaskEvent>,
    truncated: bool,
    invalid: u64,
}

#[derive(Debug)]
pub(crate) enum SpoolError {
    /// No sealed notification was written.
    Absent,
    /// A path component is a symlink or has an unexpected type.
    Unsafe,
    /// The sealed notification exceeds its transport bound.
    TooLarge,
}

impl SpoolError {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Unsafe => "unsafe",
            Self::TooLarge => "oversized",
        }
    }
}

/// Read `<workspace>/notify/result.ndjson` without following any path
/// component. The caller must first terminate the sandbox and require that
/// termination to succeed: the absent live writer is what excludes a directory
/// race. The component lstat/open checks then defend the static tree, and
/// `O_NOFOLLOW` alone is not claimed to prevent a concurrent swap.
pub(crate) fn read_spool(workspace: &Path) -> Result<Option<Vec<u8>>, SpoolError> {
    let root = std::fs::symlink_metadata(workspace).map_err(|_| SpoolError::Absent)?;
    if root.file_type().is_symlink() || !root.is_dir() {
        return Err(SpoolError::Unsafe);
    }
    let directory = workspace.join(NOTIFY_DIR);
    let directory_meta = match std::fs::symlink_metadata(&directory) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SpoolError::Absent);
        }
        Err(_) => return Err(SpoolError::Unsafe),
    };
    if directory_meta.file_type().is_symlink() || !directory_meta.is_dir() {
        return Err(SpoolError::Unsafe);
    }
    let path = directory.join(NOTIFY_FILE);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SpoolError::Absent);
        }
        Err(_) => return Err(SpoolError::Unsafe),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(SpoolError::Unsafe);
    }
    if meta.len() > MAX_NOTIFICATION_BYTES as u64 {
        return Err(SpoolError::TooLarge);
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(&path)
        .map_err(|_| SpoolError::Unsafe)?;
    let opened = file.metadata().map_err(|_| SpoolError::Unsafe)?;
    if !opened.is_file() || opened.dev() != meta.dev() || opened.ino() != meta.ino() {
        return Err(SpoolError::Unsafe);
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    file.by_ref()
        .take(MAX_NOTIFICATION_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SpoolError::Unsafe)?;
    if bytes.len() > MAX_NOTIFICATION_BYTES {
        return Err(SpoolError::TooLarge);
    }
    Ok(Some(bytes))
}

fn exact_keys(object: &serde_json::Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

/// Parse the sealed spool. Malformed lines are counted, never interpreted.
pub(crate) fn parse(bytes: &[u8], targets: &BTreeMap<String, Uuid>, secrets: &[Vec<u8>]) -> Parsed {
    let secrets: Vec<String> = secrets
        .iter()
        .filter_map(|secret| String::from_utf8(secret.clone()).ok())
        .filter(|secret| !secret.is_empty())
        .collect();
    let mut secrets = secrets;
    secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
    let mut parsed = Parsed::default();
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_LINE_BYTES {
            parsed.invalid = parsed.invalid.saturating_add(1);
            parsed.truncated = true;
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            parsed.invalid = parsed.invalid.saturating_add(1);
            continue;
        };
        let Some(object) = value.as_object() else {
            parsed.invalid = parsed.invalid.saturating_add(1);
            continue;
        };
        let Some(kind) = object
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            parsed.invalid = parsed.invalid.saturating_add(1);
            continue;
        };
        match kind.as_str() {
            "truncated" => {
                if !exact_keys(object, &["kind"]) {
                    parsed.invalid = parsed.invalid.saturating_add(1);
                }
                parsed.truncated = true;
            }
            "host" => {
                if !exact_keys(
                    object,
                    &[
                        "kind",
                        "host",
                        "ok",
                        "changed",
                        "unreachable",
                        "failed",
                        "skipped",
                        "rescued",
                        "ignored",
                    ],
                ) {
                    parsed.invalid = parsed.invalid.saturating_add(1);
                    continue;
                }
                let Ok(record) = serde_json::from_value::<HostRecord>(value.clone()) else {
                    parsed.invalid = parsed.invalid.saturating_add(1);
                    continue;
                };
                // A native launch has no managed host identity to bind; its
                // host labels are not stored as if they were dispatch targets.
                if targets.is_empty() {
                    continue;
                }
                if !valid_host(&record.host) || !targets.contains_key(&record.host) {
                    parsed.invalid = parsed.invalid.saturating_add(1);
                    continue;
                }
                if parsed.hosts.len() >= MAX_HOSTS && !parsed.hosts.contains_key(&record.host) {
                    parsed.truncated = true;
                    continue;
                }
                // Several playbooks legitimately report stats for one host.
                // Counts only accumulate, so a later record cannot turn an
                // observed failure into success.
                let summary = record.summary();
                parsed.hosts.entry(record.host).or_default().add(&summary);
            }
            "task" => {
                if !exact_keys(object, &["kind", "host", "task", "result"]) {
                    parsed.invalid = parsed.invalid.saturating_add(1);
                    continue;
                }
                let Ok(record) = serde_json::from_value::<TaskRecord>(value.clone()) else {
                    parsed.invalid = parsed.invalid.saturating_add(1);
                    continue;
                };
                let Some(result) = TaskResult::parse(&record.result) else {
                    parsed.invalid = parsed.invalid.saturating_add(1);
                    continue;
                };
                let task = sanitize(&record.task, &secrets, MAX_TASK_CHARS);
                if task.is_empty() {
                    parsed.invalid = parsed.invalid.saturating_add(1);
                    continue;
                }
                let (run_id, host) = if targets.is_empty() {
                    (None, None)
                } else if valid_host(&record.host) {
                    match targets.get(&record.host) {
                        Some(run_id) => (Some(*run_id), Some(record.host.clone())),
                        None => {
                            parsed.invalid = parsed.invalid.saturating_add(1);
                            continue;
                        }
                    }
                } else {
                    parsed.invalid = parsed.invalid.saturating_add(1);
                    continue;
                };
                if parsed.tasks.len() >= MAX_TASKS {
                    parsed.truncated = true;
                    continue;
                }
                parsed.tasks.push(TaskEvent {
                    run_id,
                    host,
                    task,
                    result,
                });
            }
            _ => parsed.invalid = parsed.invalid.saturating_add(1),
        }
    }
    parsed
}

fn valid_host(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        && value.chars().count() <= MAX_HOST_CHARS
}

fn sanitize(value: &str, secrets: &[String], max_chars: usize) -> String {
    let mut output: String = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    for secret in secrets {
        if output.contains(secret.as_str()) {
            output = output.replace(secret.as_str(), REDACTED);
        }
    }
    let trimmed = output.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        trimmed.chars().take(max_chars).collect()
    }
}

#[derive(Deserialize)]
struct HostRecord {
    host: String,
    ok: u64,
    changed: u64,
    unreachable: u64,
    failed: u64,
    skipped: u64,
    rescued: u64,
    ignored: u64,
}

impl HostRecord {
    fn summary(&self) -> HostSummary {
        HostSummary {
            ok: self.ok.min(MAX_COUNT),
            changed: self.changed.min(MAX_COUNT),
            unreachable: self.unreachable.min(MAX_COUNT),
            failed: self.failed.min(MAX_COUNT),
            skipped: self.skipped.min(MAX_COUNT),
            rescued: self.rescued.min(MAX_COUNT),
            ignored: self.ignored.min(MAX_COUNT),
        }
    }
}

#[derive(Deserialize)]
struct TaskRecord {
    host: String,
    task: String,
    result: String,
}

pub(crate) struct Projection {
    pub host_verdicts: BTreeMap<String, bool>,
    pub batch: Option<RunnerGroupEventBatch>,
}

/// Project untrusted records into the bounded event batch and the per-host
/// verdicts used by completion. `unavailable` is a supervisor-side reason code
/// used only when no valid host summary exists.
pub(crate) fn project(
    parsed: &Parsed,
    targets: &BTreeMap<String, Uuid>,
    batch_id: Uuid,
    unavailable: Option<&'static str>,
) -> Projection {
    let host_verdicts: BTreeMap<String, bool> = parsed
        .hosts
        .iter()
        .map(|(host, summary)| (host.clone(), summary.succeeded()))
        .collect();

    let mut events = Vec::new();
    for (host, summary) in &parsed.hosts {
        let Some(run_id) = targets.get(host) else {
            continue;
        };
        events.push(RunnerGroupEvent {
            run_id: Some(*run_id),
            host: Some(host.clone()),
            event_kind: RunnerEventKind::Result,
            event_type: "host_summary".into(),
            message: None,
            payload: Some(json!({
                "ok": summary.ok,
                "changed": summary.changed,
                "unreachable": summary.unreachable,
                "failed": summary.failed,
                "skipped": summary.skipped,
                "rescued": summary.rescued,
                "ignored": summary.ignored,
            })),
        });
    }
    for task in &parsed.tasks {
        events.push(RunnerGroupEvent {
            run_id: task.run_id,
            host: task.host.clone(),
            event_kind: RunnerEventKind::Result,
            event_type: "task_result".into(),
            message: Some(task.task.clone()),
            payload: Some(json!({"task": task.task.clone(), "result": task.result.as_str()})),
        });
    }

    let missing = targets
        .keys()
        .filter(|host| !parsed.hosts.contains_key(*host))
        .count();
    let mut diagnostics = Vec::new();
    if parsed.truncated || parsed.invalid > 0 {
        diagnostics.push(RunnerGroupEvent {
            run_id: None,
            host: None,
            event_kind: RunnerEventKind::System,
            event_type: "notification_truncated".into(),
            message: Some("Structured notification was incomplete".into()),
            payload: Some(json!({
                "invalid_lines": parsed.invalid,
                "truncated": parsed.truncated,
                "hosts": parsed.hosts.len(),
                "tasks": parsed.tasks.len(),
            })),
        });
    }
    if !targets.is_empty() && (parsed.hosts.is_empty() || missing > 0) {
        diagnostics.push(RunnerGroupEvent {
            run_id: None,
            host: None,
            event_kind: RunnerEventKind::System,
            event_type: "notification_unavailable".into(),
            message: Some("No valid structured host result was reported".into()),
            payload: Some(json!({
                "reason": unavailable.unwrap_or("partial"),
                "unreported_hosts": missing,
                "reported_hosts": parsed.hosts.len(),
            })),
        });
    }

    let mut events = events;
    while events.len() + diagnostics.len() > MAX_EVENTS {
        match events.last() {
            Some(event) if event.event_type == "task_result" => {
                events.pop();
            }
            _ => break,
        }
    }
    events.extend(diagnostics);

    let mut batch = RunnerGroupEventBatch { batch_id, events };
    loop {
        let encoded = match serde_json::to_vec(&batch) {
            Ok(encoded) => encoded,
            Err(_) => {
                return Projection {
                    host_verdicts,
                    batch: None,
                };
            }
        };
        if encoded.len() <= MAX_NOTIFICATION_BYTES {
            break;
        }
        if let Some(index) = batch
            .events
            .iter()
            .rposition(|event| event.event_type == "task_result")
        {
            batch.events.remove(index);
            continue;
        }
        if batch
            .events
            .last()
            .is_some_and(|event| event.event_type == "notification_unavailable")
        {
            batch.events.pop();
            continue;
        }
        break;
    }
    if batch.events.is_empty() {
        return Projection {
            host_verdicts,
            batch: None,
        };
    }
    Projection {
        host_verdicts,
        batch: Some(batch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(hosts: &[&str]) -> BTreeMap<String, Uuid> {
        hosts
            .iter()
            .map(|host| ((*host).to_string(), Uuid::new_v4()))
            .collect()
    }

    fn host_line(host: &str, failed: u64) -> String {
        format!(
            "{{\"kind\":\"host\",\"host\":\"{host}\",\"ok\":1,\"changed\":0,\"unreachable\":0,\"failed\":{failed},\"skipped\":0,\"rescued\":0,\"ignored\":0}}"
        )
    }

    #[test]
    fn binds_hosts_and_derives_verdicts_from_counts() {
        let targets = targets(&["web"]);
        let parsed = parse(
            format!("{}\n{}", host_line("web", 0), host_line("db", 0)).as_bytes(),
            &targets,
            &[],
        );
        assert_eq!(parsed.hosts.len(), 1);
        assert!(parsed.hosts["web"].succeeded());
        assert_eq!(parsed.invalid, 1, "out-of-dispatch host is refused");
        let projection = project(&parsed, &targets, Uuid::nil(), None);
        assert_eq!(projection.host_verdicts.get("web"), Some(&true));
    }

    #[test]
    fn duplicate_stats_accumulate_failures_and_cannot_flip_to_success() {
        let targets = targets(&["web"]);
        let parsed = parse(
            format!("{}\n{}", host_line("web", 1), host_line("web", 0)).as_bytes(),
            &targets,
            &[],
        );
        assert_eq!(parsed.hosts["web"].failed, 1);
        assert!(!parsed.hosts["web"].succeeded());
    }

    #[test]
    fn malformed_and_oversized_lines_are_counted_not_interpreted() {
        let targets = targets(&["web"]);
        let oversized = format!(
            "{{\"kind\":\"task\",\"host\":\"web\",\"task\":\"{}\",\"result\":\"ok\"}}",
            "x".repeat(MAX_LINE_BYTES)
        );
        let parsed = parse(
            format!(
                "not json\n{}\n{}\n{{\"kind\":\"host\",\"host\":\"web\",\"ok\":1,\"changed\":0,\"unreachable\":0,\"failed\":0,\"skipped\":0,\"rescued\":0,\"ignored\":0,\"extra\":1}}\n",
                oversized,
                host_line("web", 0)
            )
            .as_bytes(),
            &targets,
            &[],
        );
        assert_eq!(parsed.hosts.len(), 1);
        assert_eq!(parsed.invalid, 3);
        assert!(parsed.truncated);
    }

    #[test]
    fn known_secrets_are_redacted_before_events() {
        let targets = targets(&["web"]);
        let secret = vec![b"s3cr3t".to_vec()];
        let line =
            "{\"kind\":\"task\",\"host\":\"web\",\"task\":\"deploy s3cr3t\",\"result\":\"ok\"}";
        let parsed = parse(line.as_bytes(), &targets, &secret);
        assert_eq!(parsed.tasks.len(), 1);
        let projection = project(&parsed, &targets, Uuid::nil(), None);
        let batch = projection.batch.expect("batch");
        let task = batch
            .events
            .iter()
            .find(|event| event.event_type == "task_result")
            .expect("task event");
        assert_eq!(task.message.as_deref(), Some("deploy [REDACTED]"));
        assert!(!serde_json::to_string(&batch).unwrap().contains("s3cr3t"));
    }

    #[test]
    fn absent_host_results_are_visible_and_unreported() {
        let targets = targets(&["web"]);
        let parsed = parse(b"", &targets, &[]);
        let projection = project(&parsed, &targets, Uuid::nil(), Some("absent"));
        assert!(projection.host_verdicts.is_empty());
        let batch = projection.batch.expect("diagnostic batch");
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].event_type, "notification_unavailable");
        assert_eq!(
            batch.events[0].payload.as_ref().unwrap()["reason"],
            "absent"
        );
    }

    #[test]
    fn batch_stays_within_the_event_and_byte_bounds() {
        let targets = targets(&["web"]);
        let mut lines = Vec::new();
        for index in 0..(MAX_TASKS + 10) {
            lines.push(format!(
                "{{\"kind\":\"task\",\"host\":\"web\",\"task\":\"task {index}\",\"result\":\"ok\"}}"
            ));
        }
        let parsed = parse(lines.join("\n").as_bytes(), &targets, &[]);
        let projection = project(&parsed, &targets, Uuid::nil(), None);
        let batch = projection.batch.expect("batch");
        assert!(batch.events.len() <= MAX_EVENTS);
        assert!(serde_json::to_vec(&batch).unwrap().len() <= MAX_NOTIFICATION_BYTES);
        assert!(
            batch
                .events
                .iter()
                .any(|event| event.event_type == "notification_truncated")
        );
    }

    #[test]
    fn docker_spool_refuses_a_symlinked_directory_or_file() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("job");
        std::fs::create_dir(&workspace).unwrap();
        assert!(matches!(read_spool(&workspace), Err(SpoolError::Absent)));
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join(NOTIFY_FILE), host_line("web", 0)).unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join(NOTIFY_DIR)).unwrap();
        assert!(matches!(read_spool(&workspace), Err(SpoolError::Unsafe)));

        std::fs::remove_file(workspace.join(NOTIFY_DIR)).unwrap();
        std::fs::create_dir(workspace.join(NOTIFY_DIR)).unwrap();
        std::os::unix::fs::symlink(
            outside.join(NOTIFY_FILE),
            workspace.join(NOTIFY_DIR).join(NOTIFY_FILE),
        )
        .unwrap();
        assert!(matches!(read_spool(&workspace), Err(SpoolError::Unsafe)));
    }

    #[test]
    fn docker_spool_reads_a_regular_file_and_refuses_oversize() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("job");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(workspace.join(NOTIFY_DIR)).unwrap();
        std::fs::write(workspace.join(NOTIFY_DIR).join(NOTIFY_FILE), b"{}\n").unwrap();
        assert_eq!(
            read_spool(&workspace).unwrap().as_deref(),
            Some(b"{}\n".as_slice())
        );
        let oversized = vec![b' '; MAX_NOTIFICATION_BYTES + 1];
        std::fs::write(workspace.join(NOTIFY_DIR).join(NOTIFY_FILE), oversized).unwrap();
        assert!(matches!(read_spool(&workspace), Err(SpoolError::TooLarge)));
    }
}
