//! PostgreSQL coverage for signed-ceiling enforcement and last-login LIFO ordering.

use std::{path::PathBuf, sync::Arc};

use chrono::{DateTime, Duration, Utc};
use piping_backend::contexts::access_control::infrastructure::persistence::repositories::UserApiTokenRepository;
use piping_backend::{
    config::Settings,
    contexts::licensing::{
        application::service::LicensingService,
        infrastructure::repositories::{
            LicensingRepository, SqlxLicensingRepository, repository::RefreshSuccess,
        },
    },
    errors::AppError,
};
use sqlx::PgPool;
use uuid::Uuid;

#[path = "common/mod.rs"]
mod common;
#[path = "support/license_activation_support.rs"]
#[expect(
    dead_code,
    reason = "this target uses only the signing and reset helpers; the remaining support serves sibling licensing targets"
)]
mod support;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
const INSTANCE: &str = "piping-seat-eviction-test";

struct Fixture {
    pool: PgPool,
    service: LicensingService,
    users: [Uuid; 3],
}

impl Fixture {
    async fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let pool = common::build_test_db_pool().await?;
        piping_backend::storage::database::migrations::run_migrations(&pool).await?;
        support::reset_license_tables(&pool).await?;
        sqlx::query("TRUNCATE auth_session, auth_user_api_token CASCADE")
            .execute(&pool)
            .await?;
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/default.toml");
        let settings: Settings = config::Config::builder()
            .add_source(config::File::from(path))
            .build()?
            .try_deserialize()?;
        piping_backend::contexts::secrets::infrastructure::crypto::configure_cipher_key_material_ref(
            "inline:MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        )?;
        let service = LicensingService::new(Arc::new(settings), pool.clone());
        service.activate_license(&token(3)?).await?;
        let users = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
        for (index, user) in users.iter().enumerate() {
            sqlx::query(
                "INSERT INTO identity_user (id, username, email, auth_source) VALUES ($1, $2, $2, 'local')",
            )
            .bind(user)
            .bind(format!("seat-eviction-{user}@example.invalid"))
            .execute(&pool)
            .await?;
            sqlx::query(
                "INSERT INTO license_seat_assignment (user_id, updated_at) VALUES ($1, NOW() - make_interval(days => $2))",
            )
            .bind(user)
            .bind(3 - index as i32)
            .execute(&pool)
            .await?;
            sqlx::query(
                "INSERT INTO auth_session (user_id, expires_at) VALUES ($1, NOW() + INTERVAL '1 hour'), ($1, NOW() - INTERVAL '1 hour')",
            )
            .bind(user)
            .execute(&pool)
            .await?;
            sqlx::query(
                "INSERT INTO auth_user_api_token (user_id, name, token_prefix, token_hash, status) VALUES ($1, 'synthetic', $2, $2, 'active')",
            )
            .bind(user)
            .bind(format!("synthetic-{user}"))
            .execute(&pool)
            .await?;
        }
        Ok(Self {
            pool,
            service,
            users,
        })
    }

    async fn refresh(&self, max_seats: i32) -> TestResult {
        let repository = SqlxLicensingRepository::new(self.pool.clone());
        let context = repository.begin_refresh().await?.ok_or("missing license")?;
        repository
            .mark_refresh_success(RefreshSuccess {
                revision: context.revision,
                instance_id: INSTANCE,
                entitlement_expires_at: Utc::now() + Duration::hours(1),
                max_seats,
                entitlement_jwt: &token(max_seats)?,
                entitlement_version: "v1",
                now: Utc::now(),
            })
            .await?;
        Ok(())
    }

    async fn assign_admin(&self, user_id: Uuid) -> TestResult {
        let role_id: Uuid = sqlx::query_scalar("SELECT id FROM rbac_role WHERE name = 'admin'")
            .fetch_one(&self.pool)
            .await?;
        sqlx::query(
            "INSERT INTO rbac_role_assignment (id, user_id, group_id, role_id)
             VALUES ($1, $2, NULL, $3)",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(role_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn assign_admin_group(&self, user_id: Uuid) -> TestResult {
        let role_id: Uuid = sqlx::query_scalar("SELECT id FROM rbac_role WHERE name = 'admin'")
            .fetch_one(&self.pool)
            .await?;
        let group_id = Uuid::now_v7();
        sqlx::query("INSERT INTO identity_group (id, name) VALUES ($1, $2)")
            .bind(group_id)
            .bind(format!("admins-{group_id}"))
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "INSERT INTO identity_group_membership (id, user_id, group_id, source)
             VALUES ($1, $2, $3, 'manual')",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(group_id)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO rbac_role_assignment (id, user_id, group_id, role_id)
             VALUES ($1, NULL, $2, $3)",
        )
        .bind(Uuid::now_v7())
        .bind(group_id)
        .bind(role_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn revoked_indices(
        &self,
    ) -> Result<Vec<usize>, Box<dyn std::error::Error + Send + Sync>> {
        let mut revoked = Vec::new();
        for (index, user) in self.users.iter().enumerate() {
            let value: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM license_seat_assignment WHERE user_id = $1 AND revoked_at IS NOT NULL)",
            )
            .bind(user)
            .fetch_one(&self.pool)
            .await?;
            if value {
                revoked.push(index);
            }
        }
        Ok(revoked)
    }

