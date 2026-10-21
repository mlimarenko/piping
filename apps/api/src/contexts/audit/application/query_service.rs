use std::sync::Arc;

use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::audit::infrastructure::persistence::{AuditEventFilter, AuditRepository};
use crate::errors::AppError;

pub struct AuditQueryService<R>
where
    R: AuditRepository,
{
    repository: Arc<R>,
}

impl<R> AuditQueryService<R>
where
    R: AuditRepository,
{
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn list_events(
        &self,
        filter: AuditEventFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<AuditEvent>, AppError> {
        self.repository.list_events(filter, limit, offset).await
    }

    pub async fn count_events(&self, filter: AuditEventFilter) -> Result<u64, AppError> {
        self.repository.count_events(filter).await
    }
}
