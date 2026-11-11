use std::{sync::Arc, time::Duration};

use tokio::sync::watch;
use uuid::Uuid;

use super::{
    AppState,
    worker_backoff::{WorkerBackoff, WorkerRetryConfig},
};
use crate::api::sync_helpers::{build_inventory_pull_runner, build_pull_schedule_service};
use crate::config::constants::SYNC_LOCK_TTL_SECONDS;
use crate::contexts::sync::application::inventory_pull::InventoryPullError;
use crate::contexts::sync::application::lock::{ProviderLease, ProviderLeaseError};
use crate::contexts::sync::application::pull_schedule::{
    PullOccurrenceLease, next_occurrence, schedule_principal,
};
use crate::contexts::sync::domain::{MappingRef, SyncStatus};
use crate::contexts::sync::infrastructure::persistence::{
    OccurrenceResult, PullOccurrence, PullSchedule, SqlxPullScheduleRepository, SyncJobRepository,
};
use crate::errors::AppError;
use crate::observability::metrics::{self, Worker};

const POLL_MS: u64 = 5_000;
const LEASE_SECONDS: i32 = 90;

pub(super) async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut backoff = WorkerBackoff::starting_immediately(POLL_MS, WorkerRetryConfig::POLLER);
    metrics::record_worker_heartbeat(Worker::PullSchedule);
    loop {
        tokio::select! {
            changed = shutdown.changed() => if changed.is_err() || *shutdown.borrow() { return; },
            _ = tokio::time::sleep(Duration::from_millis(backoff.next_sleep_ms())) => {
                metrics::record_worker_heartbeat(Worker::PullSchedule);
                let mut tick = metrics::start_worker_tick(Worker::PullSchedule);
                match tick_once(&state, &mut shutdown).await {
                    Ok(()) => { tick.record(true); backoff.on_success(); }
                    Err(error) => {
                        tick.record(false);
                        tracing::warn!(error = %error, "pull schedule worker tick failed");
                        backoff.on_failure();
                    }
                }
            }
        }
    }
}

async fn tick_once(state: &AppState, shutdown: &mut watch::Receiver<bool>) -> Result<(), AppError> {
    let service = build_pull_schedule_service(state);
    let repo = &service.schedules;
    let recovery_id = Uuid::now_v7();
    if let Some(occurrence) = repo
        .claim_expired(recovery_id, LEASE_SECONDS, 1)
        .await?
        .into_iter()
        .next()
    {
        process(state, repo, occurrence, shutdown).await?;
    }
    if *shutdown.borrow() {
        return Ok(());
    }
    if let Some(due) = repo.list_due(1).await?.into_iter().next() {
        let Some(scheduled_for) = due.schedule.next_run_at else {
            return Ok(());
        };
        let next = next_occurrence(
            &due.schedule.cron_expression,
            &due.schedule.timezone,
            due.database_now,
        )?;
        if let Some(occurrence) = repo
            .claim_due(
                due.schedule.id,
                scheduled_for,
                next,
                Uuid::now_v7(),
                LEASE_SECONDS,
            )
            .await?
        {
            process(state, repo, occurrence, shutdown).await?;
        }
    }
    Ok(())
}

async fn process(
    state: &AppState,
    repo: &SqlxPullScheduleRepository,
    occurrence: PullOccurrence,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), AppError> {
    let lease = occurrence
        .lease_id
        .ok_or_else(|| AppError::Internal("claimed occurrence has no lease".into()))?;
    let schedule = repo
        .get(occurrence.schedule_id)
        .await?
        .ok_or_else(|| AppError::Internal("occurrence schedule is missing".into()))?;
    let recovery = occurrence.status == "running" || schedule.cancelled_at.is_some();
    let action = execute(state, repo, &schedule, &occurrence, lease);
    tokio::pin!(action);
    let mut renewal = tokio::time::interval(Duration::from_secs((LEASE_SECONDS / 3) as u64));
    renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            outcome = &mut action => return outcome,
            changed = shutdown.changed() => if changed.is_err() || *shutdown.borrow() { return Ok(()); },
            _ = renewal.tick() => {
                metrics::record_worker_heartbeat(Worker::PullSchedule);
                let renewal = async {
                    if recovery {
                        repo.renew_recovery(schedule.id, occurrence.scheduled_for, lease, LEASE_SECONDS).await
                    } else {
                        repo.renew_heartbeat(schedule.id, occurrence.scheduled_for, lease, LEASE_SECONDS).await
                    }
                };
                let owned = tokio::time::timeout(Duration::from_secs(10), renewal).await
                    .map_err(|_| AppError::Database("pull occurrence renewal timed out".into()))??;
                if !owned { return Err(AppError::Conflict("pull occurrence lease lost or cancelled".into())); }
            }
        }
    }
}

