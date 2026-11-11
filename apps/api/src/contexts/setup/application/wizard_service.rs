use chrono::Utc;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use tracing::info;
use uuid::Uuid;

use crate::contexts::setup::domain::{WizardProgress, WizardReadiness, WizardStatus};
use crate::contexts::setup::infrastructure::persistence::WizardProgressRepository;
use crate::errors::AppError;

pub const CONNECTION_SETUP_WIZARD_TYPE: &str = "connection_setup";

#[derive(Clone)]
pub struct WizardService {
    repository: WizardProgressRepository,
    pool: PgPool,
}

impl WizardService {
    pub fn new(repository: WizardProgressRepository, pool: PgPool) -> Self {
        Self { repository, pool }
    }

    pub async fn check_readiness(
        &self,
        user_id: Uuid,
        user_is_admin: bool,
    ) -> Result<WizardReadiness, AppError> {
        let has_hosts = self.has_hosts().await?;
        let has_connection_configs = self.has_connection_configs().await?;
        let user_has_credential_sets = self.user_has_credential_sets(user_id).await?;
        let user_has_connected = self.user_has_connected(user_id).await?;
        let progress = self
            .repository
            .find_by_user_and_type(user_id, CONNECTION_SETUP_WIZARD_TYPE)
            .await?;

        Ok(WizardReadiness {
            has_hosts,
            has_connection_configs,
            user_has_credential_sets,
            user_has_connected,
            wizard_status: progress.as_ref().map(|value| value.status.clone()),
            wizard_current_step: progress.and_then(|value| value.current_step),
            user_is_admin,
        })
    }

    pub async fn get_progress(
        &self,
        user_id: Uuid,
        wizard_type: &str,
    ) -> Result<Option<WizardProgress>, AppError> {
        self.repository
            .find_by_user_and_type(user_id, wizard_type)
            .await
    }

    pub async fn update_progress(
        &self,
        user_id: Uuid,
        wizard_type: &str,
        current_step: &str,
        step_data: Value,
    ) -> Result<WizardProgress, AppError> {
        let normalized_step = normalize_step(current_step)?;
        ensure_step_data_object(&step_data)?;

        self.repository
            .upsert(user_id, wizard_type, Some(&normalized_step), &step_data)
            .await
    }

    pub async fn complete(
        &self,
        user_id: Uuid,
        wizard_type: &str,
    ) -> Result<WizardProgress, AppError> {
        let existing = self.ensure_progress_exists(user_id, wizard_type).await?;
        let completed = self
            .repository
            .update_status(
                user_id,
                wizard_type,
                WizardStatus::Completed.to_string().as_str(),
                Some(Utc::now()),
            )
            .await?;

        info!(
            target: "piping_backend::contexts::setup::wizard_service",
            user_id = %user_id,
            wizard_type,
            step_reached = existing.current_step.as_deref().unwrap_or("none"),
            outcome = "completed",
            "setup wizard lifecycle event"
        );

        Ok(completed)
    }

    pub async fn dismiss(
        &self,
        user_id: Uuid,
        wizard_type: &str,
    ) -> Result<WizardProgress, AppError> {
        let existing = self.ensure_progress_exists(user_id, wizard_type).await?;
        let dismissed = self
            .repository
            .update_status(
                user_id,
                wizard_type,
                WizardStatus::Dismissed.to_string().as_str(),
                None,
            )
            .await?;

        info!(
            target: "piping_backend::contexts::setup::wizard_service",
            user_id = %user_id,
            wizard_type,
            step_reached = existing.current_step.as_deref().unwrap_or("none"),
            outcome = "dismissed",
            "setup wizard lifecycle event"
        );

        Ok(dismissed)
    }

    pub async fn reset(
        &self,
        user_id: Uuid,
        wizard_type: &str,
    ) -> Result<WizardProgress, AppError> {
        let existing = self
            .repository
            .find_by_user_and_type(user_id, wizard_type)
            .await?;

        match existing {
            Some(progress) => {
                self.repository
                    .reset(user_id, wizard_type, progress.status.as_str())
                    .await
            }
            None => {
                let empty = json!({});
                self.repository
                    .upsert(user_id, wizard_type, None, &empty)
                    .await
            }
        }
    }

    async fn ensure_progress_exists(
        &self,
        user_id: Uuid,
        wizard_type: &str,
    ) -> Result<WizardProgress, AppError> {
        match self
            .repository
            .find_by_user_and_type(user_id, wizard_type)
            .await?
        {
            Some(progress) => Ok(progress),
            None => {
                let empty = json!({});
                self.repository
                    .upsert(user_id, wizard_type, None, &empty)
                    .await
            }
        }
    }

    async fn has_hosts(&self) -> Result<bool, AppError> {
        let row = sqlx::query(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM dcim_device
                UNION ALL
                SELECT 1 FROM virtualization_virtualmachine
            ) AS has_hosts
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to evaluate host readiness: {error}"))
        })?;

        row.try_get("has_hosts").map_err(|error| {
            AppError::Database(format!("Failed to read has_hosts readiness flag: {error}"))
        })
    }

    async fn has_connection_configs(&self) -> Result<bool, AppError> {
        let row = sqlx::query(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM config_context
                WHERE is_active = true
                  AND jsonb_typeof(data #> '{piping,connections}') = 'object'
                  AND (data #> '{piping,connections}') <> '{}'::jsonb
            ) AS has_connection_configs
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to evaluate connection config readiness: {error}"
            ))
        })?;

        row.try_get("has_connection_configs").map_err(|error| {
            AppError::Database(format!(
                "Failed to read has_connection_configs readiness flag: {error}"
            ))
        })
    }

    async fn user_has_credential_sets(&self, user_id: Uuid) -> Result<bool, AppError> {
        let row = sqlx::query(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM connections_user_credential_set
                WHERE user_id = $1
            ) AS user_has_credential_sets
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to evaluate credential set readiness: {error}"
            ))
        })?;

        row.try_get("user_has_credential_sets").map_err(|error| {
            AppError::Database(format!(
                "Failed to read user_has_credential_sets readiness flag: {error}"
            ))
        })
    }

    async fn user_has_connected(&self, user_id: Uuid) -> Result<bool, AppError> {
        let row = sqlx::query(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM connections_session
                WHERE user_id = $1
            ) AS user_has_connected
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to evaluate session readiness: {error}"))
        })?;

        row.try_get("user_has_connected").map_err(|error| {
            AppError::Database(format!(
                "Failed to read user_has_connected readiness flag: {error}"
            ))
        })
    }
}

fn normalize_step(value: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(
            "Wizard current_step is required".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn ensure_step_data_object(step_data: &Value) -> Result<(), AppError> {
    if !step_data.is_object() {
        return Err(AppError::Validation(
            "Wizard step_data must be a JSON object".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ensure_step_data_object, normalize_step};
    use crate::errors::AppError;

    #[test]
    fn normalize_step_rejects_empty_value() {
        let result = normalize_step("   ");
        assert!(
            matches!(&result, Err(AppError::Validation(message)) if message.contains("current_step")),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn normalize_step_trims_valid_value() {
        let result = normalize_step(" inventory ");
        assert_eq!(result.unwrap_or_default(), "inventory");
    }

    #[test]
    fn ensure_step_data_object_rejects_non_object() {
        let result = ensure_step_data_object(&json!(["bad"]));
        assert!(
            matches!(&result, Err(AppError::Validation(message)) if message.contains("step_data")),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn ensure_step_data_object_accepts_object() {
        let result = ensure_step_data_object(&json!({"ok": true}));
        assert!(result.is_ok(), "expected object to be accepted: {result:?}");
    }
}
