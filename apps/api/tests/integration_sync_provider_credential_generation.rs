//! PIP-606 credential generation for NetBox sources.
//!
//! One locked secret-engine read pairs the token with the revision of the row it
//! was decrypted from, and the commit gate refuses a credential that rotated,
//! cleared, rebound, disabled or re-pointed since the client was built.
//!
//! Every case creates its own `sync_provider` row under a random UUID and deletes
//! it again; nothing here reads or writes a foreign provider. Run against a
//! disposable database only, like the other `#[ignore]` suites:
//!
//! `cargo test -p piping-backend --test integration_sync_provider_credential_generation -- --ignored --test-threads=1`

mod common;

use std::error::Error;
use std::sync::Arc;

use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use piping_backend::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentService,
};
use piping_backend::contexts::secrets::infrastructure::{SecretRepository, SqlxSecretRepository};
use piping_backend::contexts::sync::application::netbox_provider_config::{
    ProviderGenerationBinding, ProviderSecretResolver, ResolvedNetboxSource, resolve_netbox_source,
    revalidate_provider_generation,
};
use piping_backend::contexts::sync::domain::{
    Provider, ProviderType, SYNC_PROVIDER_CONSUMER_TYPE, SYNC_PROVIDER_FIELD_KEY_API_TOKEN,
    SYNC_PROVIDER_OWNER_SCOPE_TYPE,
};
use piping_backend::contexts::sync::infrastructure::{ProviderRepository, SqlxProviderRepository};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

const BASE_URL: &str = "https://netbox-credential-generation.invalid";
const OTHER_BASE_URL: &str = "https://netbox-credential-generation-other.invalid";
const TOKEN_ONE: &str = "pip606-token-one-0123456789";
const TOKEN_TWO: &str = "pip606-token-two-9876543210";

fn configure_cipher() {
    let mut settings = piping_backend::config::load_settings().expect("test settings");
    common::configure_test_cipher(&mut settings);
    piping_backend::contexts::secrets::infrastructure::crypto::configure_cipher_key_material_ref(
        &settings.secrets.cipher.key_material_ref,
    )
    .expect("test cipher");
}

async fn pool() -> Result<PgPool, Box<dyn Error + Send + Sync>> {
    Ok(PgPoolOptions::new()
        .max_connections(4)
        .connect(&common::database_url())
        .await?)
}

fn provider_config(token: Option<&str>, sync_mode: &str, base_url: &str) -> serde_json::Value {
    match token {
        Some(token) => json!({ "base_url": base_url, "sync_mode": sync_mode, "api_token": token }),
        None => json!({ "base_url": base_url, "sync_mode": sync_mode }),
    }
}

fn new_provider(inline_token: Option<&str>) -> Provider {
    Provider {
        id: Uuid::now_v7(),
        name: format!("pip606-credential-generation-{}", Uuid::now_v7()),
        provider_type: ProviderType::Netbox,
        config: provider_config(inline_token, "bidirectional", BASE_URL),
        is_enabled: true,
    }
}

/// Provider write is the canonical extraction path: the inline token leaves the
/// JSON and becomes a sealed secret binding.
async fn create_sealed_provider(
    providers: &SqlxProviderRepository,
    token: &str,
) -> Result<Provider, Box<dyn Error + Send + Sync>> {
    Ok(providers.create(new_provider(Some(token))).await?)
}

fn field_intent(
    provider: &Provider,
    intent: SecretFieldIntent,
    value: Option<&str>,
) -> ApplySecretFieldIntentInput {
    ApplySecretFieldIntentInput {
        consumer_type: SYNC_PROVIDER_CONSUMER_TYPE.to_string(),
        consumer_id: provider.id,
        owner_scope_type: SYNC_PROVIDER_OWNER_SCOPE_TYPE.to_string(),
        owner_scope_id: provider.id,
        field_key: SYNC_PROVIDER_FIELD_KEY_API_TOKEN.to_string(),
        intent,
        value: value.map(str::to_string),
        secret_id: None,
    }
}

async fn stored_ciphertext(pool: &PgPool, secret_id: Uuid) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT encrypted_value FROM secrets_store WHERE secret_id = $1")
        .bind(secret_id)
        .fetch_one(pool)
        .await
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn is_lock_timeout(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.code().as_deref() == Some("55P03"))
}