async fn execute(
    state: &AppState,
    repo: &SqlxPullScheduleRepository,
    schedule: &PullSchedule,
    occurrence: &PullOccurrence,
    lease: Uuid,
) -> Result<(), AppError> {
    if occurrence.status == "running" {
        return reconcile(state, repo, schedule, occurrence, lease).await;
    }
    if schedule.cancelled_at.is_some() {
        return finish(
            repo,
            schedule,
            occurrence,
            lease,
            OccurrenceResult::Failed,
            "CANCELLED",
        )
        .await;
    }
    let service = build_pull_schedule_service(state);
    let mapping = MappingRef::new(schedule.mapping_logical_id, schedule.mapping_version_id);
    match service.validate_source(schedule.provider_id, mapping).await {
        Ok(()) => {}
        Err(AppError::Validation(_) | AppError::NotFound(_)) => {
            return finish(
                repo,
                schedule,
                occurrence,
                lease,
                OccurrenceResult::Failed,
                "SOURCE_OR_MAPPING_UNAVAILABLE",
            )
            .await;
        }
        Err(error) => return Err(error),
    }
    let provider = state
        .services
        .sync_queries
        .get_provider(schedule.provider_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Provider not found".into()))?;
    let runner = match build_inventory_pull_runner(state, &provider).await {
        Ok(runner) => runner,
        Err(_) => {
            return finish(
                repo,
                schedule,
                occurrence,
                lease,
                OccurrenceResult::Failed,
                "SOURCE_UNAVAILABLE",
            )
            .await;
        }
    };
    let principal = schedule_principal(schedule)?;
    let admission = Arc::new(PullOccurrenceLease {
        repository: repo.clone(),
        schedule_id: schedule.id,
        scheduled_for: occurrence.scheduled_for,
        lease_id: lease,
        ttl_seconds: LEASE_SECONDS,
    });
    let result = runner
        .run_scheduled(
            schedule.provider_id,
            &mapping.to_wire(),
            principal.request_context(Uuid::now_v7().to_string()),
            admission,
        )
        .await;
    let (status, code) = match result {
        Ok(_) => (OccurrenceResult::Completed, "COMPLETED"),
        Err(InventoryPullError::Busy(_)) => (OccurrenceResult::Skipped, "PROVIDER_BUSY"),
        Err(InventoryPullError::Failed(_)) => (OccurrenceResult::Failed, "PULL_FAILED"),
    };
    finish(repo, schedule, occurrence, lease, status, code).await
}

async fn finish(
    repo: &SqlxPullScheduleRepository,
    schedule: &PullSchedule,
    occurrence: &PullOccurrence,
    lease: Uuid,
    status: OccurrenceResult,
    code: &str,
) -> Result<(), AppError> {
    if repo
        .finish(schedule.id, occurrence.scheduled_for, lease, status, code)
        .await?
    {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "pull occurrence completion lost lease".into(),
        ))
    }
}

