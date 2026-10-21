use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::contexts::inventory::domain::content_type;
use crate::errors::AppError;

/// Batched native JSONB read for custom-field projections, using the canonical
/// content-type registry for SQL identifiers. The value service masks secrets.
#[derive(Clone)]
pub struct SqlxCustomFieldDataRepository {
    pool: PgPool,
}

impl SqlxCustomFieldDataRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn read_page(
        &self,
        token: &str,
        ids: &[Uuid],
    ) -> Result<Vec<(Uuid, Value)>, AppError> {
        let entity = content_type::by_token(token)
            .ok_or_else(|| AppError::Validation("Unsupported inventory content type".into()))?;
        if entity.is_admin_model() {
            return Err(AppError::Validation(
                "AdminModel does not support custom fields".into(),
            ));
        }
        let query = format!(
            "SELECT id,custom_field_data FROM {} WHERE id=ANY($1)",
            entity.table
        );
        sqlx::query_as(&query)
            .bind(ids)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| AppError::Database("Failed to load native custom-field data".into()))
    }
}
