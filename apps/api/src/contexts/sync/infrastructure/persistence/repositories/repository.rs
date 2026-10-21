use uuid::Uuid;

use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::sync::domain::{
    InboundWebhookEvent, OutboundEvent, OutboundEventCounters, Provider, SourceRuntime, SyncJob,
    SyncJobStep,
};
use crate::errors::AppError;

#[derive(Debug, Clone, Default)]
pub struct ProviderListFilter {
    pub provider_type: Vec<String>,
    pub is_enabled: Vec<bool>,
}

pub trait ProviderRepository: Send + Sync {
    fn get_by_id(
        &self,
        provider_id: Uuid,
    ) -> impl Future<Output = Result<Option<Provider>, AppError>> + Send;
    fn list(&self) -> impl Future<Output = Result<Vec<Provider>, AppError>> + Send;
    fn list_paginated(
        &self,
        limit: usize,
        offset: usize,
        sort: Option<SortSpec>,
        filter: ProviderListFilter,
    ) -> impl Future<Output = Result<Vec<Provider>, AppError>> + Send;
    fn count(
        &self,
        filter: ProviderListFilter,
    ) -> impl Future<Output = Result<usize, AppError>> + Send;
    fn create(&self, provider: Provider)
    -> impl Future<Output = Result<Provider, AppError>> + Send;
    fn update(&self, provider: Provider)
    -> impl Future<Output = Result<Provider, AppError>> + Send;
    fn delete(&self, provider_id: Uuid) -> impl Future<Output = Result<(), AppError>> + Send;
    fn set_enabled(
        &self,
        provider_id: Uuid,
        is_enabled: bool,
    ) -> impl Future<Output = Result<Provider, AppError>> + Send;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait SyncJobRepository: Send + Sync {
    async fn create(&self, job: SyncJob) -> Result<SyncJob, AppError>;
    async fn update(&self, job: SyncJob) -> Result<SyncJob, AppError>;
    async fn get_by_id(&self, job_id: Uuid) -> Result<Option<SyncJob>, AppError>;
    /// The most recent job for a provider, bounded to one row.
    async fn latest_for_provider(&self, provider_id: Uuid) -> Result<Option<SyncJob>, AppError>;
    async fn upsert_step(&self, step: SyncJobStep) -> Result<SyncJobStep, AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait SourceRuntimeRepository: Send + Sync {
    async fn get_by_source_id(&self, source_id: Uuid) -> Result<Option<SourceRuntime>, AppError>;
    async fn upsert(&self, runtime: &SourceRuntime) -> Result<(), AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait WebhookEventRepository: Send + Sync {
    async fn get_by_dedup_key(
        &self,
        source_id: Uuid,
        dedup_key: &str,
    ) -> Result<Option<InboundWebhookEvent>, AppError>;
    async fn create(&self, event: &InboundWebhookEvent) -> Result<(), AppError>;
    async fn update_status(
        &self,
        event_id: Uuid,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<(), AppError>;
    /// Delete events older than `days` days. Returns the number of rows pruned.
    async fn prune_older_than(&self, days: i64) -> Result<u64, AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait OutboundEventRepository: Send + Sync {
    async fn enqueue(&self, event: &OutboundEvent) -> Result<(), AppError>;
    async fn list_pending(&self, limit: usize) -> Result<Vec<OutboundEvent>, AppError>;
    async fn mark_in_progress(&self, event_id: Uuid) -> Result<(), AppError>;
    async fn mark_succeeded(&self, event_id: Uuid) -> Result<(), AppError>;
    async fn mark_failed(
        &self,
        event_id: Uuid,
        attempts: i32,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        last_error: Option<&str>,
    ) -> Result<(), AppError>;
    async fn replay_dead_letter(&self, source_id: Uuid, limit: usize) -> Result<usize, AppError>;
    async fn get_counters(&self, source_id: Uuid) -> Result<OutboundEventCounters, AppError>;
}
