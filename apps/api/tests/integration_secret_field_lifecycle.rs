//! Native transactional field reference/retirement proof on the isolated test database.

mod common;

use std::sync::Arc;
use std::time::Duration;
use piping_backend::contexts::connections::infrastructure::ConnectionRepository;
use piping_backend::contexts::connections::infrastructure::persistence::repositories::sqlx_connection_repository::SqlxConnectionRepository;

use piping_backend::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentService, SecretStore,
};
use piping_backend::contexts::secrets::domain::{SecretBinding, SecretVariable};
use piping_backend::contexts::secrets::infrastructure::{
    DbSecretStore, SecretRepository, SqlxSecretRepository,
};
use sqlx::PgPool;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn proxy(pool: &PgPool) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO connections_proxy_node (id, name, slug, base_url) VALUES ($1, $2, $2, 'https://synthetic-lifecycle.invalid')")
        .bind(id).bind(format!("lifecycle-{id}")).execute(pool).await?;
    Ok(id)
}

fn command(
    owner: Uuid,
    field: &str,
    intent: SecretFieldIntent,
    secret_id: Option<Uuid>,
) -> ApplySecretFieldIntentInput {
    ApplySecretFieldIntentInput {
        consumer_type: "proxy_node:credential".into(),
        consumer_id: owner,
        owner_scope_type: "proxy_node".into(),
        owner_scope_id: owner,
        field_key: field.into(),
        intent,
        secret_id,
        value: (intent != SecretFieldIntent::Clear && secret_id.is_none())
            .then(|| "synthetic-field-value".into()),
    }
}

async fn count(pool: &PgPool, table: &str, secret_id: Uuid) -> Result<i64, sqlx::Error> {
    // The table is selected solely by the fixed test-owned callers below.
    let column = if table == "secrets_variable" {
        "id"
    } else {
        "secret_id"
    };
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {column} = $1"))
        .bind(secret_id)
        .fetch_one(pool)
        .await
}