struct RepositoryResolver<'a>(&'a SqlxSecretRepository);

impl ProviderSecretResolver for RepositoryResolver<'_> {
    async fn resolve_secret_credential(
        &self,
        provider_id: Uuid,
        field_key: &str,
    ) -> Result<
        Option<piping_backend::contexts::secrets::domain::ResolvedCredential>,
        piping_backend::errors::AppError,
    > {
        self.0
            .resolve_consumer_credential(SYNC_PROVIDER_CONSUMER_TYPE, provider_id, field_key)
            .await
    }
}

async fn resolve(
    provider: &Provider,
    secrets: &SqlxSecretRepository,
) -> Result<ResolvedNetboxSource, Box<dyn Error + Send + Sync>> {
    Ok(resolve_netbox_source(provider, &RepositoryResolver(secrets)).await?)
}

#[tokio::test]
#[ignore] // Isolated fixtures against a disposable PostgreSQL only.
async fn provider_write_seals_the_token_and_revision_matches_its_ciphertext() -> TestResult {
    configure_cipher();
    let pool = pool().await?;
    let providers = SqlxProviderRepository::new(pool.clone());
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));

    let created = create_sealed_provider(&providers, TOKEN_ONE).await?;
    assert!(
        created.config.get("api_token").is_none(),
        "the canonical write path must extract the inline token: {:?}",
        created.config
    );

    let resolved = resolve(&created, &secrets).await?;
    assert_eq!(resolved.config.api_token, TOKEN_ONE);
    let credential = resolved.sealed_credential()?;
    let ciphertext = stored_ciphertext(&pool, credential.secret_id).await?;
    assert_eq!(
        credential.revision,
        sha256_hex(&ciphertext),
        "the revision must be the digest of the very row the value was decrypted from"
    );

    let binding = ProviderGenerationBinding::capture(&created, &resolved)?;
    let mut transaction = pool.begin().await?;
    revalidate_provider_generation(&mut transaction, &providers, secrets.as_ref(), &binding)
        .await?;
    transaction.commit().await?;

    providers.delete(created.id).await?;
    Ok(())
}

#[tokio::test]
#[ignore]
async fn rotation_never_pairs_the_old_token_with_the_new_revision() -> TestResult {
    configure_cipher();
    let pool = pool().await?;
    let providers = SqlxProviderRepository::new(pool.clone());
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let intents = SecretFieldIntentService::new(secrets.clone());

    let created = create_sealed_provider(&providers, TOKEN_ONE).await?;
    let before = resolve(&created, &secrets).await?;
    let before_credential = before.sealed_credential()?.clone();

    intents
        .apply_intent(field_intent(
            &created,
            SecretFieldIntent::Set,
            Some(TOKEN_TWO),
        ))
        .await?;

    let after = resolve(&created, &secrets).await?;
    let after_credential = after.sealed_credential()?.clone();
    let after_ciphertext = stored_ciphertext(&pool, after_credential.secret_id).await?;
    assert_eq!(after.config.api_token, TOKEN_TWO);
    assert_eq!(
        after_credential.revision,
        sha256_hex(&after_ciphertext),
        "the rotated value must carry the revision of its own ciphertext"
    );
    assert_eq!(
        after_credential.secret_id, before_credential.secret_id,
        "a value rotation keeps the secret identity"
    );
    assert_ne!(
        after_credential.revision, before_credential.revision,
        "a rotation must change the revision"
    );

    let mut transaction = pool.begin().await?;
    let stale = secrets
        .revalidate_consumer_credential(
            &mut transaction,
            SYNC_PROVIDER_CONSUMER_TYPE,
            created.id,
            SYNC_PROVIDER_FIELD_KEY_API_TOKEN,
            before_credential.secret_id,
            &before_credential.revision,
        )
        .await;
    transaction.rollback().await?;
    assert!(
        stale.is_err(),
        "the pre-rotation revision must never be accepted after a rotation"
    );

    let mut transaction = pool.begin().await?;
    secrets
        .revalidate_consumer_credential(
            &mut transaction,
            SYNC_PROVIDER_CONSUMER_TYPE,
            created.id,
            SYNC_PROVIDER_FIELD_KEY_API_TOKEN,
            after_credential.secret_id,
            &after_credential.revision,
        )
        .await?;
    transaction.commit().await?;

    providers.delete(created.id).await?;
    Ok(())
}

