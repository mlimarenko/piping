//! DB-backed integration test for the scheduled-run C3 secret gate (PWA-62).
//!
//! Manual REST launches already clear the per-secret `Execute × SecretVariable`
//! gate (`ensure_secret_use_grants`) and persist the server-authored snapshot in
//! `run_group.metadata.secret_env_authorization`. Scheduled runs are authored by
//! the schedule worker, which used to pass `secret_env_authorization: None` —
//! so a scheduled fire applied no C3 boundary at all.
//!
//! These tests exercise the public `PlaybookService::create_scheduled_run_group`
//! seam directly against a real Postgres: Test A proves a scheduled fire without
//! the secret grant fails closed (`Forbidden`); Test B proves that with the grant
//! the service authors the same snapshot shape the manual path writes.
//!
//! They are `#[ignore]`d like the rest of the repo's DB tests and run under
//! `make test` (which brings up the dev Postgres).

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use piping_backend::app::{SecretRecheckSubject, resolve_runtime_secret_env};
use piping_backend::contexts::playbooks::application::CreateRunGroupInput;
use piping_backend::contexts::playbooks::domain::PlaybookTargetSetMode;
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CreateServicePrincipalRecord, CreateTargetSetRecord, PlaybookRepository,
    SqlxPlaybookRepository, StaticTargetMemberRecord,
};
use piping_backend::contexts::secrets::application::SecretStore;
use piping_backend::errors::AppError;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

/// Declared runtime secret key (must not start with `PIPING_`/`ANSIBLE_`/`SSH_`).
const SECRET_KEY: &str = "pwa62-grad-key";
/// Env name the version exports the secret as (must match `[A-Z_][A-Z0-9_]*`).
const SECRET_ENV_NAME: &str = "GRAD_TOKEN";
/// A protected secret's key/env: a row exists but must never be exported.
const PROTECTED_SECRET_KEY: &str = "pwa62-protected-key";
const PROTECTED_SECRET_ENV_NAME: &str = "PROTECTED_TOKEN";
/// A declared key with no backing `secrets_variable` row (fail-closed).
const MISSING_SECRET_KEY: &str = "pwa62-missing-key";
const MISSING_SECRET_ENV_NAME: &str = "MISSING_TOKEN";

/// What the seeded version declares in `token_policy.runtime.secrets.env`.
#[derive(Clone, Copy)]
enum DeclaredSecret {
    /// One declared key backed by a real row with the given `is_protected`.
    Row {
        key: &'static str,
        env_name: &'static str,
        is_protected: bool,
    },
    /// One declared key with no backing `secrets_variable` row.
    Missing {
        key: &'static str,
        env_name: &'static str,
    },
    /// No declared runtime secrets at all.
    None,
    /// No runtime secret env, but a token policy that the full
    /// `runtime_policy_from_token_policy` parse rejects (a malformed
    /// non-secret field). Used to prove the scheduled-fire path does not move
    /// this failure from dispatch to fire.
    NoneMalformedPolicy,
}

fn token_policy_for(declared: DeclaredSecret) -> serde_json::Value {
    match declared {
        DeclaredSecret::Row { key, env_name, .. } | DeclaredSecret::Missing { key, env_name } => {
            json!({ "runtime": { "secrets": { "env": { env_name: key } } } })
        }
        DeclaredSecret::None => json!({}),
        // `runtime.piping_api.inventory` without `enabled=true` is rejected by
        // `runtime_policy_from_token_policy`, but declares no runtime secrets.
        DeclaredSecret::NoneMalformedPolicy => {
            json!({ "runtime": { "piping_api": { "enabled": false, "inventory": ["read"] } } })
        }
    }
}

struct Seeded {
    playbook_id: Uuid,
    secret_id: Option<Uuid>,
    target_set_id: Uuid,
    service_principal_id: Uuid,
    schedule_id: Uuid,
    scheduled_for: chrono::DateTime<Utc>,
}

