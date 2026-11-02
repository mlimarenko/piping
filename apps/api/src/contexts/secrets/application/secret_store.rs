use async_trait::async_trait;
use uuid::Uuid;

use crate::errors::AppError;

#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn get_secret(&self, secret_id: Uuid) -> Result<String, AppError>;
    async fn set_secret(&self, secret_id: Uuid, value: &str) -> Result<(), AppError>;
    async fn delete_secret(&self, secret_id: Uuid) -> Result<(), AppError>;
    async fn ensure_unique_key(
        &self,
        scope_type: &str,
        scope_id: Uuid,
        key: &str,
    ) -> Result<(), AppError>;
}
