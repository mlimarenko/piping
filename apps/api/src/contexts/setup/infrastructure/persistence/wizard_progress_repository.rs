use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::contexts::setup::domain::WizardProgress;
use crate::errors::AppError;

#[derive(Clone)]
pub struct WizardProgressRepository {
    pool: PgPool,
}

impl WizardProgressRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn find_by_user_and_type(
        &self,
        user_id: Uuid,
        wizard_type: &str,
    ) -> Result<Option<WizardProgress>, AppError> {
        sqlx::query_as::<_, WizardProgress>(
            r#"
            SELECT id,
                   user_id,
                   wizard_type,
                   current_step,
                   step_data,
                   status,
                   completed_at,
                   created_at,
                   updated_at
            FROM wizard_progress
            WHERE user_id = $1
              AND wizard_type = $2
            "#,
        )
        .bind(user_id)
        .bind(wizard_type)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to load wizard progress: {error}")))
    }

    pub async fn upsert(
        &self,
        user_id: Uuid,
        wizard_type: &str,
        current_step: Option<&str>,
        step_data: &Value,
    ) -> Result<WizardProgress, AppError> {
        sqlx::query_as::<_, WizardProgress>(
            r#"
            INSERT INTO wizard_progress (
                id,
                user_id,
                wizard_type,
                current_step,
                step_data,
                status,
                completed_at,
                created_at,
                updated_at
            )
            VALUES (
                $1,
                $2,
                $3,
                $4,
                $5,
                'active',
                NULL,
                NOW(),
                NOW()
            )
            ON CONFLICT (user_id, wizard_type)
            DO UPDATE SET
                current_step = EXCLUDED.current_step,
                step_data = CASE
                    WHEN jsonb_typeof(wizard_progress.step_data) = 'object'
                         AND jsonb_typeof(EXCLUDED.step_data) = 'object'
                    THEN wizard_progress.step_data || EXCLUDED.step_data
                    ELSE EXCLUDED.step_data
                END,
                status = 'active',
                completed_at = NULL,
                updated_at = NOW()
            RETURNING id,
                      user_id,
                      wizard_type,
                      current_step,
                      step_data,
                      status,
                      completed_at,
                      created_at,
                      updated_at
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(wizard_type)
        .bind(current_step)
        .bind(step_data)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to upsert wizard progress: {error}")))
    }

    pub async fn update_status(
        &self,
        user_id: Uuid,
        wizard_type: &str,
        status: &str,
        completed_at: Option<DateTime<Utc>>,
    ) -> Result<WizardProgress, AppError> {
        let progress = sqlx::query_as::<_, WizardProgress>(
            r#"
            UPDATE wizard_progress
            SET status = $3,
                completed_at = $4,
                updated_at = NOW()
            WHERE user_id = $1
              AND wizard_type = $2
            RETURNING id,
                      user_id,
                      wizard_type,
                      current_step,
                      step_data,
                      status,
                      completed_at,
                      created_at,
                      updated_at
            "#,
        )
        .bind(user_id)
        .bind(wizard_type)
        .bind(status)
        .bind(completed_at)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to update wizard progress status: {error}"))
        })?;

        progress.ok_or_else(|| {
            AppError::NotFound("Wizard progress not found for status update".to_string())
        })
    }

    pub async fn reset(
        &self,
        user_id: Uuid,
        wizard_type: &str,
        previous_status: &str,
    ) -> Result<WizardProgress, AppError> {
        let query = if previous_status == "completed" {
            r#"
            UPDATE wizard_progress
            SET status = 'active',
                current_step = NULL,
                completed_at = NULL,
                updated_at = NOW()
            WHERE user_id = $1
              AND wizard_type = $2
            RETURNING id,
                      user_id,
                      wizard_type,
                      current_step,
                      step_data,
                      status,
                      completed_at,
                      created_at,
                      updated_at
            "#
        } else {
            r#"
            UPDATE wizard_progress
            SET status = 'active',
                completed_at = NULL,
                updated_at = NOW()
            WHERE user_id = $1
              AND wizard_type = $2
            RETURNING id,
                      user_id,
                      wizard_type,
                      current_step,
                      step_data,
                      status,
                      completed_at,
                      created_at,
                      updated_at
            "#
        };

        let progress = sqlx::query_as::<_, WizardProgress>(query)
            .bind(user_id)
            .bind(wizard_type)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!("Failed to reset wizard progress state: {error}"))
            })?;

        progress.ok_or_else(|| AppError::NotFound("Wizard progress not found".to_string()))
    }
}