/// Seed an active playbook whose current version declares the given runtime
/// secret scenario, a resolvable local-connection target set, an active service
/// principal, the three SP launch grants, and a schedule occurrence so
/// `create_scheduled_run_group` can reach the secret gate.
async fn seed_scheduled_secret_fixture(
    state: &piping_backend::app::AppState,
    grant_secret_use: bool,
    declared: DeclaredSecret,
) -> Result<Seeded, Box<dyn std::error::Error + Send + Sync>> {
    let pool = &state.db_pool;
    let suffix = common::unique_suffix();
    let playbook_id = Uuid::now_v7();
    let version_id = Uuid::now_v7();

    sqlx::query(
        r#"
        INSERT INTO playbook (id, slug, name, status, runtime, risk, category, connection_default)
        VALUES ($1, $2, 'Scheduled Secret Gate', 'active', 'ansible', 'low', 'general', 'local')
        "#,
    )
    .bind(playbook_id)
    .bind(format!("sched-secret-{suffix}"))
    .execute(pool)
    .await?;

    let token_policy = token_policy_for(declared);
    sqlx::query(
        r#"
        INSERT INTO playbook_version (
            id, playbook_id, version, language, runtime, content, content_sha256, token_policy
        )
        VALUES ($1, $2, 1, 'yaml', 'ansible', '---\n- hosts: all\n', $3, $4)
        "#,
    )
    .bind(version_id)
    .bind(playbook_id)
    // 64 hex chars; any 64-hex passes the CHECK.
    .bind("0000000000000000000000000000000000000000000000000000000000000000")
    .bind(&token_policy)
    .execute(pool)
    .await?;
    sqlx::query("UPDATE playbook SET current_version_id = $2 WHERE id = $1")
        .bind(playbook_id)
        .bind(version_id)
        .execute(pool)
        .await?;

    // A row-backed declared secret must exist; `key` is globally unique, so
    // re-use the same row across test invocations and read back its canonical id.
    // A missing-key scenario deliberately inserts nothing.
    let secret_id: Option<Uuid> = match declared {
        DeclaredSecret::Row {
            key, is_protected, ..
        } => Some(
            sqlx::query_scalar(
                r#"
                INSERT INTO secrets_variable (id, key, is_masked, is_protected)
                VALUES ($1, $2, true, $3)
                ON CONFLICT (key) DO UPDATE SET is_masked = EXCLUDED.is_masked,
                                                is_protected = EXCLUDED.is_protected
                RETURNING id
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(key)
            .bind(is_protected)
            .fetch_one(pool)
            .await?,
        ),
        DeclaredSecret::Missing { .. }
        | DeclaredSecret::None
        | DeclaredSecret::NoneMalformedPolicy => None,
    };

    // A real device makes the static target set resolve to one target. The
    // playbook uses the `local` connection, so the SSH-capability filter is
    // skipped and the address-less device is still a valid run target.
    let (device_id, _name) = common::seed_device(pool, "pwa62").await?;
    let repo = Arc::new(SqlxPlaybookRepository::new(pool.clone()));
    let target_set_id = repo
        .create_target_set(
            CreateTargetSetRecord {
                name: format!("pwa62-sched-set-{suffix}"),
                mode: PlaybookTargetSetMode::Static,
                selector: json!({}),
                members: vec![StaticTargetMemberRecord {
                    target_kind: "connection_target".to_string(),
                    target_id: device_id,
                    source_kind: "device".to_string(),
                    source_id: device_id,
                    sort_order: 0,
                }],
                user_id: None,
            },
            100,
        )
        .await?
        .target_set
        .id;

    let service_principal = repo
        .create_service_principal(CreateServicePrincipalRecord {
            name: format!("PWA-62 Scheduler {suffix}"),
            slug: format!("pwa62-sched-{suffix}"),
            description: Some("scheduled-run secret gate test".to_string()),
            metadata: json!({}),
            user_id: None,
        })
        .await?;
    let service_principal_id = service_principal.id;

    let subject = format!("service_principal:{service_principal_id}");
    for (action, resource_type, resources) in [
        ("EXECUTE", "PLAYBOOK", vec![playbook_id.to_string()]),
        (
            "READ",
            "PLAYBOOK_TARGET_SET",
            vec![target_set_id.to_string()],
        ),
        (
            "CREATE",
            "PLAYBOOK_RUN_GROUP",
            vec!["scheduled".to_string()],
        ),
    ] {
        insert_allow_policy(pool, &subject, action, resource_type, resources).await?;
    }
    if let (true, Some(secret_id)) = (grant_secret_use, secret_id) {
        insert_allow_policy(
            pool,
            &subject,
            "EXECUTE",
            "SECRET_VARIABLE",
            vec![secret_id.to_string()],
        )
        .await?;
    }

    // Schedule + occurrence: the run group's `(schedule_id, scheduled_for)` FK
    // references `playbook_schedule_occurrence`, and the repository locks the
    // `playbook_schedule` row for its concurrency policy.
    let schedule_id = Uuid::now_v7();
    let scheduled_for = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO playbook_schedule (
            id, playbook_id, target_set_id, service_principal_id, name, cron_expression,
            timezone, misfire_policy, concurrency_policy, enabled, next_run_at, metadata
        )
        VALUES ($1, $2, $3, $4, $5, '0 3 * * *', 'UTC', 'fire_once', 'queue_pending',
                false, NULL, '{}'::jsonb)
        "#,
    )
    .bind(schedule_id)
    .bind(playbook_id)
    .bind(target_set_id)
    .bind(service_principal_id)
    .bind(format!("pwa62-schedule-{suffix}"))
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO playbook_schedule_occurrence (
            schedule_id, scheduled_for, fired_at, created_at, updated_at
        )
        VALUES ($1, $2, $2, NOW(), NOW())
        "#,
    )
    .bind(schedule_id)
    .bind(scheduled_for)
    .execute(pool)
    .await?;

    Ok(Seeded {
        playbook_id,
        secret_id,
        target_set_id,
        service_principal_id,
        schedule_id,
        scheduled_for,
    })
}

