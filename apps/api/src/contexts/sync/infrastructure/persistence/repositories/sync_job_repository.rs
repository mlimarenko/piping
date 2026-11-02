use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::repository::SyncJobRepository;
use crate::contexts::sync::domain::{SyncJob, SyncJobStep, SyncStatus, SyncStepStatus};
use crate::errors::AppError;

#[derive(Clone)]
pub struct SqlxSyncJobRepository {
    pool: PgPool,
}

impl SqlxSyncJobRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Mark an unfinished job failed without overwriting an ambiguous commit.
    pub async fn fail_if_running(&self, job: &SyncJob) -> Result<Option<SyncJob>, AppError> {
        let row = sqlx::query!(
            "UPDATE sync_job SET status = 'failed', finished_at = $2, stats = $3 WHERE id = $1 AND status IN ('queued', 'running') RETURNING id, provider_id, mapping_version, status, started_at, finished_at, stats",
            job.id, job.finished_at, &job.stats,
        ).fetch_optional(&self.pool).await?;
        row.map(|row| {
            Ok(SyncJob {
                id: row.id,
                provider_id: row.provider_id,
                mapping_version: row.mapping_version,
                status: parse_status(&row.status)?,
                started_at: row.started_at,
                finished_at: row.finished_at,
                stats: row.stats,
            })
        })
        .transpose()
    }
}

impl SyncJobRepository for SqlxSyncJobRepository {
    async fn create(&self, job: SyncJob) -> Result<SyncJob, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO sync_job (
                id, provider_id, mapping_version, status,
                started_at, finished_at, stats
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, provider_id, mapping_version, status,
                      started_at, finished_at, stats
            "#,
        )
        .bind(job.id)
        .bind(job.provider_id)
        .bind(&job.mapping_version)
        .bind(status_to_string(&job.status))
        .bind(job.started_at)
        .bind(job.finished_at)
        .bind(&job.stats)
        .fetch_one(&self.pool)
        .await?;

        map_row_to_sync_job(&row)
    }

    async fn update(&self, job: SyncJob) -> Result<SyncJob, AppError> {
        let row = sqlx::query(
            r#"
            UPDATE sync_job
            SET status = $2,
                started_at = $3,
                finished_at = $4,
                stats = $5
            WHERE id = $1
            RETURNING id, provider_id, mapping_version, status,
                      started_at, finished_at, stats
            "#,
        )
        .bind(job.id)
        .bind(status_to_string(&job.status))
        .bind(job.started_at)
        .bind(job.finished_at)
        .bind(&job.stats)
        .fetch_one(&self.pool)
        .await?;

        map_row_to_sync_job(&row)
    }

    async fn get_by_id(&self, _job_id: Uuid) -> Result<Option<SyncJob>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, provider_id, mapping_version, status,
                   started_at, finished_at, stats
            FROM sync_job
            WHERE id = $1
            "#,
        )
        .bind(_job_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => map_row_to_sync_job(&row).map(Some),
            None => Ok(None),
        }
    }

    async fn latest_for_provider(&self, provider_id: Uuid) -> Result<Option<SyncJob>, AppError> {
        // `id` is UUIDv7, so ordering by it is the durable "most recent"
        // ordering even before the job has a started_at.
        let row = sqlx::query(
            r#"
            SELECT id, provider_id, mapping_version, status,
                   started_at, finished_at, stats
            FROM sync_job
            WHERE provider_id = $1
            ORDER BY id DESC
            LIMIT 1
            "#,
        )
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => map_row_to_sync_job(&row).map(Some),
            None => Ok(None),
        }
    }

    async fn upsert_step(&self, step: SyncJobStep) -> Result<SyncJobStep, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO sync_job_step (
                id,
                job_id,
                step_key,
                status,
                progress_current,
                progress_total,
                started_at,
                finished_at,
                metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (job_id, step_key)
            DO UPDATE SET
                status = EXCLUDED.status,
                progress_current = EXCLUDED.progress_current,
                progress_total = EXCLUDED.progress_total,
                started_at = COALESCE(sync_job_step.started_at, EXCLUDED.started_at),
                finished_at = EXCLUDED.finished_at,
                metadata = EXCLUDED.metadata
            RETURNING id, job_id, step_key, status, progress_current, progress_total, started_at,
                      finished_at, metadata
            "#,
        )
        .bind(step.id)
        .bind(step.job_id)
        .bind(&step.step_key)
        .bind(step_status_to_string(&step.status))
        .bind(step.progress_current)
        .bind(step.progress_total)
        .bind(step.started_at)
        .bind(step.finished_at)
        .bind(&step.metadata)
        .fetch_one(&self.pool)
        .await?;

        map_row_to_sync_job_step(&row)
    }
}

fn map_row_to_sync_job(row: &sqlx::postgres::PgRow) -> Result<SyncJob, AppError> {
    let status: String = row.try_get("status")?;
    let status = parse_status(&status)?;

    Ok(SyncJob {
        id: row.try_get("id")?,
        provider_id: row.try_get("provider_id")?,
        mapping_version: row.try_get("mapping_version")?,
        status,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        stats: row.try_get("stats")?,
    })
}

fn map_row_to_sync_job_step(row: &sqlx::postgres::PgRow) -> Result<SyncJobStep, AppError> {
    let status: String = row.try_get("status")?;
    let status = parse_step_status(&status)?;

    Ok(SyncJobStep {
        id: row.try_get("id")?,
        job_id: row.try_get("job_id")?,
        step_key: row.try_get("step_key")?,
        status,
        progress_current: row.try_get("progress_current")?,
        progress_total: row.try_get("progress_total")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        metadata: row.try_get("metadata")?,
    })
}

fn status_to_string(status: &SyncStatus) -> String {
    match status {
        SyncStatus::Queued => "queued",
        SyncStatus::Running => "running",
        SyncStatus::Completed => "completed",
        SyncStatus::Failed => "failed",
    }
    .to_string()
}

fn step_status_to_string(status: &SyncStepStatus) -> String {
    match status {
        SyncStepStatus::Queued => "queued",
        SyncStepStatus::Running => "running",
        SyncStepStatus::Completed => "completed",
        SyncStepStatus::Failed => "failed",
    }
    .to_string()
}

fn parse_status(status: &str) -> Result<SyncStatus, AppError> {
    match status {
        "queued" => Ok(SyncStatus::Queued),
        "running" => Ok(SyncStatus::Running),
        "completed" => Ok(SyncStatus::Completed),
        "failed" => Ok(SyncStatus::Failed),
        _ => Err(AppError::Validation(format!(
            "Unknown sync status '{}'",
            status
        ))),
    }
}

fn parse_step_status(status: &str) -> Result<SyncStepStatus, AppError> {
    match status {
        "queued" => Ok(SyncStepStatus::Queued),
        "running" => Ok(SyncStepStatus::Running),
        "completed" => Ok(SyncStepStatus::Completed),
        "failed" => Ok(SyncStepStatus::Failed),
        _ => Err(AppError::Validation(format!(
            "Unknown sync step status '{}'",
            status
        ))),
    }
}