    async fn assert_eviction(&self, evicted: usize) -> TestResult {
        for (index, user) in self.users.iter().enumerate() {
            let (revoked, reason): (bool, Option<String>) = sqlx::query_as(
                "SELECT revoked_at IS NOT NULL, revoked_reason FROM license_seat_assignment WHERE user_id = $1 ORDER BY assigned_at DESC LIMIT 1",
            )
            .bind(user)
            .fetch_one(&self.pool)
            .await?;
            assert_eq!(revoked, index == evicted);
            assert_eq!(
                reason.as_deref(),
                (index == evicted).then_some("seat_evicted_over_capacity")
            );
            let sessions: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM auth_session WHERE user_id = $1 AND revoked_at IS NULL",
            )
            .bind(user)
            .fetch_one(&self.pool)
            .await?;
            assert_eq!(sessions, if index == evicted { 0 } else { 2 });
            let (status, revoked, updated): (String, bool, bool) = sqlx::query_as(
                "SELECT status, revoked_at IS NOT NULL, updated_at >= created_at FROM auth_user_api_token WHERE user_id = $1",
            )
            .bind(user)
            .fetch_one(&self.pool)
            .await?;
            assert_eq!(
                status,
                if index == evicted {
                    "revoked"
                } else {
                    "active"
                }
            );
            assert_eq!(revoked, index == evicted);
            assert!(updated);
        }
        let ledger: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT user_id, metadata_json FROM license_seat_ledger WHERE event_type = 'seat_revoked' AND reason_code = 'seat_evicted_over_capacity'",
        )
        .fetch_all(&self.pool)
        .await?;
        assert_eq!(
            ledger,
            vec![(self.users[evicted], serde_json::json!({"max_seats": 2}))]
        );
        Ok(())
    }
}

fn token(max_seats: i32) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    support::build_test_entitlement_token(INSTANCE, max_seats, Utc::now() + Duration::hours(1))
}

#[tokio::test]
#[ignore = "Requires PostgreSQL; run this target with --ignored --test-threads=1"]
async fn refresh_evicts_newest_seat_revokes_credentials_and_is_idempotent() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.refresh(4).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM license_seat_assignment WHERE revoked_at IS NOT NULL",
    )
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(count, 0);
    fixture.refresh(3).await?;
    fixture.refresh(2).await?;
    fixture.assert_eviction(2).await?;
    fixture.refresh(2).await?;
    fixture.assert_eviction(2).await?;
    fixture.refresh(3).await?;
    fixture
        .service
        .enforce_seat_on_login(fixture.users[2])
        .await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM license_seat_assignment WHERE revoked_at IS NULL")
            .fetch_one(&fixture.pool)
            .await?;
    assert_eq!(
        count, 3,
        "an evicted user may re-enter when signed capacity rises"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "Requires PostgreSQL; run this target with --ignored --test-threads=1"]
async fn evicted_user_cannot_mint_api_token() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.refresh(2).await?;
    fixture.assert_eviction(2).await?;
    let repository = piping_backend::contexts::access_control::infrastructure::persistence::repositories::SqlxUserApiTokenRepository::new(fixture.pool.clone());
    let now = Utc::now();
    let token = piping_backend::contexts::access_control::domain::UserApiToken {
        id: Uuid::now_v7(),
        user_id: fixture.users[2],
        name: "post-eviction".to_string(),
        token_prefix: "piping_test".to_string(),
        token_hash: "synthetic-hash".to_string(),
        status: piping_backend::contexts::access_control::domain::UserApiTokenStatus::Active,
        expires_at: None,
        last_used_at: None,
        created_at: now,
        updated_at: now,
        revoked_at: None,
        revoked_by_user_id: None,
        scopes: Vec::new(),
    };
    let audit = piping_backend::contexts::audit::domain::AuditEvent {
        id: Uuid::now_v7(),
        event_type: "access_control.user_api_token.created".to_string(),
        actor_id: Some(fixture.users[2]),
        actor_kind: "user".to_string(),
        target_type: None,
        target_id: Some(fixture.users[2]),
        metadata: serde_json::json!({}),
        created_at: now,
    };
    let error = repository
        .create_token(token, audit)
        .await
        .expect_err("an evicted user must not mint a new API token");
    assert!(matches!(error, AppError::Forbidden(_)));
    Ok(())
}

