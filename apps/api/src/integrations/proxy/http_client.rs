use std::time::Instant;

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::config::DEFAULT_HTTP_TIMEOUT;
use crate::errors::AppError;
use crate::integrations::proxy::client::{ProxyClient, ProxySession};
use crate::integrations::proxy::metrics::{ProxyMetricsResponse, ProxyMetricsSample};
use crate::integrations::proxy::shared_secret_provider::ProxyCredential;
use crate::observability::{instrument_span, proxy_call_span};

#[derive(Debug, Clone)]
pub struct HttpProxyClient {
    base_url: String,
    client: reqwest::Client,
    default_credential: ProxyCredential,
}

#[derive(Debug, Deserialize)]
struct CreateConnectionResponse {
    connection_id: Uuid,
}

impl HttpProxyClient {
    pub fn new(base_url: String, default_credential: ProxyCredential) -> Result<Self, AppError> {
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_HTTP_TIMEOUT)
            .build()
            .map_err(|error| {
                AppError::External(format!("Failed to build proxy HTTP client: {}", error))
            })?;
        Ok(Self {
            base_url,
            client,
            default_credential,
        })
    }

    fn endpoint_for(&self, base_url: Option<&str>, path: &str) -> String {
        let base = base_url.unwrap_or(&self.base_url);
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    async fn map_status_error(
        &self,
        operation: &str,
        status: reqwest::StatusCode,
        response: reqwest::Response,
    ) -> AppError {
        let body = response.text().await.unwrap_or_default();
        if body.is_empty() {
            AppError::External(format!("Proxy {} failed with status {}", operation, status))
        } else {
            AppError::External(format!(
                "Proxy {} failed with status {}: {}",
                operation, status, body
            ))
        }
    }

    fn credential<'a>(&'a self, credential: Option<&'a ProxyCredential>) -> &'a ProxyCredential {
        credential.unwrap_or(&self.default_credential)
    }
}

impl ProxyClient for HttpProxyClient {
    async fn create_connection(
        &self,
        payload: Value,
        proxy_base_url: Option<&str>,
        credential: Option<&ProxyCredential>,
    ) -> Result<ProxySession, AppError> {
        let span = proxy_call_span("create_connection", &[]);
        instrument_span(span, async {
            let url = self.endpoint_for(proxy_base_url, "proxy/v1/connections");
            let credential = self.credential(credential);
            let response = self
                .client
                .post(url)
                .header("X-Piping-Proxy-Node-Id", &credential.node_id)
                .header("X-Piping-Proxy-Secret", &credential.secret)
                .json(&payload)
                .send()
                .await
                .map_err(|error| {
                    AppError::External(format!("Proxy create_connection request failed: {}", error))
                })?;
            let status = response.status();
            if !status.is_success() {
                return Err(self
                    .map_status_error("create_connection", status, response)
                    .await);
            }
            let payload = response
                .json::<CreateConnectionResponse>()
                .await
                .map_err(|error| {
                    AppError::External(format!(
                        "Invalid proxy create_connection response payload: {}",
                        error
                    ))
                })?;
            Ok(ProxySession {
                id: payload.connection_id,
            })
        })
        .await
    }

    async fn terminate_connection(
        &self,
        proxy_session_id: Uuid,
        proxy_base_url: Option<&str>,
        credential: Option<&ProxyCredential>,
    ) -> Result<(), AppError> {
        let span = proxy_call_span(
            "terminate_connection",
            &[("proxy_session_id", proxy_session_id.to_string())],
        );
        instrument_span(span, async {
            let url = self.endpoint_for(
                proxy_base_url,
                &format!("proxy/v1/connections/{}", proxy_session_id),
            );
            let credential = self.credential(credential);
            let response = self
                .client
                .delete(url)
                .header("X-Piping-Proxy-Node-Id", &credential.node_id)
                .header("X-Piping-Proxy-Secret", &credential.secret)
                .send()
                .await
                .map_err(|error| {
                    AppError::External(format!(
                        "Proxy terminate_connection request failed: {}",
                        error
                    ))
                })?;
            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                // Proxy sessions can expire independently; disconnect should be idempotent.
                return Ok(());
            }
            if !status.is_success() {
                return Err(self
                    .map_status_error("terminate_connection", status, response)
                    .await);
            }
            Ok(())
        })
        .await
    }

    async fn get_metrics(
        &self,
        proxy_session_id: Uuid,
        proxy_base_url: Option<&str>,
        credential: Option<&ProxyCredential>,
    ) -> Result<ProxyMetricsSample, AppError> {
        let span = proxy_call_span(
            "get_metrics",
            &[("proxy_session_id", proxy_session_id.to_string())],
        );
        instrument_span(span, async {
            let url = self.endpoint_for(
                proxy_base_url,
                &format!("proxy/v1/connections/{}/metrics", proxy_session_id),
            );
            let credential = self.credential(credential);
            let request_started_at = Instant::now();
            let response = self
                .client
                .get(url)
                .header("X-Piping-Proxy-Node-Id", &credential.node_id)
                .header("X-Piping-Proxy-Secret", &credential.secret)
                .send()
                .await
                .map_err(|error| {
                    AppError::External(format!("Proxy get_metrics request failed: {}", error))
                })?;
            let request_latency_ms = elapsed_request_latency_ms(request_started_at);
            let status = response.status();
            if !status.is_success() {
                return Err(self.map_status_error("get_metrics", status, response).await);
            }
            let metrics = response
                .json::<ProxyMetricsResponse>()
                .await
                .map_err(|error| {
                    AppError::External(format!(
                        "Invalid proxy get_metrics response payload: {}",
                        error
                    ))
                })?;
            Ok(ProxyMetricsSample {
                metrics,
                request_latency_ms: Some(request_latency_ms),
            })
        })
        .await
    }
}

fn elapsed_request_latency_ms(started_at: Instant) -> i64 {
    let elapsed_ms = started_at.elapsed().as_millis();
    if elapsed_ms > i64::MAX as u128 {
        i64::MAX
    } else {
        (elapsed_ms as i64).max(1)
    }
}