async fn gone(pool: &PgPool, secret_id: Uuid) -> TestResult {
    for table in [
        "secrets_variable",
        "secrets_store",
        "secrets_binding",
        "secret_usage_binding",
    ] {
        assert_eq!(
            count(pool, table, secret_id).await?,
            0,
            "retired field remains in {table}"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore] // Requires the explicitly isolated PostgreSQL/Redis stack; common resets that database.
async fn managed_shared_borrowed_named_and_protected_retirement_is_transactional() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = &state.db_pool;
    let repository = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let service = SecretFieldIntentService::new(repository.clone());
    let store = DbSecretStore::new(pool.clone());
    let owner = proxy(pool).await?;
    let foreign_owner = proxy(pool).await?;

    let first = service
        .apply_intent(command(owner, "first", SecretFieldIntent::Set, None))
        .await?;
    let secret_id = first.secret_id.expect("managed secret");
    assert!(
        repository
            .get_variable_by_id(secret_id)
            .await?
            .expect("variable")
            .retire_with_field_usage
    );
    assert!(repository.list_bindings_for_secret(secret_id).await?[0].managed_by_field_usage);
    service
        .apply_intent(command(
            owner,
            "second",
            SecretFieldIntent::Set,
            Some(secret_id),
        ))
        .await?;
    service
        .apply_intent(command(owner, "first", SecretFieldIntent::Clear, None))
        .await?;
    assert_eq!(count(pool, "secret_usage_binding", secret_id).await?, 1);
    assert_eq!(count(pool, "secrets_store", secret_id).await?, 1);
    assert_eq!(count(pool, "secrets_binding", secret_id).await?, 1);
    service
        .apply_intent(command(owner, "second", SecretFieldIntent::Clear, None))
        .await?;
    gone(pool, secret_id).await?;

    let named_key = format!("lifecycle-named-{}", Uuid::now_v7());
    let named = repository.create_named(&named_key, None, None).await?;
    store.set_secret(named.id, "synthetic-named-value").await?;
    service
        .apply_intent(command(
            owner,
            "named",
            SecretFieldIntent::Set,
            Some(named.id),
        ))
        .await?;
    assert!(matches!(
        repository.delete_named(named.id).await,
        Err(piping_backend::errors::AppError::Conflict(_))
    ));
    service
        .apply_intent(command(owner, "named", SecretFieldIntent::Clear, None))
        .await?;
    assert_eq!(count(pool, "secrets_variable", named.id).await?, 1);
    assert_eq!(count(pool, "secrets_store", named.id).await?, 1);
    assert_eq!(count(pool, "secrets_binding", named.id).await?, 0);
    service
        .apply_intent(command(
            owner,
            "missing",
            SecretFieldIntent::Clear,
            Some(named.id),
        ))
        .await?;
    assert_eq!(
        count(pool, "secrets_store", named.id).await?,
        1,
        "missing Clear must ignore caller UUID"
    );

    let borrowed = repository
        .create_variable(SecretVariable {
            id: Uuid::now_v7(),
            key: format!("secret-field/ambiguous/{}", Uuid::now_v7()),
            kind: "field".into(),
            description: None,
            is_masked: true,
            is_protected: false,
            retire_with_field_usage: true,
        })
        .await?;
    assert!(!borrowed.retire_with_field_usage);
    store
        .set_secret(borrowed.id, "synthetic-borrowed-value")
        .await?;
    service
        .apply_intent(command(
            owner,
            "borrowed",
            SecretFieldIntent::Set,
            Some(borrowed.id),
        ))
        .await?;
    service
        .apply_intent(command(owner, "borrowed", SecretFieldIntent::Clear, None))
        .await?;
    assert_eq!(count(pool, "secrets_binding", borrowed.id).await?, 0);
    assert_eq!(count(pool, "secrets_store", borrowed.id).await?, 1);
    assert_eq!(count(pool, "secrets_variable", borrowed.id).await?, 1);

    let protected = service
        .apply_intent(command(owner, "protected", SecretFieldIntent::Set, None))
        .await?
        .secret_id
        .expect("protected variable");
    let original_binding = repository.list_bindings_for_secret(protected).await?[0].clone();
    let promoted = repository
        .create_binding(SecretBinding {
            id: Uuid::now_v7(),
            secret_id: protected,
            scope_type: "proxy_node".into(),
            scope_id: owner,
            weight: 0,
            managed_by_field_usage: true,
        })
        .await?;
    assert_eq!(
        promoted.id, original_binding.id,
        "protection promotes rather than duplicates"
    );
    assert!(!promoted.managed_by_field_usage);
    service
        .apply_intent(command(owner, "protected", SecretFieldIntent::Clear, None))
        .await?;
    assert_eq!(count(pool, "secrets_variable", protected).await?, 1);
    assert_eq!(count(pool, "secrets_store", protected).await?, 1);
    assert_eq!(count(pool, "secrets_binding", protected).await?, 1);

    let old = service
        .apply_intent(command(owner, "rollback", SecretFieldIntent::Set, None))
        .await?
        .secret_id
        .expect("old variable");
    let foreign = service
        .apply_intent(command(
            foreign_owner,
            "foreign",
            SecretFieldIntent::Set,
            None,
        ))
        .await?
        .secret_id
        .expect("foreign variable");
    let before: String =
        sqlx::query_scalar("SELECT encrypted_value FROM secrets_store WHERE secret_id = $1")
            .bind(old)
            .fetch_one(pool)
            .await?;
    for replacement in [Uuid::now_v7(), foreign] {
        assert!(
            service
                .apply_intent(command(
                    owner,
                    "rollback",
                    SecretFieldIntent::Set,
                    Some(replacement)
                ))
                .await
                .is_err()
        );
        assert_eq!(
            repository
                .get_usage_binding("proxy_node:credential", owner, "rollback")
                .await?
                .expect("original usage")
                .secret_id,
            old
        );
        let after: String =
            sqlx::query_scalar("SELECT encrypted_value FROM secrets_store WHERE secret_id = $1")
                .bind(old)
                .fetch_one(pool)
                .await?;
        assert!(before == after, "failed rebind changed ciphertext");
    }
    let mut invalid_owner = command(owner, "rollback", SecretFieldIntent::Clear, None);
    invalid_owner.owner_scope_id = foreign_owner;
    assert!(service.apply_intent(invalid_owner).await.is_err());
    assert_eq!(count(pool, "secret_usage_binding", old).await?, 1);
    service
        .apply_intent(command(
            owner,
            "rollback",
            SecretFieldIntent::Set,
            Some(named.id),
        ))
        .await?;
    gone(pool, old).await?;
    assert_eq!(count(pool, "secrets_store", foreign).await?, 1);
    let long_fields = [
        format!("{}a", "x".repeat(512)),
        format!("{}b", "x".repeat(512)),
    ];
    let mut long_secrets = Vec::new();
    for field in &long_fields {
        long_secrets.push(
            service
                .apply_intent(command(owner, field, SecretFieldIntent::Set, None))
                .await?
                .secret_id
                .expect("long-path field"),
        );
    }
    assert_ne!(
        long_secrets[0], long_secrets[1],
        "truncated display keys must not collide"
    );
    for (field, secret_id) in long_fields.iter().zip(long_secrets) {
        service
            .apply_intent(command(owner, field, SecretFieldIntent::Clear, None))
            .await?;
        gone(pool, secret_id).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore] // Uses only the isolated native stack, barriers and bounded completion; no sleeps.
async fn proxy_blocked_delete_and_field_races_preserve_atomic_retirement() -> TestResult {
    let state = common::build_test_app_state().await?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&common::database_url())
        .await?;
    let repository = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let service = Arc::new(SecretFieldIntentService::new(repository.clone()));
    let connections = Arc::new(SqlxConnectionRepository::new(pool.clone()));
    let owner = proxy(&pool).await?;
    let secret_id = service
        .apply_intent(command(owner, "blocked", SecretFieldIntent::Set, None))
        .await?
        .secret_id
        .expect("managed field");
    let assignment = Uuid::now_v7();
    sqlx::query("INSERT INTO connections_proxy_assignment (id, proxy_node_id, scope_type) VALUES ($1, $2, 'global')")
        .bind(assignment).bind(owner).execute(&pool).await?;
    let ciphertext_before: String =
        sqlx::query_scalar("SELECT encrypted_value FROM secrets_store WHERE secret_id = $1")
            .bind(secret_id)
            .fetch_one(&pool)
            .await?;
    assert!(matches!(
        connections.delete_proxy_node(owner).await,
        Err(piping_backend::errors::AppError::DeleteBlocked(_))
    ));
    for table in [
        "secrets_variable",
        "secrets_store",
        "secrets_binding",
        "secret_usage_binding",
    ] {
        assert_eq!(
            count(&pool, table, secret_id).await?,
            1,
            "blocked deletion modified {table}"
        );
    }
    let ciphertext_after: String =
        sqlx::query_scalar("SELECT encrypted_value FROM secrets_store WHERE secret_id = $1")
            .bind(secret_id)
            .fetch_one(&pool)
            .await?;
    assert!(
        ciphertext_before == ciphertext_after,
        "blocked deletion modified ciphertext"
    );
    sqlx::query("DELETE FROM connections_proxy_assignment WHERE id = $1")
        .bind(assignment)
        .execute(&pool)
        .await?;
    assert!(connections.delete_proxy_node(owner).await?);
    gone(&pool, secret_id).await?;
    assert!(
        service
            .apply_intent(command(owner, "blocked", SecretFieldIntent::Set, None))
            .await
            .is_err(),
        "Set cannot resurrect a deleted consumer"
    );

    let owner = proxy(&pool).await?;
    let original = service
        .apply_intent(command(owner, "race", SecretFieldIntent::Set, None))
        .await?
        .secret_id
        .expect("race field");
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let set = {
        let service = service.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            service
                .apply_intent(command(owner, "race", SecretFieldIntent::Set, None))
                .await
        })
    };
    let clear = {
        let service = service.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            service
                .apply_intent(command(owner, "race", SecretFieldIntent::Clear, None))
                .await
        })
    };
    barrier.wait().await;
    let (set, clear) =
        tokio::time::timeout(Duration::from_secs(15), async { tokio::join!(set, clear) }).await?;
    let set = set??;
    clear??;
    let selected = set.secret_id.expect("Set response");
    let live = repository
        .get_usage_binding("proxy_node:credential", owner, "race")
        .await?;
    for id in [original, selected] {
        if live.as_ref().is_some_and(|usage| usage.secret_id == id) {
            for table in [
                "secrets_variable",
                "secrets_store",
                "secrets_binding",
                "secret_usage_binding",
            ] {
                assert_eq!(
                    count(&pool, table, id).await?,
                    1,
                    "live field missing {table}"
                );
            }
        } else {
            gone(&pool, id).await?;
        }
    }

    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let set = {
        let service = service.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            service
                .apply_intent(command(owner, "race", SecretFieldIntent::Set, None))
                .await
        })
    };
    let deletion = {
        let connections = connections.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            connections.delete_proxy_node(owner).await
        })
    };
    barrier.wait().await;
    let (set, deletion) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(set, deletion)
    })
    .await?;
    assert!(deletion??);
    if let Ok(set) = set? {
        gone(&pool, set.secret_id.expect("raced Set")).await?;
    }
    gone(&pool, original).await?;
    gone(&pool, selected).await?;
    assert!(
        repository
            .get_usage_binding("proxy_node:credential", owner, "race")
            .await?
            .is_none()
    );
    pool.close().await;
    drop(state);
    Ok(())
}