async fn insert_allow_policy(
    pool: &sqlx::PgPool,
    subject: &str,
    action: &str,
    resource_type: &str,
    resources: Vec<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        r#"
        INSERT INTO rbac_policy (id, name, effect, actions, resource_types, subjects, resources)
        VALUES ($1, $2, 'allow', $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::now_v7())
    // `rbac_policy.name` is globally unique; namespace per policy row so the
    // two tests (and repeated runs) cannot collide.
    .bind(format!(
        "pwa62-{action}-{resource_type}-{}",
        Uuid::now_v7().simple()
    ))
    .bind(json!([action]))
    .bind(json!([resource_type]))
    .bind(json!([subject]))
    .bind(json!(resources))
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_scheduled(
    state: &piping_backend::app::AppState,
    seeded: &Seeded,
    secret_env_authorization: Option<serde_json::Value>,
) -> Result<piping_backend::contexts::playbooks::application::PlaybookRunGroupCreated, AppError> {
    state
        .services
        .playbooks
        .create_scheduled_run_group(
            seeded.playbook_id,
            CreateRunGroupInput {
                target_set_id: Some(seeded.target_set_id),
                schedule_id: Some(seeded.schedule_id),
                scheduled_for: Some(seeded.scheduled_for),
                trigger_kind: None,
                target_limit: None,
                metadata: None,
                secret_env_authorization,
            },
            Some(seeded.service_principal_id),
        )
        .await
}

/// RED (today): a scheduled fire of a secret-bearing playbook by a service
/// principal WITHOUT `Execute × SecretVariable` returns `Ok`. The gate must
/// make it fail closed with `Forbidden`.
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn scheduled_fire_without_secret_grant_fails_closed()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    let seeded = seed_scheduled_secret_fixture(
        &state,
        false,
        DeclaredSecret::Row {
            key: SECRET_KEY,
            env_name: SECRET_ENV_NAME,
            is_protected: false,
        },
    )
    .await?;

    let result = create_scheduled(&state, &seeded, None).await;
    assert!(
        matches!(result, Err(AppError::Forbidden(_))),
        "scheduled fire without Execute × SecretVariable must fail closed with Forbidden, got: {result:?}"
    );

    Ok(())
}

