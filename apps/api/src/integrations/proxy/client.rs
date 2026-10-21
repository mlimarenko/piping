use serde_json::Value;
use uuid::Uuid;

use crate::errors::AppError;
use crate::integrations::proxy::metrics::ProxyMetricsSample;
use crate::integrations::proxy::shared_secret_provider::ProxyCredential;

/// Proxy session handle returned by proxy integration.
#[derive(Debug, Clone)]
pub struct ProxySession {
    pub id: Uuid,
}

/// Proxy integration client for creating and terminating sessions.
#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ProxyClient: Send + Sync {
    async fn create_connection(
        &self,
        payload: Value,
        proxy_base_url: Option<&str>,
        credential: Option<&ProxyCredential>,
    ) -> Result<ProxySession, AppError>;
    async fn terminate_connection(
        &self,
        proxy_session_id: Uuid,
        proxy_base_url: Option<&str>,
        credential: Option<&ProxyCredential>,
    ) -> Result<(), AppError>;
    async fn get_metrics(
        &self,
        proxy_session_id: Uuid,
        proxy_base_url: Option<&str>,
        credential: Option<&ProxyCredential>,
    ) -> Result<ProxyMetricsSample, AppError>;
}
