use async_trait::async_trait;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::contexts::secrets::application::SecretStore;
use crate::contexts::secrets::infrastructure::crypto::{
    decrypt_secret, encrypt_secret, is_encrypted_secret_envelope,
};
use crate::errors::AppError;

#[derive(Clone)]
pub struct DbSecretStore {
    pool: PgPool,
}

impl DbSecretStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub(crate) fn decode_stored_secret(encrypted_value: &str) -> Result<String, AppError> {
        if is_encrypted_secret_envelope(encrypted_value) {
            return decrypt_secret(encrypted_value);
        }
        Err(AppError::Validation(
            "Secret payload is not encrypted with the canonical envelope format".to_string(),
        ))
    }
}

#[async_trait]
impl SecretStore for DbSecretStore {
    async fn get_secret(&self, secret_id: Uuid) -> Result<String, AppError> {
        let row = sqlx::query(
            r#"
            SELECT encrypted_value
            FROM secrets_store
            WHERE secret_id = $1
            "#,
        )
        .bind(secret_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to get secret: {}", e)))?;

        let Some(row) = row else {
            return Err(AppError::MissingSecret(format!(
                "Secret not found: {}",
                secret_id
            )));
        };

        let encrypted: String = row
            .try_get("encrypted_value")
            .map_err(|e| AppError::Database(format!("Failed to read secret payload: {}", e)))?;
        Self::decode_stored_secret(&encrypted)
    }

    async fn set_secret(&self, secret_id: Uuid, value: &str) -> Result<(), AppError> {
        let encrypted = encrypt_secret(value)?;
        sqlx::query(
            r#"
            INSERT INTO secrets_store (secret_id, encrypted_value)
            VALUES ($1, $2)
            ON CONFLICT (secret_id)
            DO UPDATE SET encrypted_value = EXCLUDED.encrypted_value
            "#,
        )
        .bind(secret_id)
        .bind(encrypted)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to set secret: {}", e)))?;
        Ok(())
    }

    async fn delete_secret(&self, secret_id: Uuid) -> Result<(), AppError> {
        sqlx::query(
            r#"
            DELETE FROM secrets_store
            WHERE secret_id = $1
            "#,
        )
        .bind(secret_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to delete secret: {}", e)))?;
        Ok(())
    }

    async fn ensure_unique_key(
        &self,
        _scope_type: &str,
        _scope_id: Uuid,
        _key: &str,
    ) -> Result<(), AppError> {
        let _ = &self.pool;
        Ok(())
    }
}