/// GREEN: with the secret grant the service authors the same
/// `secret_env_authorization` snapshot shape the manual REST path writes, so the
/// worker can re-check the run group's real principal at execution time.
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn scheduled_fire_with_secret_grant_authors_snapshot()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    let seeded = seed_scheduled_secret_fixture(
        &state,
        true,
        DeclaredSecret::Row {
            key: SECRET_KEY,
            env_name: SECRET_ENV_NAME,
            is_protected: false,
        },
    )
    .await?;

    let created = create_scheduled(&state, &seeded, None)
        .await
        .expect("scheduled fire with the secret grant must succeed");

    let snapshot = created
        .run_group
        .metadata
        .get("secret_env_authorization")
        .expect("service must author the secret_env_authorization snapshot");
    let keys: std::collections::BTreeSet<&str> = snapshot
        .as_object()
        .expect("snapshot is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["decided_at", "subject", "env"].into_iter().collect(),
        "snapshot keys must match the manual-path shape"
    );
    assert_eq!(
        snapshot.get("subject").and_then(|v| v.as_str()),
        Some(format!("service_principal:{}", seeded.service_principal_id).as_str()),
        "snapshot subject must be the run group's service principal"
    );

    let entry = snapshot
        .get("env")
        .and_then(|env| env.get(SECRET_ENV_NAME))
        .expect("snapshot must carry the declared env name");
    assert_eq!(
        entry.get("secret_id").and_then(|v| v.as_str()),
        Some(
            seeded
                .secret_id
                .expect("row-backed scenario has a secret id")
                .to_string()
                .as_str()
        ),
        "snapshot must record the resolved secret id"
    );
    assert_eq!(
        entry.get("key").and_then(|v| v.as_str()),
        Some(SECRET_KEY),
        "snapshot must record the resolved secret key"
    );

    // Sanity: the run group really is the scheduled one under the SP.
    let row = sqlx::query(
        "SELECT service_principal_id, trigger_kind FROM playbook_run_group WHERE id = $1",
    )
    .bind(created.run_group.id)
    .fetch_one(&state.db_pool)
    .await?;
    assert_eq!(
        row.try_get::<Uuid, _>("service_principal_id")?,
        seeded.service_principal_id
    );
    assert_eq!(row.try_get::<String, _>("trigger_kind")?, "schedule");

    Ok(())
}

/// A version that declares a secret backed by an `is_protected=true` row must be
/// refused even when the service principal holds `Execute × SecretVariable`:
/// protected values are never exported to a run environment. The refusal happens
/// before the run group is authored, so no row is left behind.
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn scheduled_fire_refuses_a_protected_secret()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    let seeded = seed_scheduled_secret_fixture(
        &state,
        true,
        DeclaredSecret::Row {
            key: PROTECTED_SECRET_KEY,
            env_name: PROTECTED_SECRET_ENV_NAME,
            is_protected: true,
        },
    )
    .await?;

    let result = create_scheduled(&state, &seeded, None).await;
    assert!(
        matches!(result, Err(AppError::Forbidden(_))),
        "a protected declared secret must fail closed with Forbidden, got: {result:?}"
    );
    assert_eq!(
        run_groups_for_occurrence(&state, &seeded).await?,
        0,
        "a refused scheduled fire must not author a run group"
    );

    Ok(())
}

/// A version that declares a key with no `secrets_variable` row must fail closed
/// with a validation error naming the missing key, and must not author a run
/// group.
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn scheduled_fire_refuses_a_missing_secret_key()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    let seeded = seed_scheduled_secret_fixture(
        &state,
        false,
        DeclaredSecret::Missing {
            key: MISSING_SECRET_KEY,
            env_name: MISSING_SECRET_ENV_NAME,
        },
    )
    .await?;

    let result = create_scheduled(&state, &seeded, None).await;
    assert!(
        matches!(&result, Err(AppError::Validation(message)) if message.contains("does not exist")),
        "a declared key with no secret row must fail closed with Validation, got: {result:?}"
    );
    assert_eq!(
        run_groups_for_occurrence(&state, &seeded).await?,
        0,
        "a refused scheduled fire must not author a run group"
    );

    Ok(())
}

/// Fix 1: with NO declared secrets the service must NOT persist a
/// caller-supplied `secret_env_authorization` — the snapshot is server-authored
/// unconditionally, so a forged value must be dropped.
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn scheduled_fire_drops_a_caller_supplied_snapshot_without_secrets()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    let seeded = seed_scheduled_secret_fixture(&state, false, DeclaredSecret::None).await?;
    let forged = json!({
        "env": {"FORGED": {"secret_id": "01890000-0000-7000-8000-0000000000ff", "key": "forged-key"}},
        "subject": "user:forged"
    });

    let created = create_scheduled(&state, &seeded, Some(forged))
        .await
        .expect("a version without declared secrets launches without a snapshot");

    assert!(
        created
            .run_group
            .metadata
            .get("secret_env_authorization")
            .is_none(),
        "the returned run group must not carry a forged snapshot"
    );
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT metadata FROM playbook_run_group WHERE id = $1")
            .bind(created.run_group.id)
            .fetch_one(&state.db_pool)
            .await?;
    assert!(
        stored.get("secret_env_authorization").is_none(),
        "the persisted run group must not carry a forged snapshot: {stored}"
    );

    Ok(())
}

