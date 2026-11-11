use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Sync job status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

/// Sync job step status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncStepStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

/// Sync job domain model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncJob {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub mapping_version: String,
    pub status: SyncStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub stats: Value,
}

/// Sync job step domain model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncJobStep {
    pub id: Uuid,
    pub job_id: Uuid,
    pub step_key: String,
    pub status: SyncStepStatus,
    pub progress_current: i32,
    pub progress_total: Option<i32>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub metadata: Value,
}

impl SyncJob {
    pub fn start(&mut self) {
        self.status = SyncStatus::Running;
        self.started_at = Some(Utc::now());
    }

    pub fn complete(&mut self) {
        self.status = SyncStatus::Completed;
        self.finished_at = Some(Utc::now());
    }

    pub fn fail(&mut self) {
        self.status = SyncStatus::Failed;
        self.finished_at = Some(Utc::now());
    }
}

impl SyncJobStep {
    pub fn new(job_id: Uuid, step_key: String, metadata: Value) -> Self {
        Self {
            id: Uuid::now_v7(),
            job_id,
            step_key,
            status: SyncStepStatus::Queued,
            progress_current: 0,
            progress_total: None,
            started_at: None,
            finished_at: None,
            metadata,
        }
    }

    pub fn start(&mut self) {
        self.status = SyncStepStatus::Running;
        self.started_at = Some(Utc::now());
    }

    pub fn progress(&mut self, current: i32, total: Option<i32>) {
        self.status = SyncStepStatus::Running;
        self.progress_current = current;
        self.progress_total = total;
    }

    pub fn complete(&mut self) {
        self.status = SyncStepStatus::Completed;
        self.finished_at = Some(Utc::now());
    }

    pub fn fail(&mut self) {
        self.status = SyncStepStatus::Failed;
        self.finished_at = Some(Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn queued_job() -> SyncJob {
        SyncJob {
            id: Uuid::now_v7(),
            provider_id: Uuid::now_v7(),
            mapping_version: "v1".to_string(),
            status: SyncStatus::Queued,
            started_at: None,
            finished_at: None,
            stats: json!({}),
        }
    }

    #[test]
    fn sync_job_lifecycle_transitions_set_timestamps() {
        let mut job = queued_job();
        assert_eq!(job.status, SyncStatus::Queued);

        job.start();
        assert_eq!(job.status, SyncStatus::Running);
        assert!(job.started_at.is_some());

        job.complete();
        assert_eq!(job.status, SyncStatus::Completed);
        assert!(job.finished_at.is_some());
    }

    #[test]
    fn sync_job_fail_marks_failed_with_finish_time() {
        let mut job = queued_job();
        job.start();
        job.fail();
        assert_eq!(job.status, SyncStatus::Failed);
        assert!(job.finished_at.is_some());
    }

    #[test]
    fn sync_job_serializes_round_trip() {
        let job = queued_job();
        let serialized = serde_json::to_string(&job).expect("serialize");
        let restored: SyncJob = serde_json::from_str(&serialized).expect("deserialize");
        assert_eq!(restored.id, job.id);
        assert_eq!(restored.provider_id, job.provider_id);
        assert_eq!(restored.status, SyncStatus::Queued);
    }
}
