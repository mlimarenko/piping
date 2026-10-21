use async_trait::async_trait;
use sqlx::{PgPool, Row};

use super::repository::OidcSettingsRepository;
use crate::contexts::access_control::domain::{OidcProvisioningMode, OidcSettings};
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct SqlxOidcSettingsRepository {
    pool: PgPool,
}

impl SqlxOidcSettingsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OidcSettingsRepository for SqlxOidcSettingsRepository {
    async fn get_settings(&self) -> Result<Option<OidcSettings>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT
                enabled,
                issuer_url,
                client_id,
                redirect_url,
                scopes,
                group_claim,
                provisioning_mode
            FROM auth_oidc_settings
            WHERE id = 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to load OIDC settings from database: {}",
                error
            ))
        })?;

        let Some(row) = row else {
            return Ok(None);
        };

        let provisioning_mode_raw: String = row.try_get("provisioning_mode").map_err(|error| {
            AppError::Database(format!("Failed to read OIDC provisioning mode: {}", error))
        })?;

        let settings = OidcSettings {
            enabled: row.try_get("enabled").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC enabled flag: {}", error))
            })?,
            issuer_url: row.try_get("issuer_url").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC issuer URL: {}", error))
            })?,
            client_id: row.try_get("client_id").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC client ID: {}", error))
            })?,
            redirect_url: row.try_get("redirect_url").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC redirect URL: {}", error))
            })?,
            scopes: row.try_get("scopes").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC scopes: {}", error))
            })?,
            group_claim: row.try_get("group_claim").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC group claim: {}", error))
            })?,
            provisioning_mode: OidcProvisioningMode::parse(&provisioning_mode_raw)?,
        };

        Ok(Some(settings))
    }

    async fn upsert_settings(&self, settings: OidcSettings) -> Result<OidcSettings, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO auth_oidc_settings (
                id,
                enabled,
                issuer_url,
                client_id,
                redirect_url,
                scopes,
                group_claim,
                provisioning_mode,
                updated_at
            )
            VALUES (1, $1, $2, $3, $4, $5, $6, $7, NOW())
            ON CONFLICT (id) DO UPDATE
            SET enabled = EXCLUDED.enabled,
                issuer_url = EXCLUDED.issuer_url,
                client_id = EXCLUDED.client_id,
                redirect_url = EXCLUDED.redirect_url,
                scopes = EXCLUDED.scopes,
                group_claim = EXCLUDED.group_claim,
                provisioning_mode = EXCLUDED.provisioning_mode,
                updated_at = NOW()
            RETURNING
                enabled,
                issuer_url,
                client_id,
                redirect_url,
                scopes,
                group_claim,
                provisioning_mode
            "#,
        )
        .bind(settings.enabled)
        .bind(&settings.issuer_url)
        .bind(&settings.client_id)
        .bind(&settings.redirect_url)
        .bind(&settings.scopes)
        .bind(&settings.group_claim)
        .bind(settings.provisioning_mode.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to upsert OIDC settings in database: {}",
                error
            ))
        })?;

        let provisioning_mode_raw: String = row.try_get("provisioning_mode").map_err(|error| {
            AppError::Database(format!("Failed to read OIDC provisioning mode: {}", error))
        })?;

        Ok(OidcSettings {
            enabled: row.try_get("enabled").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC enabled flag: {}", error))
            })?,
            issuer_url: row.try_get("issuer_url").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC issuer URL: {}", error))
            })?,
            client_id: row.try_get("client_id").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC client ID: {}", error))
            })?,
            redirect_url: row.try_get("redirect_url").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC redirect URL: {}", error))
            })?,
            scopes: row.try_get("scopes").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC scopes: {}", error))
            })?,
            group_claim: row.try_get("group_claim").map_err(|error| {
                AppError::Database(format!("Failed to read OIDC group claim: {}", error))
            })?,
            provisioning_mode: OidcProvisioningMode::parse(&provisioning_mode_raw)?,
        })
    }
}