async fn run_groups_for_occurrence(
    state: &piping_backend::app::AppState,
    seeded: &Seeded,
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM playbook_run_group WHERE schedule_id = $1 AND scheduled_for = $2",
    )
    .bind(seeded.schedule_id)
    .bind(seeded.scheduled_for)
    .fetch_one(&state.db_pool)
    .await?)
}

/// A version with NO declared runtime secrets but a token policy the full
/// `runtime_policy_from_token_policy` parse rejects must still author at fire:
/// only the dispatch worker owns that version's policy validation, so the
/// failure must not move earlier. (Before the fix the scheduled path parsed the
/// policy unconditionally and returned the validation error at fire.)
#[tokio::test]
#[ignore = "requires postgres (make test)"]
async fn scheduled_fire_without_secrets_defers_malformed_policy_to_dispatch()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = common::build_test_app_state().await.expect(
        "test database must be reachable; this suite is #[ignore]d and only runs in the DB lane",
    );
    let seeded =
        seed_scheduled_secret_fixture(&state, false, DeclaredSecret::NoneMalformedPolicy).await?;

    let created = create_scheduled(&state, &seeded, None).await.expect(
        "a secretless version must not parse its token policy at fire; \
         the worker keeps owning dispatch-time policy validation",
    );
    assert!(
        created
            .run_group
            .metadata
            .get("secret_env_authorization")
            .is_none(),
        "a secretless run group must not carry a secret snapshot"
    );
    assert_eq!(
        run_groups_for_occurrence(&state, &seeded).await?,
        1,
        "the secretless scheduled fire must author exactly one run group"
    );

    Ok(())
}

/// C3 dispatch re-check, exercised through the real
/// `resolve_runtime_secret_env` with a service-principal subject.
mod dispatch_secret_recheck {
    use super::*;