async fn reconcile(
    state: &AppState,
    repo: &SqlxPullScheduleRepository,
    schedule: &PullSchedule,
    occurrence: &PullOccurrence,
    lease_id: Uuid,
) -> Result<(), AppError> {
    let lease = match ProviderLease::acquire(
        state.redis.clone(),
        schedule.provider_id,
        SYNC_LOCK_TTL_SECONDS as usize,
    )
    .await
    {
        Ok(lease) => lease,
        Err(ProviderLeaseError::Busy(_)) => {
            if !repo
                .release(schedule.id, occurrence.scheduled_for, lease_id)
                .await?
            {
                return Err(AppError::Conflict("recovery lease lost".into()));
            }
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    lease
        .run(|authorisation| async move {
            let job_id = occurrence
                .job_id
                .ok_or_else(|| AppError::Internal("running occurrence has no job id".into()))?;
            let job = state.repositories.sync_jobs.get_by_id(job_id).await?;
            let (status, code) = match job {
                None => (OccurrenceResult::Failed, "ABANDONED_BEFORE_JOB"),
                Some(job)
                    if job.provider_id != schedule.provider_id
                        || job.mapping_version
                            != MappingRef::new(
                                schedule.mapping_logical_id,
                                schedule.mapping_version_id,
                            )
                            .to_wire() =>
                {
                    (OccurrenceResult::Failed, "JOB_ID_MISMATCH")
                }
                Some(job) if job.status == SyncStatus::Completed => {
                    (OccurrenceResult::Completed, "COMPLETED")
                }
                Some(job) if matches!(job.status, SyncStatus::Queued | SyncStatus::Running) => {
                    authorisation.ensure_owned().await?;
                    if repo
                        .fail_abandoned_job(
                            schedule.id,
                            occurrence.scheduled_for,
                            lease_id,
                            job.id,
                            schedule.provider_id,
                            &job.mapping_version,
                        )
                        .await?
                    {
                        (OccurrenceResult::Failed, "ABANDONED_PULL")
                    } else {
                        let latest = state.repositories.sync_jobs.get_by_id(job_id).await?;
                        if latest.is_some_and(|value| value.status == SyncStatus::Completed) {
                            (OccurrenceResult::Completed, "COMPLETED")
                        } else {
                            (OccurrenceResult::Failed, "PULL_FAILED")
                        }
                    }
                }
                Some(_) => (OccurrenceResult::Failed, "PULL_FAILED"),
            };
            if !repo
                .finish(
                    schedule.id,
                    occurrence.scheduled_for,
                    lease_id,
                    status,
                    code,
                )
                .await?
            {
                return Err(AppError::Conflict("recovery lease lost".into()));
            }
            Ok(())
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use sqlx::postgres::PgPoolOptions;

    #[tokio::test]
    #[ignore = "requires a PostgreSQL superuser and an isolated Redis fixture"]
    async fn worker_preserves_busy_job_then_recovers_and_refuses_disabled_source()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let admin_url = std::env::var("PIPING_DATABASE__URL")?;
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await?;
        // UUID-derived identifier is controlled by the test, never user input.
        let name = format!("piping_schedule_{}", Uuid::now_v7().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await?;
        admin.close().await;
        let mut url = url::Url::parse(&admin_url)?;
        url.set_path(&format!("/{name}"));
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(url.as_str())
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let redis_url = std::env::var("PIPING_REDIS__URL")?;
        let redis = redis::aio::ConnectionManager::new(redis::Client::open(redis_url)?).await?;
        let mut settings = crate::config::load_settings()?;
        settings.database.url = url.to_string();
        settings.redis.url = std::env::var("PIPING_REDIS__URL")?;
        settings.auth.local_enabled = false;
        settings.security.allow_weak_dev_secrets = true;
        let state = AppState::build(Arc::new(settings), pool.clone(), redis.clone()).await?;
        let repo = SqlxPullScheduleRepository::new(pool.clone());
        let now = repo.database_now().await?;
        let mut schedules = Vec::new();
        for disabled in [false, true] {
            let provider = Uuid::now_v7();
            let mapping = Uuid::now_v7();
            sqlx::query("INSERT INTO sync_provider(id,name,provider_type,config,is_enabled) VALUES($1,$2,'netbox','{\"sync_mode\":\"pull_only\"}',true)")
                .bind(provider).bind(format!("worker-test-{provider}")).execute(&pool).await?;
            sqlx::query("INSERT INTO mapping_configurations(logical_id,version_id,name,source_provider_id,status,direction,conflict_mode) VALUES($1,1,'worker-test',$2,'active','pull','netbox_wins')")
                .bind(mapping).bind(provider).execute(&pool).await?;
            let due = now - ChronoDuration::minutes(2);
            let schedule = repo
                .create(&PullSchedule {
                    id: Uuid::now_v7(),
                    provider_id: provider,
                    mapping_logical_id: mapping,
                    mapping_version_id: 1,
                    principal_method: "session".into(),
                    principal_user_id: Uuid::now_v7(),
                    principal_token_id: None,
                    cron_expression: "* * * * *".into(),
                    timezone: "UTC".into(),
                    enabled: true,
                    cancelled_at: None,
                    next_run_at: Some(due),
                    last_run_at: None,
                    created_at: now,
                    updated_at: now,
                })
                .await?;
            if disabled {
                sqlx::query("UPDATE sync_provider SET is_enabled = false WHERE id = $1")
                    .bind(provider)
                    .execute(&pool)
                    .await?;
            }
            schedules.push((schedule, due));
        }
        let (recovery, due) = &schedules[0];
        let lease_id = Uuid::now_v7();
        repo.claim_due(
            recovery.id,
            *due,
            Some(now + ChronoDuration::days(1)),
            lease_id,
            LEASE_SECONDS,
        )
        .await?
        .ok_or("claim missing")?;
        let job_id = Uuid::now_v7();
        let initial = ProviderLease::acquire(redis.clone(), recovery.provider_id, 60).await?;
        initial.run(|_| async {
            if !repo.record_admitted_job(recovery.id, *due, lease_id, job_id).await? {
                return Err(AppError::Conflict("initial occurrence admission failed".into()));
            }
            sqlx::query("INSERT INTO sync_job(id,provider_id,mapping_version,status) VALUES($1,$2,$3,'running')")
                .bind(job_id).bind(recovery.provider_id)
                .bind(MappingRef::new(recovery.mapping_logical_id, recovery.mapping_version_id).to_wire())
                .execute(&pool).await?;
            Ok(())
        }).await?;
        sqlx::query("UPDATE sync_pull_schedule_occurrence SET lease_expires_at = clock_timestamp() - interval '1 second' WHERE schedule_id = $1 AND scheduled_for = $2")
            .bind(recovery.id).bind(*due).execute(&pool).await?;
        let (_shutdown_tx, mut shutdown) = watch::channel(false);
        let manual = ProviderLease::acquire(redis, recovery.provider_id, 60).await?;
        manual
            .run(|_| async {
                tick_once(&state, &mut shutdown).await?;
                let job = state
                    .repositories
                    .sync_jobs
                    .get_by_id(job_id)
                    .await?
                    .ok_or_else(|| AppError::NotFound("job missing".into()))?;
                assert_eq!(job.status, SyncStatus::Running);
                let occurrence = repo
                    .occurrences(recovery.id, 1)
                    .await?
                    .pop()
                    .ok_or_else(|| AppError::NotFound("occurrence missing".into()))?;
                assert_eq!(occurrence.status, "running");
                Ok(())
            })
            .await?;
        let disabled_occurrence = repo
            .occurrences(schedules[1].0.id, 1)
            .await?
            .pop()
            .ok_or("disabled occurrence missing")?;
        assert_eq!(disabled_occurrence.status, "failed");
        assert_eq!(
            disabled_occurrence.result_code.as_deref(),
            Some("SOURCE_OR_MAPPING_UNAVAILABLE")
        );
        let disabled_jobs: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sync_job WHERE provider_id = $1")
                .bind(schedules[1].0.provider_id)
                .fetch_one(&pool)
                .await?;
        assert_eq!(disabled_jobs, 0);
        tick_once(&state, &mut shutdown).await?;
        let recovered = state
            .repositories
            .sync_jobs
            .get_by_id(job_id)
            .await?
            .ok_or("job missing")?;
        assert_eq!(recovered.status, SyncStatus::Failed);
        let occurrence = repo
            .occurrences(recovery.id, 1)
            .await?
            .pop()
            .ok_or("occurrence missing")?;
        assert_eq!(occurrence.result_code.as_deref(), Some("ABANDONED_PULL"));
        Ok(())
    }
}
