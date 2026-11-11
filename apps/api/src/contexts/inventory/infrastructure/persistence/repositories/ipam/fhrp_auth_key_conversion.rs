//! Offline plaintext retirement before migration 0004. Never called at startup.

use crate::contexts::secrets::application::consumer_type::{
    FHRP_AUTH_KEY_FIELD, INVENTORY_FHRP_GROUP_CONSUMER_TYPE,
};
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentService,
};
use crate::contexts::secrets::infrastructure::{SqlxSecretRepository, crypto::decrypt_secret};
use crate::errors::AppError;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

/// Converts all nonempty legacy values in one transaction, then clears plaintext.
/// The operator must stop every writer and verify a private backup beforehand.
/// No schema or migration-ledger changes are performed here.
pub async fn convert_fhrp_auth_keys_offline(pool: &PgPool) -> Result<usize, AppError> {
    let mut tx = pool.begin().await?;
    crate::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::lock_field_catalog_for_usage(&mut tx).await?;
    // Acquire all locks before observing values or bindings. No online writer may
    // run during this operation; the locks also exclude accidental concurrent writes.
    sqlx::query("LOCK TABLE ipam_fhrpgroup, secret_usage_binding, secrets_binding, secrets_variable, secrets_store IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx).await?;
    let rows =
        sqlx::query("SELECT id, auth_key FROM ipam_fhrpgroup WHERE auth_key <> '' ORDER BY id")
            .fetch_all(&mut *tx)
            .await?;
    let mut inputs = Vec::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let value: String = row.try_get("auth_key")?;
        // Validate the complete cohort before the lifecycle can mutate anything.
        // Whitespace-only legacy keys cannot pass canonical intent validation;
        // refuse rather than silently dropping them or changing their bytes.
        if value.trim().is_empty() || value.chars().count() > 255 {
            return Err(AppError::Validation(
                "Legacy FHRP key cannot be converted without operator correction".into(),
            ));
        }
        let bound: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM secret_usage_binding WHERE consumer_type = $1 AND consumer_id = $2 AND field_key = $3)")
            .bind(INVENTORY_FHRP_GROUP_CONSUMER_TYPE).bind(id).bind(FHRP_AUTH_KEY_FIELD)
            .fetch_one(&mut *tx).await?;
        if bound {
            return Err(AppError::Conflict(
                "Legacy FHRP key already has a managed binding; refusing to overwrite".into(),
            ));
        }
        inputs.push(ApplySecretFieldIntentInput {
            consumer_type: INVENTORY_FHRP_GROUP_CONSUMER_TYPE.into(),
            consumer_id: id,
            owner_scope_type: INVENTORY_FHRP_GROUP_CONSUMER_TYPE.into(),
            owner_scope_id: id,
            field_key: FHRP_AUTH_KEY_FIELD.into(),
            intent: SecretFieldIntent::Set,
            value: Some(value),
            secret_id: None,
        });
    }
    let count = inputs.len();
    let service = SecretFieldIntentService::new(Arc::new(SqlxSecretRepository::new(pool.clone())));
    let states = service
        .apply_intents_in_transaction(&mut tx, inputs.clone())
        .await?;
    if states.len() != inputs.len() {
        return Err(AppError::Internal(
            "Incomplete FHRP conversion batch".into(),
        ));
    }
    for (input, state) in inputs.iter().zip(states) {
        let secret_id = state
            .secret_id
            .ok_or_else(|| AppError::Internal("FHRP conversion did not create a secret".into()))?;
        // Verify the persisted binding AND encrypted bytes using this transaction,
        // not a second pool connection that cannot observe uncommitted writes.
        let encrypted: Option<String> = sqlx::query_scalar("SELECT s.encrypted_value FROM secret_usage_binding u JOIN secrets_store s ON s.secret_id = u.secret_id JOIN secrets_binding b ON b.secret_id = u.secret_id AND b.scope_type = u.owner_scope_type AND b.scope_id = u.owner_scope_id WHERE u.consumer_type = $1 AND u.consumer_id = $2 AND u.field_key = $3 AND u.owner_scope_type = $1 AND u.owner_scope_id = $2 AND u.secret_id = $4")
            .bind(INVENTORY_FHRP_GROUP_CONSUMER_TYPE).bind(input.consumer_id).bind(FHRP_AUTH_KEY_FIELD).bind(secret_id)
            .fetch_optional(&mut *tx).await?;
        let encrypted = encrypted
            .ok_or_else(|| AppError::Internal("FHRP conversion binding/value missing".into()))?;
        if input.value.as_deref() != Some(decrypt_secret(&encrypted)?.as_str()) {
            return Err(AppError::Internal(
                "FHRP conversion did not preserve exact key bytes".into(),
            ));
        }
    }
    // Only after every binding/value has been proven may any plaintext be cleared.
    sqlx::query("UPDATE ipam_fhrpgroup SET auth_key = '' WHERE auth_key <> ''")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(count)
}
