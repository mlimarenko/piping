use chrono::Duration;
use piping_backend::contexts::sync::application::lock::{ProviderLease, ProviderLeaseError};
use piping_backend::contexts::sync::infrastructure::persistence::{
    OccurrenceResult, PullSchedule, SqlxPullScheduleRepository,
};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
#[ignore = "requires isolated PostgreSQL and Redis fixtures"]
async fn leases_are_unique_expire_and_cancel_without_overwriting_a_successor() -> TestResult {
    let url = std::env::var("PIPING_DATABASE__URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    let provider = Uuid::now_v7();
    let mapping = Uuid::now_v7();
    sqlx::query("INSERT INTO sync_provider (id, name, provider_type, config, is_enabled) VALUES ($1, $2, 'netbox', '{\"sync_mode\":\"pull_only\"}', true)")
        .bind(provider).bind(format!("schedule-test-{provider}")).execute(&pool).await?;
    sqlx::query("INSERT INTO mapping_configurations (logical_id, version_id, name, source_provider_id, status, direction, conflict_mode) VALUES ($1, 1, 'schedule-test', $2, 'active', 'pull', 'netbox_wins')")
        .bind(mapping).bind(provider).execute(&pool).await?;
    let repository = SqlxPullScheduleRepository::new(pool.clone());
    let now = repository.database_now().await?;
    let due = now - Duration::hours(2);
    let next = now + Duration::minutes(1);
    let schedule = repository
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
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    let (a, b) = tokio::join!(
        repository.claim_due(schedule.id, due, Some(next), first, 60),
        repository.claim_due(schedule.id, due, Some(next), second, 60)
    );
    let a = a?;
    let b = b?;
    assert_ne!(a.is_some(), b.is_some());
    let winner = if a.is_some() { first } else { second };
    assert_eq!(
        repository.get(schedule.id).await?.unwrap().next_run_at,
        Some(next)
    );
    assert_eq!(repository.occurrences(schedule.id, 10).await?.len(), 1);
    assert!(repository.renew(schedule.id, due, winner, 60).await?);
    let job = Uuid::now_v7();
    assert!(
        repository
            .record_admitted_job(schedule.id, due, winner, job)
            .await?
    );
    let mapping_wire = format!("{mapping}:1");
    sqlx::query("INSERT INTO sync_job (id, provider_id, mapping_version, status) VALUES ($1, $2, $3, 'running')")
        .bind(job).bind(provider).bind(&mapping_wire).execute(&pool).await?;
    sqlx::query("UPDATE sync_provider SET is_enabled = false WHERE id = $1")
        .bind(provider)
        .execute(&pool)
        .await?;
    assert!(!repository.renew(schedule.id, due, winner, 60).await?);
    assert!(
        repository
            .renew_heartbeat(schedule.id, due, winner, 60)
            .await?
    );
    sqlx::query("UPDATE sync_provider SET is_enabled = true, config = '{\"sync_mode\":\"bidirectional\"}' WHERE id = $1")
        .bind(provider)
        .execute(&pool)
        .await?;
    assert!(!repository.renew(schedule.id, due, winner, 60).await?);
    sqlx::query("UPDATE sync_provider SET config = '{\"sync_mode\":\"pull_only\"}' WHERE id = $1")
        .bind(provider)
        .execute(&pool)
        .await?;

    sqlx::query("UPDATE sync_pull_schedule_occurrence SET lease_expires_at = clock_timestamp() - interval '1 second' WHERE schedule_id = $1 AND scheduled_for = $2")
        .bind(schedule.id).bind(due).execute(&pool).await?;
    assert!(!repository.renew(schedule.id, due, winner, 60).await?);
    let recovery = Uuid::now_v7();
    let recovered = repository.claim_expired(recovery, 60, 10).await?;
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, "running");
    assert_eq!(recovered[0].job_id, Some(job));
    let redis = redis::aio::ConnectionManager::new(redis::Client::open(std::env::var(
        "PIPING_REDIS__URL",
    )?)?)
    .await?;
    let manual = ProviderLease::acquire(redis.clone(), provider, 60).await?;
    manual.run(|_| async {
        assert!(matches!(ProviderLease::acquire(redis.clone(), provider, 60).await, Err(ProviderLeaseError::Busy(id)) if id == provider));
        let status: String = sqlx::query_scalar("SELECT status FROM sync_job WHERE id = $1").bind(job).fetch_one(&pool).await?;
        assert_eq!(status, "running");
        Ok(())
    }).await?;
    let admission = ProviderLease::acquire(redis, provider, 60).await?;
    admission
        .run(|_| async {
            assert!(
                !repository
                    .fail_abandoned_job(schedule.id, due, winner, job, provider, &mapping_wire)
                    .await?
            );
            assert!(
                repository
                    .fail_abandoned_job(schedule.id, due, recovery, job, provider, &mapping_wire)
                    .await?
            );
            Ok(())
        })
        .await?;
    let status: String = sqlx::query_scalar("SELECT status FROM sync_job WHERE id = $1")
        .bind(job)
        .fetch_one(&pool)
        .await?;
    assert_eq!(status, "failed");
    assert!(
        !repository
            .finish(schedule.id, due, winner, OccurrenceResult::Failed, "stale")
            .await?
    );
    assert!(
        repository
            .finish(
                schedule.id,
                due,
                recovery,
                OccurrenceResult::Failed,
                "abandoned"
            )
            .await?
    );
    assert_eq!(
        repository.occurrences(schedule.id, 10).await?[0]
            .result_code
            .as_deref(),
        Some("abandoned")
    );

    let claim = repository
        .claim_due(
            schedule.id,
            next,
            Some(next + Duration::minutes(1)),
            Uuid::now_v7(),
            60,
        )
        .await?;
    assert!(
        claim.is_none(),
        "future cursor is not due on the database clock"
    );
    let cancelled_due: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "UPDATE sync_pull_schedule SET next_run_at = clock_timestamp() - interval '1 second' WHERE id = $1 RETURNING next_run_at",
    )
    .bind(schedule.id)
    .fetch_one(&pool)
    .await?;
    let cancel_lease = Uuid::now_v7();
    assert!(
        repository
            .claim_due(schedule.id, cancelled_due, Some(next), cancel_lease, 60)
            .await?
            .is_some()
    );
    assert!(repository.cancel(schedule.id).await?);
    assert!(
        !repository
            .renew(schedule.id, cancelled_due, cancel_lease, 60)
            .await?
    );
    assert!(
        !repository
            .renew_heartbeat(schedule.id, cancelled_due, cancel_lease, 60)
            .await?
    );
    assert!(!repository.cancel(schedule.id).await?);
    assert!(
        repository
            .list_due(100)
            .await?
            .iter()
            .all(|item| item.schedule.id != schedule.id)
    );
    assert!(
        repository
            .get(schedule.id)
            .await?
            .unwrap()
            .cancelled_at
            .is_some()
    );
    Ok(())
}