#[tokio::test]
#[ignore]
async fn clear_and_rebind_refuse_the_commit_gate() -> TestResult {
    configure_cipher();
    let pool = pool().await?;
    let providers = SqlxProviderRepository::new(pool.clone());
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));
    let intents = SecretFieldIntentService::new(secrets.clone());

    let created = create_sealed_provider(&providers, TOKEN_ONE).await?;
    let resolved = resolve(&created, &secrets).await?;
    let binding = ProviderGenerationBinding::capture(&created, &resolved)?;

    intents
        .apply_intent(field_intent(&created, SecretFieldIntent::Clear, None))
        .await?;
    let mut transaction = pool.begin().await?;
    let after_clear =
        revalidate_provider_generation(&mut transaction, &providers, secrets.as_ref(), &binding)
            .await;
    transaction.rollback().await?;
    assert!(after_clear.is_err(), "a cleared credential must refuse");

    intents
        .apply_intent(field_intent(
            &created,
            SecretFieldIntent::Set,
            Some(TOKEN_TWO),
        ))
        .await?;
    let rebound = resolve(&created, &secrets).await?;
    let rebound_credential = rebound.sealed_credential()?.clone();
    assert_ne!(
        rebound_credential.secret_id, binding.credential.secret_id,
        "clear plus set must rebind the field to a new secret row"
    );

    let mut transaction = pool.begin().await?;
    let after_rebind =
        revalidate_provider_generation(&mut transaction, &providers, secrets.as_ref(), &binding)
            .await;
    transaction.rollback().await?;
    assert!(
        after_rebind.is_err(),
        "a rebind to another secret must refuse the previous generation"
    );

    providers.delete(created.id).await?;
    Ok(())
}

#[tokio::test]
#[ignore]
async fn an_unchanged_read_and_accounting_timestamps_keep_the_generation() -> TestResult {
    configure_cipher();
    let pool = pool().await?;
    let providers = SqlxProviderRepository::new(pool.clone());
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));

    let created = create_sealed_provider(&providers, TOKEN_ONE).await?;
    let first = resolve(&created, &secrets).await?;
    let binding = ProviderGenerationBinding::capture(&created, &first)?;
    let second = resolve(&created, &secrets).await?;
    assert_eq!(
        binding.credential.revision,
        second.sealed_credential()?.revision,
        "an ordinary read must preserve the revision"
    );

    sqlx::query(
        "UPDATE secret_usage_binding SET updated_at = now() \
         WHERE consumer_type = $1 AND consumer_id = $2 AND field_key = $3",
    )
    .bind(SYNC_PROVIDER_CONSUMER_TYPE)
    .bind(created.id)
    .bind(SYNC_PROVIDER_FIELD_KEY_API_TOKEN)
    .execute(&pool)
    .await?;

    let mut transaction = pool.begin().await?;
    revalidate_provider_generation(&mut transaction, &providers, secrets.as_ref(), &binding)
        .await?;
    transaction.commit().await?;

    providers.delete(created.id).await?;
    Ok(())
}

