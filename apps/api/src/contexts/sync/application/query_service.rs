use std::sync::Arc;

use uuid::Uuid;

use crate::contexts::sync::domain::{Provider, SyncJob};
use crate::contexts::sync::infrastructure::{ProviderRepository, SyncJobRepository};
use crate::errors::AppError;

pub struct SyncQueryService<PRepo, JRepo>
where
    PRepo: ProviderRepository,
    JRepo: SyncJobRepository,
{
    providers: Arc<PRepo>,
    jobs: Arc<JRepo>,
}

impl<PRepo, JRepo> SyncQueryService<PRepo, JRepo>
where
    PRepo: ProviderRepository,
    JRepo: SyncJobRepository,
{
    pub fn new(providers: Arc<PRepo>, jobs: Arc<JRepo>) -> Self {
        Self { providers, jobs }
    }

    pub async fn list_providers(&self) -> Result<Vec<Provider>, AppError> {
        self.providers.list().await
    }

    pub async fn get_provider(&self, provider_id: Uuid) -> Result<Option<Provider>, AppError> {
        self.providers.get_by_id(provider_id).await
    }

    pub async fn get_job(&self, job_id: Uuid) -> Result<Option<SyncJob>, AppError> {
        self.jobs.get_by_id(job_id).await
    }
}