    /// The dispatch re-check must export the secret when the service principal
    /// is still active and still holds `Execute × SecretVariable`, resolving the
    /// value by the snapshot's `secret_id`.
    #[tokio::test]
    #[ignore = "requires postgres (make test)"]
    async fn resolves_with_an_active_service_principal()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let fixture = seed_dispatch_fixture().await?;
        let env = fixture
            .resolve(&fixture.secret_key, &fixture.secret_key)
            .await
            .expect("an active service principal with the grant must resolve the secret");
        assert_eq!(
            env.get(SECRET_ENV_NAME).map(String::as_str),
            Some(fixture.secret_value.as_str()),
            "the exported env value must be the scoped secret's stored value"
        );
        Ok(())
    }

    /// An SP revoked between authoring and dispatch must NOT export the secret:
    /// the dispatch re-check re-loads the principal and refuses a non-active,
    /// revoked row instead of trusting the bare `service_principal_id`.
    #[tokio::test]
    #[ignore = "requires postgres (make test)"]
    async fn refuses_a_revoked_service_principal()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let fixture = seed_dispatch_fixture().await?;
        sqlx::query(
            "UPDATE playbook_service_principal SET status = 'revoked', revoked_at = NOW() \
             WHERE id = $1",
        )
        .bind(fixture.service_principal_id)
        .execute(&fixture.state.db_pool)
        .await?;

        let result = fixture
            .resolve(&fixture.secret_key, &fixture.secret_key)
            .await;
        assert!(
            matches!(&result, Err(AppError::Forbidden(message)) if message.contains("not active")),
            "a revoked service principal must fail closed with Forbidden, got: {result:?}"
        );
        Ok(())
    }

    /// A disabled SP (status != active) must likewise not export the secret.
    #[tokio::test]
    #[ignore = "requires postgres (make test)"]
    async fn refuses_a_disabled_service_principal()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let fixture = seed_dispatch_fixture().await?;
        sqlx::query(
            "UPDATE playbook_service_principal SET status = 'disabled', revoked_at = NULL \
             WHERE id = $1",
        )
        .bind(fixture.service_principal_id)
        .execute(&fixture.state.db_pool)
        .await?;

        let result = fixture
            .resolve(&fixture.secret_key, &fixture.secret_key)
            .await;
        assert!(
            matches!(result, Err(AppError::Forbidden(_))),
            "a disabled service principal must fail closed, got: {result:?}"
        );
        Ok(())
    }

    /// Snapshot drift: the version now declares a different key than the
    /// author-time snapshot authorized for the same env name -> fail closed.
    #[tokio::test]
    #[ignore = "requires postgres (make test)"]
    async fn refuses_a_snapshot_key_mismatch()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let fixture = seed_dispatch_fixture().await?;
        let result = fixture
            .resolve("pwa62-different-key", &fixture.secret_key)
            .await;
        assert!(
            matches!(&result, Err(AppError::Forbidden(message)) if message.contains("different secret")),
            "snapshot key drift must fail closed with Forbidden, got: {result:?}"
        );
        Ok(())
    }

    /// A declared env name absent from the snapshot (e.g. an old run group
    /// authored before the gate existed) -> fail closed.
    #[tokio::test]
    #[ignore = "requires postgres (make test)"]
    async fn refuses_an_env_absent_from_the_snapshot()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let fixture = seed_dispatch_fixture().await?;
        let declared = BTreeMap::from([(SECRET_ENV_NAME.to_string(), SECRET_KEY.to_string())]);
        let result = resolve_runtime_secret_env(
            &fixture.state,
            Uuid::now_v7(),
            fixture.target_id,
            &declared,
            &BTreeMap::new(),
            Some(SecretRecheckSubject::ServicePrincipal(
                fixture.service_principal_id,
            )),
        )
        .await;
        assert!(
            matches!(&result, Err(AppError::Forbidden(message)) if message.contains("was not authorized")),
            "a declared env absent from the snapshot must fail closed, got: {result:?}"
        );
        Ok(())
    }

    struct DispatchFixture {
        state: piping_backend::app::AppState,
        target_id: Uuid,
        service_principal_id: Uuid,
        secret_id: Uuid,
        secret_key: String,
        secret_value: String,
    }

    impl DispatchFixture {
        async fn resolve(
            &self,
            declared_key: &str,
            authorized_key: &str,
        ) -> Result<BTreeMap<String, String>, AppError> {
            let declared =
                BTreeMap::from([(SECRET_ENV_NAME.to_string(), declared_key.to_string())]);
            let authorized = BTreeMap::from([(
                SECRET_ENV_NAME.to_string(),
                (self.secret_id, authorized_key.to_string()),
            )]);
            resolve_runtime_secret_env(
                &self.state,
                Uuid::now_v7(),
                self.target_id,
                &declared,
                &authorized,
                Some(SecretRecheckSubject::ServicePrincipal(
                    self.service_principal_id,
                )),
            )
            .await
        }
    }

    async fn seed_dispatch_fixture()
    -> Result<DispatchFixture, Box<dyn std::error::Error + Send + Sync>> {
        let state = common::build_test_app_state().await?;
        let suffix = common::unique_suffix();
        let (target_id, _) = common::seed_device(&state.db_pool, "pwa62-dispatch").await?;

        let repo = SqlxPlaybookRepository::new(state.db_pool.clone());
        let service_principal = repo
            .create_service_principal(CreateServicePrincipalRecord {
                name: format!("PWA-62 Dispatch {suffix}"),
                slug: format!("pwa62-dispatch-{suffix}"),
                description: None,
                metadata: json!({}),
                user_id: None,
            })
            .await?;
        let service_principal_id = service_principal.id;

        let secret_id = Uuid::now_v7();
        let secret_key = format!("pwa62-dispatch-key-{suffix}");
        sqlx::query(
            "INSERT INTO secrets_variable (id, key, is_masked, is_protected) \
             VALUES ($1, $2, true, false)",
        )
        .bind(secret_id)
        .bind(&secret_key)
        .execute(&state.db_pool)
        .await?;
        sqlx::query(
            "INSERT INTO secrets_binding (secret_id, scope_type, scope_id, weight) \
             VALUES ($1, 'connection_target', $2, 0)",
        )
        .bind(secret_id)
        .bind(target_id)
        .execute(&state.db_pool)
        .await?;
        let secret_value = format!("pwa62-secret-value-{suffix}");
        state
            .services
            .secret_store
            .set_secret(secret_id, &secret_value)
            .await?;

        insert_allow_policy(
            &state.db_pool,
            &format!("service_principal:{service_principal_id}"),
            "EXECUTE",
            "SECRET_VARIABLE",
            vec![secret_id.to_string()],
        )
        .await?;

        Ok(DispatchFixture {
            state,
            target_id,
            service_principal_id,
            secret_id,
            secret_key,
            secret_value,
        })
    }
}
