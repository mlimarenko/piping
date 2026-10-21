use std::sync::Arc;

use async_trait::async_trait;

use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::audit::infrastructure::persistence::AuditRepository;
use crate::errors::AppError;

#[async_trait]
pub trait AuditRecorder: Send + Sync {
    async fn record_event(&self, event: AuditEvent) -> Result<(), AppError>;
}

pub trait AuditPostCommit: Send + Sync {
    fn fanout_recorded_event(&self, event: AuditEvent);
}

/// Best-effort fan-out sink for recorded domain events.
///
/// Implemented by the sync context's `WebhookDispatcher`. The audit service
/// hands every successfully recorded event to this sink on a spawned task; a
/// fan-out failure must never fail or block audit recording. Living here (in
/// the audit context) keeps the dependency direction audit ← sync rather than
/// audit → sync.
#[async_trait]
pub trait AuditEventFanout: Send + Sync {
    async fn on_event(&self, event: &AuditEvent);
}

pub struct AuditService<R>
where
    R: AuditRepository,
{
    repository: Arc<R>,
    /// Optional best-effort fan-out (e.g. outbound webhook delivery).
    fanout: Option<Arc<dyn AuditEventFanout>>,
}

impl<R> AuditService<R>
where
    R: AuditRepository,
{
    pub fn new(repository: Arc<R>) -> Self {
        Self {
            repository,
            fanout: None,
        }
    }

    /// Construct an audit service that also fans recorded events out to `fanout`
    /// on a best-effort basis (spawned, non-blocking, failures swallowed).
    pub fn with_fanout(repository: Arc<R>, fanout: Arc<dyn AuditEventFanout>) -> Self {
        Self {
            repository,
            fanout: Some(fanout),
        }
    }

    pub async fn record_event(&self, event: AuditEvent) -> Result<(), AppError> {
        record(&self.repository, self.fanout.as_ref(), event).await
    }

    /// Dispatch an event that was inserted by a caller-owned database transaction.
    /// The caller must invoke this only after that transaction commits.
    pub fn fanout_recorded_event(&self, event: AuditEvent) {
        dispatch(self.fanout.as_ref(), event);
    }
}

impl<R> AuditPostCommit for AuditService<R>
where
    R: AuditRepository,
{
    fn fanout_recorded_event(&self, event: AuditEvent) {
        dispatch(self.fanout.as_ref(), event);
    }
}

/// Insert the event, then best-effort fan it out. The fan-out runs on a spawned
/// task so a slow/failing sink never blocks or fails audit recording.
async fn record<R>(
    repository: &Arc<R>,
    fanout: Option<&Arc<dyn AuditEventFanout>>,
    event: AuditEvent,
) -> Result<(), AppError>
where
    R: AuditRepository + ?Sized,
{
    repository.insert(event.clone()).await?;
    dispatch(fanout, event);
    Ok(())
}

fn dispatch(fanout: Option<&Arc<dyn AuditEventFanout>>, event: AuditEvent) {
    if let Some(fanout) = fanout {
        let fanout = fanout.clone();
        tokio::spawn(async move {
            fanout.on_event(&event).await;
        });
    }
}

#[async_trait]
impl<R> AuditRecorder for AuditService<R>
where
    R: AuditRepository + Send + Sync,
{
    async fn record_event(&self, event: AuditEvent) -> Result<(), AppError> {
        record(&self.repository, self.fanout.as_ref(), event).await
    }
}

#[async_trait]
impl<R> AuditRecorder for Arc<AuditService<R>>
where
    R: AuditRepository + Send + Sync,
{
    async fn record_event(&self, event: AuditEvent) -> Result<(), AppError> {
        record(&self.repository, self.fanout.as_ref(), event).await
    }
}
