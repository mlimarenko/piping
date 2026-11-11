use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PullSchedule {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub mapping_logical_id: Uuid,
    pub mapping_version_id: i32,
    pub principal_method: String,
    pub principal_user_id: Uuid,
    pub principal_token_id: Option<Uuid>,
    pub cron_expression: String,
    pub timezone: String,
    pub enabled: bool,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DuePullSchedule {
    #[sqlx(flatten)]
    pub schedule: PullSchedule,
    pub database_now: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PullOccurrence {
    pub schedule_id: Uuid,
    pub scheduled_for: DateTime<Utc>,
    pub status: String,
    pub lease_id: Option<Uuid>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub job_id: Option<Uuid>,
    pub result_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy)]
pub enum OccurrenceResult {
    Completed,
    Failed,
    Skipped,
}

impl OccurrenceResult {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Clone)]
pub struct SqlxPullScheduleRepository {
    pool: PgPool,
}

impl SqlxPullScheduleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn database_now(&self) -> Result<DateTime<Utc>, AppError> {
        Ok(sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn create(&self, schedule: &PullSchedule) -> Result<PullSchedule, AppError> {
        let mut tx = self.pool.begin().await?;
        let eligible: Option<Uuid> = sqlx::query_scalar(
            r#"SELECT p.id FROM sync_provider p JOIN mapping_configurations m
                ON m.logical_id = $2 AND m.version_id = $3
               WHERE p.id = $1 AND p.is_enabled AND p.provider_type = 'netbox'
                 AND p.config->>'sync_mode' = 'pull_only' AND m.source_provider_id = p.id
                 AND m.status = 'active' AND m.target_provider_id IS NULL
                 AND m.direction = 'pull' AND m.conflict_mode = 'netbox_wins'
               FOR SHARE OF p, m"#,
        )
        .bind(schedule.provider_id)
        .bind(schedule.mapping_logical_id)
        .bind(schedule.mapping_version_id)
        .fetch_optional(&mut *tx)
        .await?;
        if eligible.is_none() {
            return Err(AppError::Validation(
                "An enabled pull_only source and active local pull mapping are required".into(),
            ));
        }
        let created = sqlx::query_as::<_, PullSchedule>(
            r#"INSERT INTO sync_pull_schedule
                (id, provider_id, mapping_logical_id, mapping_version_id, principal_method,
                 principal_user_id, principal_token_id, cron_expression, timezone, enabled, next_run_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING *"#,
        )
        .bind(schedule.id)
        .bind(schedule.provider_id)
        .bind(schedule.mapping_logical_id)
        .bind(schedule.mapping_version_id)
        .bind(&schedule.principal_method)
        .bind(schedule.principal_user_id)
        .bind(schedule.principal_token_id)
        .bind(&schedule.cron_expression)
        .bind(&schedule.timezone)
        .bind(schedule.enabled)
        .bind(schedule.next_run_at)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(created)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<PullSchedule>, AppError> {
        Ok(
            sqlx::query_as("SELECT * FROM sync_pull_schedule WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<PullSchedule>, AppError> {
        Ok(sqlx::query_as(
            "SELECT * FROM sync_pull_schedule ORDER BY created_at DESC, id DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit.clamp(1, 500))
        .bind(offset.max(0))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn list_due(&self, limit: i64) -> Result<Vec<DuePullSchedule>, AppError> {
        Ok(sqlx::query_as(
            "SELECT schedule.*, clock_timestamp() AS database_now FROM sync_pull_schedule schedule \
             WHERE enabled AND next_run_at <= clock_timestamp() ORDER BY next_run_at, id LIMIT $1",
        )
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?)
    }

    /// Advance exactly the observed cursor and insert its unique occurrence in one transaction.
    pub async fn claim_due(
        &self,
        schedule_id: Uuid,
        scheduled_for: DateTime<Utc>,
        next_run_at: Option<DateTime<Utc>>,
        lease_id: Uuid,
        ttl_seconds: i32,
    ) -> Result<Option<PullOccurrence>, AppError> {
        validate_ttl(ttl_seconds)?;
        if next_run_at.is_some_and(|next| next <= scheduled_for) {
            return Err(AppError::Validation(
                "next schedule occurrence must advance".into(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            r#"UPDATE sync_pull_schedule SET last_run_at = $2, next_run_at = $3,
                 enabled = enabled AND $3::timestamptz IS NOT NULL, updated_at = clock_timestamp()
               WHERE id = $1 AND enabled AND next_run_at = $2 AND next_run_at <= clock_timestamp()"#,
        )
        .bind(schedule_id)
        .bind(scheduled_for)
        .bind(next_run_at)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        let occurrence = sqlx::query_as(
            r#"INSERT INTO sync_pull_schedule_occurrence
                (schedule_id, scheduled_for, status, lease_id, lease_expires_at)
               VALUES ($1, $2, 'claimed', $3, clock_timestamp() + make_interval(secs => $4))
               RETURNING *"#,
        )
        .bind(schedule_id)
        .bind(scheduled_for)
        .bind(lease_id)
        .bind(f64::from(ttl_seconds))
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(occurrence))
    }

    /// Take expired unfinished occurrences. A `running` job must not be changed
    /// until the caller has independently acquired the shared provider lease.
    pub async fn claim_expired(
        &self,
        lease_id: Uuid,
        ttl_seconds: i32,
        limit: i64,
    ) -> Result<Vec<PullOccurrence>, AppError> {
        validate_ttl(ttl_seconds)?;
        Ok(sqlx::query_as(
            r#"WITH candidates AS (
                SELECT schedule_id, scheduled_for FROM sync_pull_schedule_occurrence
                WHERE status IN ('claimed', 'running')
                  AND (lease_expires_at IS NULL OR lease_expires_at <= clock_timestamp())
                ORDER BY scheduled_for, schedule_id LIMIT $3 FOR UPDATE SKIP LOCKED
               )
               UPDATE sync_pull_schedule_occurrence o SET lease_id = $1,
                   lease_expires_at = clock_timestamp() + make_interval(secs => $2),
                   updated_at = clock_timestamp()
               FROM candidates c WHERE o.schedule_id = c.schedule_id AND o.scheduled_for = c.scheduled_for
               RETURNING o.*"#,
        )
        .bind(lease_id)
        .bind(f64::from(ttl_seconds))
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?)
    }

    /// Renewal also checks cancellation; an expired or replaced lease never revives.
    pub async fn renew(
        &self,
        schedule: Uuid,
        when: DateTime<Utc>,
        lease: Uuid,
        ttl: i32,
    ) -> Result<bool, AppError> {
        self.renew_inner(schedule, when, lease, ttl, false).await
    }

    /// Recovery may renew a cancelled occurrence but cannot start its job.
    pub async fn renew_recovery(
        &self,
        schedule: Uuid,
        when: DateTime<Utc>,
        lease: Uuid,
        ttl: i32,
    ) -> Result<bool, AppError> {
        self.renew_inner(schedule, when, lease, ttl, true).await
    }

    /// Keep an occurrence alive while validation decides whether it can start.
    /// Commit authorization separately rechecks provider and mapping eligibility.
    pub async fn renew_heartbeat(
        &self,
        schedule: Uuid,
        when: DateTime<Utc>,
        lease: Uuid,
        ttl: i32,
    ) -> Result<bool, AppError> {
        validate_ttl(ttl)?;
        Ok(sqlx::query(
            r#"UPDATE sync_pull_schedule_occurrence o SET
                 lease_expires_at = clock_timestamp() + make_interval(secs => $4), updated_at = clock_timestamp()
               FROM sync_pull_schedule s WHERE o.schedule_id = $1 AND o.scheduled_for = $2
                 AND o.lease_id = $3 AND o.lease_expires_at > clock_timestamp()
                 AND o.status IN ('claimed', 'running') AND s.id = o.schedule_id AND s.cancelled_at IS NULL"#,
        ).bind(schedule).bind(when).bind(lease).bind(f64::from(ttl))
         .execute(&self.pool).await?.rows_affected() == 1)
    }

    async fn renew_inner(
        &self,
        schedule: Uuid,
        when: DateTime<Utc>,
        lease: Uuid,
        ttl: i32,
        recovery: bool,
    ) -> Result<bool, AppError> {
        validate_ttl(ttl)?;
        Ok(sqlx::query(
            r#"UPDATE sync_pull_schedule_occurrence o SET
                 lease_expires_at = clock_timestamp() + make_interval(secs => $4), updated_at = clock_timestamp()
               FROM sync_pull_schedule s WHERE o.schedule_id = $1 AND o.scheduled_for = $2
                 AND o.lease_id = $3 AND o.lease_expires_at > clock_timestamp()
                 AND o.status IN ('claimed', 'running') AND s.id = o.schedule_id
                 AND ($5 OR (s.cancelled_at IS NULL AND EXISTS (
                    SELECT 1 FROM sync_provider p JOIN mapping_configurations m
                      ON m.logical_id = s.mapping_logical_id AND m.version_id = s.mapping_version_id
                    WHERE p.id = s.provider_id AND p.is_enabled AND p.provider_type = 'netbox'
                      AND p.config->>'sync_mode' = 'pull_only' AND m.source_provider_id = p.id
                      AND m.status = 'active' AND m.target_provider_id IS NULL
                      AND m.direction = 'pull' AND m.conflict_mode = 'netbox_wins')))"#,
        )
        .bind(schedule).bind(when).bind(lease).bind(f64::from(ttl)).bind(recovery)
        .execute(&self.pool).await?.rows_affected() == 1)
    }

    /// Call only after provider admission. Persist the intended job before any pull can start.
    pub async fn record_admitted_job(
        &self,
        schedule: Uuid,
        when: DateTime<Utc>,
        lease: Uuid,
        job: Uuid,
    ) -> Result<bool, AppError> {
        Ok(sqlx::query(
            r#"UPDATE sync_pull_schedule_occurrence o SET status = 'running', job_id = $4, updated_at = clock_timestamp()
               FROM sync_pull_schedule s WHERE o.schedule_id = $1 AND o.scheduled_for = $2
                 AND o.lease_id = $3 AND o.lease_expires_at > clock_timestamp() AND o.status = 'claimed'
                 AND s.id = o.schedule_id AND s.cancelled_at IS NULL AND EXISTS (
                    SELECT 1 FROM sync_provider p JOIN mapping_configurations m
                      ON m.logical_id = s.mapping_logical_id AND m.version_id = s.mapping_version_id
                    WHERE p.id = s.provider_id AND p.is_enabled AND p.provider_type = 'netbox'
                      AND p.config->>'sync_mode' = 'pull_only' AND m.source_provider_id = p.id
                      AND m.status = 'active' AND m.target_provider_id IS NULL
                      AND m.direction = 'pull' AND m.conflict_mode = 'netbox_wins')"#,
        ).bind(schedule).bind(when).bind(lease).bind(job)
         .execute(&self.pool).await?.rows_affected() == 1)
    }

    /// Reconcile only the job recorded by a still-owned occurrence. The caller
    /// must hold the shared provider lease for the duration of this operation.
    pub async fn fail_abandoned_job(
        &self,
        schedule: Uuid,
        when: DateTime<Utc>,
        lease: Uuid,
        job: Uuid,
        provider: Uuid,
        mapping: &str,
    ) -> Result<bool, AppError> {
        Ok(sqlx::query(
            r#"UPDATE sync_job j SET status = 'failed', finished_at = clock_timestamp(),
                 stats = '{"failed_count":1,"error_code":"ABANDONED_PULL"}'::jsonb
               WHERE j.id = $4 AND j.provider_id = $5 AND j.mapping_version = $6 AND j.status IN ('queued', 'running')
                 AND EXISTS (SELECT 1 FROM sync_pull_schedule_occurrence o
                   WHERE o.schedule_id = $1 AND o.scheduled_for = $2 AND o.lease_id = $3
                     AND o.lease_expires_at > clock_timestamp() AND o.status = 'running' AND o.job_id = j.id)"#,
        ).bind(schedule).bind(when).bind(lease).bind(job).bind(provider).bind(mapping)
         .execute(&self.pool).await?.rows_affected() == 1)
    }

    pub async fn finish(
        &self,
        schedule: Uuid,
        when: DateTime<Utc>,
        lease: Uuid,
        result: OccurrenceResult,
        code: &str,
    ) -> Result<bool, AppError> {
        if code.is_empty() || code.len() > 100 {
            return Err(AppError::Validation(
                "invalid occurrence result code".into(),
            ));
        }
        Ok(sqlx::query(
            r#"UPDATE sync_pull_schedule_occurrence SET status = $4, result_code = $5,
                 lease_id = NULL, lease_expires_at = NULL, updated_at = clock_timestamp()
               WHERE schedule_id = $1 AND scheduled_for = $2 AND lease_id = $3
                 AND lease_expires_at > clock_timestamp()
                 AND (($4 = 'completed' AND status = 'running')
                   OR ($4 = 'failed' AND status IN ('claimed', 'running'))
                   OR ($4 = 'skipped' AND status = 'claimed'))"#,
        )
        .bind(schedule)
        .bind(when)
        .bind(lease)
        .bind(result.as_str())
        .bind(code)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    pub async fn cancel(&self, schedule: Uuid) -> Result<bool, AppError> {
        Ok(sqlx::query("UPDATE sync_pull_schedule SET enabled = false, cancelled_at = clock_timestamp(), updated_at = clock_timestamp() WHERE id = $1 AND cancelled_at IS NULL")
            .bind(schedule).execute(&self.pool).await?.rows_affected() == 1)
    }

    /// Give up a recovery claim without changing its job or terminal status.
    pub async fn release(
        &self,
        schedule: Uuid,
        when: DateTime<Utc>,
        lease: Uuid,
    ) -> Result<bool, AppError> {
        Ok(sqlx::query(
            "UPDATE sync_pull_schedule_occurrence SET lease_id = NULL, lease_expires_at = NULL, updated_at = clock_timestamp() WHERE schedule_id = $1 AND scheduled_for = $2 AND lease_id = $3 AND lease_expires_at > clock_timestamp() AND status IN ('claimed', 'running')",
        ).bind(schedule).bind(when).bind(lease).execute(&self.pool).await?.rows_affected() == 1)
    }

    pub async fn occurrences(
        &self,
        schedule: Uuid,
        limit: i64,
    ) -> Result<Vec<PullOccurrence>, AppError> {
        Ok(sqlx::query_as("SELECT * FROM sync_pull_schedule_occurrence WHERE schedule_id = $1 ORDER BY scheduled_for DESC LIMIT $2")
            .bind(schedule).bind(limit.clamp(1, 500)).fetch_all(&self.pool).await?)
    }
}

fn validate_ttl(ttl_seconds: i32) -> Result<(), AppError> {
    if !(1..=3600).contains(&ttl_seconds) {
        return Err(AppError::Validation(
            "schedule lease TTL must be between 1 and 3600 seconds".into(),
        ));
    }
    Ok(())
}