#[tokio::test]
#[ignore = "Requires PostgreSQL; run this target with --ignored --test-threads=1"]
async fn seated_login_touches_lifo_order_and_unseated_login_is_denied_at_capacity() -> TestResult {
    let fixture = Fixture::new().await?;
    let before: DateTime<Utc> =
        sqlx::query_scalar("SELECT updated_at FROM license_seat_assignment WHERE user_id = $1")
            .bind(fixture.users[0])
            .fetch_one(&fixture.pool)
            .await?;
    let session_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM auth_session WHERE user_id = $1 AND revoked_at IS NULL ORDER BY expires_at DESC LIMIT 1",
    )
    .bind(fixture.users[0])
    .fetch_one(&fixture.pool)
    .await?;
    fixture
        .service
        .confirm_seat_on_login(fixture.users[0], session_id)
        .await?;
    let after: DateTime<Utc> =
        sqlx::query_scalar("SELECT updated_at FROM license_seat_assignment WHERE user_id = $1")
            .bind(fixture.users[0])
            .fetch_one(&fixture.pool)
            .await?;
    assert!(after > before);
    let unseated = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_user (id, username, email, auth_source) VALUES ($1, $2, $2, 'local')",
    )
    .bind(unseated)
    .bind(format!("unseated-{unseated}@example.invalid"))
    .execute(&fixture.pool)
    .await?;
    let error = fixture
        .service
        .enforce_seat_on_login(unseated)
        .await
        .err()
        .ok_or("unseated login must fail at capacity")?;
    assert!(matches!(error, AppError::Forbidden(_)));
    fixture.refresh(2).await?;
    fixture.assert_eviction(0).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "Requires PostgreSQL; run this target with --ignored --test-threads=1"]
async fn replacement_activation_evicts_newest_and_is_idempotent() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.service.activate_license(&token(2)?).await?;
    fixture.assert_eviction(2).await?;
    fixture.service.activate_license(&token(2)?).await?;
    fixture.assert_eviction(2).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "Requires PostgreSQL; run this target with --ignored --test-threads=1"]
async fn eviction_failure_rolls_back_entitlement_seats_and_credentials() -> TestResult {
    let fixture = Fixture::new().await?;
    sqlx::raw_sql(
        r#"
        CREATE FUNCTION reject_eviction_test() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'synthetic eviction ledger failure'; END; $$;
        CREATE TRIGGER reject_eviction_test BEFORE INSERT ON license_seat_ledger
        FOR EACH ROW EXECUTE FUNCTION reject_eviction_test();
        "#,
    )
    .execute(&fixture.pool)
    .await?;
    let refresh = fixture.refresh(2).await;
    let activation = fixture.service.activate_license(&token(2)?).await;
    sqlx::raw_sql(
        "DROP TRIGGER reject_eviction_test ON license_seat_ledger; DROP FUNCTION reject_eviction_test();",
    ).execute(&fixture.pool).await?;
    assert!(refresh.is_err());
    assert!(activation.is_err());
    let max_seats: i32 = sqlx::query_scalar("SELECT max_seats FROM license_state")
        .fetch_one(&fixture.pool)
        .await?;
    assert_eq!(max_seats, 3);
    let revoked: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM license_seat_assignment WHERE revoked_at IS NOT NULL) + (SELECT COUNT(*) FROM auth_session WHERE revoked_at IS NOT NULL) + (SELECT COUNT(*) FROM auth_user_api_token WHERE status = 'revoked')",
    ).fetch_one(&fixture.pool).await?;
    assert_eq!(revoked, 0);
    Ok(())
}

#[tokio::test]
#[ignore = "Requires PostgreSQL; run this target with --ignored --test-threads=1"]
async fn newest_admin_is_protected_while_an_ordinary_user_can_be_evicted() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.assign_admin(fixture.users[2]).await?;
    fixture.refresh(2).await?;
    assert_eq!(fixture.revoked_indices().await?, vec![1]);
    Ok(())
}

#[tokio::test]
#[ignore = "Requires PostgreSQL; run this target with --ignored --test-threads=1"]
async fn group_admin_is_protected_and_last_admin_survives() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.assign_admin_group(fixture.users[2]).await?;
    fixture.refresh(2).await?;
    assert_eq!(fixture.revoked_indices().await?, vec![1]);

    fixture.assign_admin(fixture.users[0]).await?;
    fixture.refresh(1).await?;
    assert_eq!(fixture.revoked_indices().await?, vec![1, 2]);
    let surviving: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM license_seat_assignment WHERE user_id = $1 AND revoked_at IS NULL)",
    )
    .bind(fixture.users[0])
    .fetch_one(&fixture.pool)
    .await?;
    assert!(surviving, "the oldest remaining administrator must survive");
    Ok(())
}
