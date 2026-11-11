use sqlx::PgPool;
use sqlx::Row;

use crate::contexts::config::domain::MonitoringSdLabelGroups;
use crate::errors::AppError;

/// Reads and upserts the org-wide monitoring SD label-group toggle singleton
/// (`monitoring_sd_label_settings`, `id = true`). Mirrors the
/// `SqlxInstanceBrandingRepository` singleton pattern.
#[derive(Clone)]
pub struct SqlxMonitoringSdLabelSettingsRepository {
    pool: PgPool,
}

impl SqlxMonitoringSdLabelSettingsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Loads the label-group settings. The row is seeded by migration
    /// `0033_init_monitoring_sd_label_settings.sql`, so it is expected to always
    /// exist; if it is somehow absent, the documented defaults are returned.
    pub async fn get(&self) -> Result<MonitoringSdLabelGroups, AppError> {
        let row = sqlx::query(
            r#"
            SELECT groups
            FROM monitoring_sd_label_settings
            WHERE id = true
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to load monitoring SD label settings: {error}"
            ))
        })?;

        let groups = row
            .map(|row| row.try_get("groups").map_err(AppError::DatabaseSource))
            .transpose()?;
        decode_settings(groups)
    }

    /// Upserts the label-group settings singleton, refreshing `updated_at`.
    pub async fn upsert(
        &self,
        groups: MonitoringSdLabelGroups,
    ) -> Result<MonitoringSdLabelGroups, AppError> {
        let payload = serde_json::to_value(normalize_settings(groups)).map_err(|error| {
            AppError::Internal(format!("Failed to serialize SD groups: {error}"))
        })?;

        let row = sqlx::query(
            r#"
            INSERT INTO monitoring_sd_label_settings (id, groups, updated_at)
            VALUES (true, $1, NOW())
            ON CONFLICT (id) DO UPDATE
            SET groups = EXCLUDED.groups,
                updated_at = NOW()
            RETURNING groups
            "#,
        )
        .bind(payload)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to upsert monitoring SD label settings: {error}"
            ))
        })?;

        let stored: serde_json::Value = row.try_get("groups").map_err(AppError::DatabaseSource)?;
        decode_settings(Some(stored))
    }
}

fn decode_settings(stored: Option<serde_json::Value>) -> Result<MonitoringSdLabelGroups, AppError> {
    match stored {
        Some(stored) => serde_json::from_value(stored)
            .map(normalize_settings)
            .map_err(|error| {
                AppError::Database(format!(
                    "Invalid monitoring SD label settings payload: {error}"
                ))
            }),
        None => Ok(MonitoringSdLabelGroups::default()),
    }
}

fn normalize_settings(mut groups: MonitoringSdLabelGroups) -> MonitoringSdLabelGroups {
    groups.device_statuses = normalize_statuses(groups.device_statuses);
    groups.site_statuses = normalize_statuses(groups.site_statuses);
    groups
}

fn normalize_statuses(statuses: Vec<String>) -> Vec<String> {
    let mut normalized = statuses
        .into_iter()
        .map(|status| status.trim().to_ascii_lowercase())
        .filter(|status| !status.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    if normalized.is_empty() {
        vec![String::from("active")]
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{decode_settings, normalize_statuses};
    use crate::contexts::config::domain::MonitoringSdLabelGroups;

    #[test]
    fn missing_settings_default_but_corrupt_settings_are_an_error() {
        assert_eq!(
            decode_settings(None).expect("an absent singleton uses documented defaults"),
            MonitoringSdLabelGroups::default()
        );
        assert!(
            decode_settings(Some(json!({"location": true}))).is_err(),
            "a malformed stored singleton must not be replaced with defaults"
        );
    }

    #[test]
    fn normalize_statuses_defaults_when_empty() {
        assert_eq!(normalize_statuses(Vec::new()), vec![String::from("active")]);
        assert_eq!(
            normalize_statuses(vec![String::from(" "), String::from("\t")]),
            vec![String::from("active")]
        );
    }

    #[test]
    fn normalize_statuses_trims_lowers_sorts_and_deduplicates() {
        assert_eq!(
            normalize_statuses(vec![
                String::from(" Offline "),
                String::from("active"),
                String::from("OFFLINE"),
                String::from("planned"),
            ]),
            vec![
                String::from("active"),
                String::from("offline"),
                String::from("planned"),
            ]
        );
    }
}