#[tokio::test]
#[ignore]
async fn binding_and_store_locks_hold_the_gate_until_commit() -> TestResult {
    configure_cipher();
    let pool = pool().await?;
    let providers = SqlxProviderRepository::new(pool.clone());
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));

    let created = create_sealed_provider(&providers, TOKEN_ONE).await?;
    let resolved = resolve(&created, &secrets).await?;
    let binding = ProviderGenerationBinding::capture(&created, &resolved)?;

    let mut gate = pool.begin().await?;
    revalidate_provider_generation(&mut gate, &providers, secrets.as_ref(), &binding).await?;

    // A clear cannot retire the binding while the gate transaction holds it.
    let mut rival = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *rival)
        .await?;
    let clear = sqlx::query(
        "DELETE FROM secret_usage_binding \
         WHERE consumer_type = $1 AND consumer_id = $2 AND field_key = $3",
    )
    .bind(SYNC_PROVIDER_CONSUMER_TYPE)
    .bind(created.id)
    .bind(SYNC_PROVIDER_FIELD_KEY_API_TOKEN)
    .execute(&mut *rival)
    .await;
    assert!(
        clear.as_ref().err().is_some_and(is_lock_timeout),
        "the binding must be share-locked by the gate transaction: {clear:?}"
    );
    rival.rollback().await?;

    // A rotation cannot overwrite the ciphertext either.
    let mut rival = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *rival)
        .await?;
    let rotate = sqlx::query(
        "UPDATE secrets_store SET encrypted_value = encrypted_value WHERE secret_id = $1",
    )
    .bind(binding.credential.secret_id)
    .execute(&mut *rival)
    .await;
    assert!(
        rotate.as_ref().err().is_some_and(is_lock_timeout),
        "the stored ciphertext must be share-locked by the gate transaction: {rotate:?}"
    );
    rival.rollback().await?;

    gate.commit().await?;

    // Once the gate committed, the same clear lands and the next check refuses.
    let cleared = sqlx::query(
        "DELETE FROM secret_usage_binding \
         WHERE consumer_type = $1 AND consumer_id = $2 AND field_key = $3",
    )
    .bind(SYNC_PROVIDER_CONSUMER_TYPE)
    .bind(created.id)
    .bind(SYNC_PROVIDER_FIELD_KEY_API_TOKEN)
    .execute(&pool)
    .await?;
    assert_eq!(cleared.rows_affected(), 1);

    let mut transaction = pool.begin().await?;
    let after_clear =
        revalidate_provider_generation(&mut transaction, &providers, secrets.as_ref(), &binding)
            .await;
    transaction.rollback().await?;
    assert!(after_clear.is_err(), "a cleared binding must refuse");

    // Restore the canonical binding so `delete` retires this fixture's secret
    // through the normal detach path instead of leaving rows behind.
    sqlx::query(
        "INSERT INTO secret_usage_binding \
         (id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(SYNC_PROVIDER_CONSUMER_TYPE)
    .bind(created.id)
    .bind(SYNC_PROVIDER_FIELD_KEY_API_TOKEN)
    .bind(SYNC_PROVIDER_OWNER_SCOPE_TYPE)
    .bind(created.id)
    .bind(binding.credential.secret_id)
    .execute(&pool)
    .await?;

    providers.delete(created.id).await?;
    Ok(())
}

#[tokio::test]
#[ignore]
async fn disable_and_endpoint_or_mode_change_refuse_the_generation() -> TestResult {
    configure_cipher();
    let pool = pool().await?;
    let providers = SqlxProviderRepository::new(pool.clone());
    let secrets = Arc::new(SqlxSecretRepository::new(pool.clone()));

    let created = create_sealed_provider(&providers, TOKEN_ONE).await?;
    let resolved = resolve(&created, &secrets).await?;
    let binding = ProviderGenerationBinding::capture(&created, &resolved)?;

    providers.set_enabled(created.id, false).await?;
    let mut transaction = pool.begin().await?;
    let disabled =
        revalidate_provider_generation(&mut transaction, &providers, secrets.as_ref(), &binding)
            .await;
    transaction.rollback().await?;
    assert!(disabled.is_err(), "a disabled source must refuse");

    let mut reenabled = created.clone();
    reenabled.is_enabled = true;
    reenabled.config = provider_config(None, "bidirectional", OTHER_BASE_URL);
    let moved = providers.update(reenabled).await?;
    let mut transaction = pool.begin().await?;
    let endpoint =
        revalidate_provider_generation(&mut transaction, &providers, secrets.as_ref(), &binding)
            .await;
    transaction.rollback().await?;
    assert!(endpoint.is_err(), "a moved endpoint must refuse");

    let mut pull_only = moved;
    pull_only.config = provider_config(None, "pull_only", BASE_URL);
    providers.update(pull_only).await?;
    let mut transaction = pool.begin().await?;
    let mode =
        revalidate_provider_generation(&mut transaction, &providers, secrets.as_ref(), &binding)
            .await;
    transaction.rollback().await?;
    assert!(mode.is_err(), "a mode change to pull_only must refuse");

    providers.delete(created.id).await?;
    Ok(())
}
