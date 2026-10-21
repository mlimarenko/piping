use sqlx::PgPool;

use crate::contexts::config::domain::{InstanceBranding, InstanceBrandingUpdate};
use crate::errors::AppError;

/// Reads and upserts the org-wide branding singleton (`instance_branding`,
/// `id = 1`). Mirrors the `auth_oidc_settings` singleton pattern.
#[derive(Clone)]
pub struct SqlxInstanceBrandingRepository {
    pool: PgPool,
}

impl SqlxInstanceBrandingRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Loads the branding singleton. The row is seeded by migration
    /// `0008_init_instance_branding.sql`, so it is expected to always exist.
    pub async fn get(&self) -> Result<Option<InstanceBranding>, AppError> {
        sqlx::query_as::<_, InstanceBranding>(
            r#"
            SELECT app_name,
                   logo_data_url,
                   favicon_data_url,
                   theme,
                   updated_at,
                   updated_by
            FROM instance_branding
            WHERE id = 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to load instance branding: {error}")))
    }

    /// Upserts the branding singleton, refreshing `updated_at` and `updated_by`.
    pub async fn upsert(
        &self,
        update: InstanceBrandingUpdate,
    ) -> Result<InstanceBranding, AppError> {
        sqlx::query_as::<_, InstanceBranding>(
            r#"
            INSERT INTO instance_branding (
                id,
                app_name,
                logo_data_url,
                favicon_data_url,
                theme,
                updated_at,
                updated_by
            )
            VALUES (1, $1, $2, $3, $4, NOW(), $5)
            ON CONFLICT (id) DO UPDATE
            SET app_name = EXCLUDED.app_name,
                logo_data_url = EXCLUDED.logo_data_url,
                favicon_data_url = EXCLUDED.favicon_data_url,
                theme = EXCLUDED.theme,
                updated_at = NOW(),
                updated_by = EXCLUDED.updated_by
            RETURNING app_name,
                      logo_data_url,
                      favicon_data_url,
                      theme,
                      updated_at,
                      updated_by
            "#,
        )
        .bind(&update.app_name)
        .bind(&update.logo_data_url)
        .bind(&update.favicon_data_url)
        .bind(&update.theme)
        .bind(update.updated_by)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to upsert instance branding: {error}")))
    }
}
