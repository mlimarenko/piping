use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::contexts::access_control::domain::UserApiToken;
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::shared::list_query::SortSpec;
use crate::errors::AppError;

#[derive(Debug, Clone, Default)]
pub struct UserApiTokenListFilter {
    pub search: Option<String>,
    pub scopes: Vec<String>,
    pub status: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[async_trait]
pub trait UserApiTokenRepository: Send + Sync {
    async fn create_token(
        &self,
        token: UserApiToken,
        audit_event: AuditEvent,
    ) -> Result<UserApiToken, AppError>;
    async fn list_user_tokens_page(
        &self,
        user_id: Uuid,
        limit: u32,
        offset: u32,
        filter: UserApiTokenListFilter,
    ) -> Result<Vec<UserApiToken>, AppError>;
    async fn count_user_tokens(
        &self,
        user_id: Uuid,
        filter: UserApiTokenListFilter,
    ) -> Result<u64, AppError>;
    async fn find_token_with_scopes(
        &self,
        token_id: Uuid,
    ) -> Result<Option<UserApiToken>, AppError>;
    async fn revoke_user_token(
        &self,
        user_id: Uuid,
        token_id: Uuid,
        now: DateTime<Utc>,
        audit_event: AuditEvent,
    ) -> Result<Option<UserApiToken>, AppError>;
    async fn touch_last_used(&self, token_id: Uuid, now: DateTime<Utc>) -> Result<(), AppError>;
    async fn count_active_tokens_for_user(
        &self,
        user_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<u32, AppError>;
}
