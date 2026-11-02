use super::*;

use serde_json::json;

use crate::integrations::proxy::ProxyMetricsSample;

/// Best-effort: turn the proxy's trust-on-first-use observed keys into pins.
///
/// The proxy is the only reporter of the keys it learned at connect time; this
/// reads its metrics and forwards `observed_host_keys` to the pin store. Both a
/// metrics failure (the proxy is already gone or unreachable) and a persistence
/// failure are logged and swallowed: failing or blocking the disconnect because
/// a pin write failed would leave the session half-closed, and the next connect
/// simply re-observes the key.
pub(super) async fn persist_observed_host_keys<R, P>(
    repository: &R,
    proxy_client: &P,
    proxy_id: Uuid,
    proxy_route: &ConnectionProxyRoute,
) where
    R: ConnectionRepository + ?Sized,
    P: ProxyClient + ?Sized,
{
    let sample = match proxy_client
        .get_metrics(
            proxy_id,
            proxy_route.proxy_base_url.as_deref(),
            proxy_route.credential.as_ref(),
        )
        .await
    {
        Ok(sample) => sample,
        Err(error) => {
            log_event(
                LogLevel::Debug,
                "connection.host_key_pins.metrics_unavailable",
                EventFields::default()
                    .with_operation("disconnect_session")
                    .with_context("proxy_id", proxy_id.to_string())
                    .with_message(error.to_string()),
            );
            return;
        }
    };

    let lines = sample.metrics.observed_host_keys;
    if lines.is_empty() {
        return;
    }

    if let Err(error) = repository.persist_observed_host_key_lines(&lines).await {
        log_event(
            LogLevel::Warning,
            "connection.host_key_pins.persist_failed",
            EventFields::default()
                .with_operation("disconnect_session")
                .with_context("proxy_id", proxy_id.to_string())
                .with_message(error.to_string()),
        );
    }
}

impl<R, P, A, M, SR, SS> ConnectionService<R, P, A, M, SR, SS>
where
    R: ConnectionRepository,
    P: ProxyClient,
    A: AuditRecorder,
    M: TargetMaterializer,
    SR: SecretRepository,
    SS: SecretStore,
{
    pub async fn disconnect_session(
        &self,
        session_id: Uuid,
        user_id: Uuid,
    ) -> Result<ConnectionSession, AppError> {
        let span = operation_span(
            "disconnect_session",
            &[("session_id", session_id.to_string())],
        );
        instrument_span(span, async {
            let mut session = self
                .repository
                .get_session(session_id)
                .await?
                .ok_or_else(|| AppError::NotFound("Connection session not found".to_string()))?;

            if session.user_id != user_id {
                return Err(AppError::NotFound(
                    "Connection session not found".to_string(),
                ));
            }

            if matches!(
                session.status,
                ConnectionStatus::Terminated
                    | ConnectionStatus::Failed
                    | ConnectionStatus::Disconnected
            ) {
                self.forget_client_credentials(&session);
                log_connection_event(log_connection_event::Input {
                    event: "connection.session.disconnect_skipped",
                    session_id: Some(session.id.to_string()),
                    host_id: Some(session.target_id.to_string()),
                    user_id: Some(session.user_id.to_string()),
                    protocol: Some(format!("{:?}", session.protocol)),
                    status: Some(format!("{:?}", session.status)),
                    correlation_id: None,
                });
                return Ok(session);
            }

            Self::ensure_transition(&session.status, &ConnectionStatus::Disconnected)?;

            if let Some(proxy_id) = session.proxy_id {
                let proxy_route = self.resolve_session_proxy_route(&session).await?;
                // Read metrics before terminating: the live proxy is the only
                // reporter of the keys it learned during this session, and the
                // call never blocks or fails the disconnect.
                persist_observed_host_keys(
                    self.repository.as_ref(),
                    self.proxy_client.as_ref(),
                    proxy_id,
                    &proxy_route,
                )
                .await;
                let proxy_span =
                    proxy_call_span("disconnect_session", &[("proxy_id", proxy_id.to_string())]);
                instrument_span(
                    proxy_span,
                    self.proxy_client.terminate_connection(
                        proxy_id,
                        proxy_route.proxy_base_url.as_deref(),
                        proxy_route.credential.as_ref(),
                    ),
                )
                .await?;
            }

            let now = Utc::now();
            session.status = ConnectionStatus::Disconnected;
            session.ended_at = Some(now);
            session.last_activity = now;
            self.repository
                .update_status(session.id, session.status.clone(), session.ended_at)
                .await?;
            self.forget_client_credentials(&session);
            self.repository
                .update_last_activity(session.id, session.last_activity)
                .await?;

            let event = AuditEvent {
                id: Uuid::now_v7(),
                event_type: "connection.session.disconnected".to_string(),
                actor_id: Some(user_id),
                actor_kind: "user".to_string(),
                target_type: Some("connection_session".to_string()),
                target_id: Some(session.id),
                metadata: json!({
                    "target_id": session.target_id.to_string(),
                    "protocol": format!("{:?}", session.protocol),
                    "status": format!("{:?}", session.status),
                }),
                created_at: Utc::now(),
            };
            self.audit_service.record_event(event).await?;

            log_connection_event(log_connection_event::Input {
                event: "connection.session.disconnected",
                session_id: Some(session.id.to_string()),
                host_id: Some(session.target_id.to_string()),
                user_id: Some(session.user_id.to_string()),
                protocol: Some(format!("{:?}", session.protocol)),
                status: Some(format!("{:?}", session.status)),
                correlation_id: None,
            });
            log_event(
                LogLevel::Info,
                "connection.session.disconnected",
                EventFields::default()
                    .with_operation("disconnect_session")
                    .with_resource("connection", session.id.to_string())
                    .with_status(format!("{:?}", session.status)),
            );
            Ok(session)
        })
        .await
    }

    /// Get connection metrics from proxy for an active session.
    /// Returns None if session not found, has no proxy, or metrics unavailable.
    pub async fn get_connection_metrics(
        &self,
        session_id: Uuid,
    ) -> Result<Option<ProxyMetricsSample>, AppError> {
        let Some(session) = self.repository.get_session(session_id).await? else {
            return Ok(None);
        };
        let Some(proxy_id) = session.proxy_id else {
            return Ok(None);
        };
        let proxy_route = self.resolve_session_proxy_route(&session).await?;
        let payload = self
            .proxy_client
            .get_metrics(
                proxy_id,
                proxy_route.proxy_base_url.as_deref(),
                proxy_route.credential.as_ref(),
            )
            .await?;
        Ok(Some(payload))
    }
}
