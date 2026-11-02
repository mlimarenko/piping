use super::*;

use crate::contexts::connection_config::ConnConfScope;
use serde_json::json;

impl<R, P, A, M, SR, SS> ConnectionService<R, P, A, M, SR, SS>
where
    R: ConnectionRepository,
    P: ProxyClient,
    A: AuditRecorder,
    M: TargetMaterializer,
    SR: SecretRepository,
    SS: SecretStore,
{
    async fn ensure_proxy_node_enabled(&self, proxy_node_id: Uuid) -> Result<ProxyNode, AppError> {
        let proxy_node = self
            .repository
            .get_proxy_node(proxy_node_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Proxy node not found".to_string()))?;
        if !proxy_node.enabled {
            return Err(AppError::Validation(format!(
                "Proxy node '{}' is disabled",
                proxy_node.slug
            )));
        }
        Ok(proxy_node)
    }

    async fn direct_target_proxy_scope(&self, target_id: Uuid) -> Result<ConnConfScope, AppError> {
        let Some(scopes) = self
            .connconf_assignments
            .get_device_scopes(target_id)
            .await?
        else {
            return Err(AppError::NotFound(
                "Connection target not found".to_string(),
            ));
        };
        if scopes.device_id == Some(target_id) {
            return Ok(ConnConfScope::Device);
        }
        if scopes.vm_id == Some(target_id) {
            return Ok(ConnConfScope::Vm);
        }
        Err(AppError::NotFound(
            "Connection target not found".to_string(),
        ))
    }

    pub async fn get_proxy_node(&self, proxy_node_id: Uuid) -> Result<Option<ProxyNode>, AppError> {
        self.repository.get_proxy_node(proxy_node_id).await
    }

    pub async fn list_proxy_nodes(
        &self,
        filter: ProxyNodeFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ProxyNode>, AppError> {
        self.repository
            .list_proxy_nodes(filter, limit, offset)
            .await
    }

    pub async fn count_proxy_nodes(&self, filter: ProxyNodeFilter) -> Result<u64, AppError> {
        self.repository.count_proxy_nodes(filter).await
    }

    pub async fn update_proxy_node(
        &self,
        proxy_node_id: Uuid,
        input: UpdateProxyNodeInput,
    ) -> Result<ProxyNode, AppError> {
        let mut proxy_node = self
            .repository
            .get_proxy_node(proxy_node_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Proxy node not found".to_string()))?;
        proxy_node.name = input.name.trim().to_string();
        proxy_node.slug = input.slug.trim().to_string();
        proxy_node.base_url = input.base_url.trim().to_string();
        proxy_node.enabled = input.enabled;
        proxy_node.updated_at = Utc::now();
        proxy_node
            .validate()
            .map_err(|error| AppError::Validation(error.to_string()))?;

        if let Some(existing) = self
            .repository
            .get_proxy_node_by_slug(&proxy_node.slug)
            .await?
            && existing.id != proxy_node.id
        {
            return Err(AppError::Conflict(format!(
                "Proxy node slug '{}' already exists",
                proxy_node.slug
            )));
        }

        self.repository.update_proxy_node(&proxy_node).await?;
        Ok(proxy_node)
    }

    pub async fn delete_proxy_node(&self, proxy_node_id: Uuid) -> Result<bool, AppError> {
        self.repository.delete_proxy_node(proxy_node_id).await
    }

    pub async fn create_proxy_node_assignment(
        &self,
        input: CreateProxyNodeAssignmentInput,
    ) -> Result<ProxyNodeAssignment, AppError> {
        self.ensure_proxy_node_enabled(input.proxy_node_id).await?;
        validate_proxy_assignment_scope(input.scope_type, input.scope_id)?;

        let input = NewProxyNodeAssignment {
            proxy_node_id: input.proxy_node_id,
            scope_type: input.scope_type,
            scope_id: input.scope_id,
            weight: input.weight,
            enabled: input.enabled,
        };
        self.repository.create_proxy_node_assignment(&input).await
    }

    pub async fn get_proxy_node_assignment(
        &self,
        assignment_id: Uuid,
    ) -> Result<Option<ProxyNodeAssignment>, AppError> {
        self.repository
            .get_proxy_node_assignment(assignment_id)
            .await
    }

    pub async fn list_proxy_node_assignments(
        &self,
        filter: ProxyNodeAssignmentFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ProxyNodeAssignment>, AppError> {
        self.repository
            .list_proxy_node_assignments(filter, limit, offset)
            .await
    }

    pub async fn count_proxy_node_assignments(
        &self,
        filter: ProxyNodeAssignmentFilter,
    ) -> Result<u64, AppError> {
        self.repository.count_proxy_node_assignments(filter).await
    }

    pub async fn update_proxy_node_assignment(
        &self,
        assignment_id: Uuid,
        input: UpdateProxyNodeAssignmentInput,
    ) -> Result<ProxyNodeAssignment, AppError> {
        if let Some(proxy_node_id) = input.proxy_node_id {
            self.ensure_proxy_node_enabled(proxy_node_id).await?;
        }
        if let Some(scope_type) = input.scope_type {
            validate_proxy_assignment_scope(scope_type, input.scope_id)?;
        }

        let update = UpdateProxyNodeAssignment {
            proxy_node_id: input.proxy_node_id,
            scope_type: input.scope_type,
            scope_id: input.scope_id,
            weight: input.weight,
            enabled: input.enabled,
        };
        self.repository
            .update_proxy_node_assignment(assignment_id, &update)
            .await?
            .ok_or_else(|| AppError::NotFound("Proxy node assignment not found".to_string()))
    }

    pub async fn delete_proxy_node_assignment(&self, assignment_id: Uuid) -> Result<(), AppError> {
        if self
            .repository
            .delete_proxy_node_assignment(assignment_id)
            .await?
        {
            Ok(())
        } else {
            Err(AppError::NotFound(
                "Proxy node assignment not found".to_string(),
            ))
        }
    }

    pub async fn get_target_proxy_assignment(
        &self,
        target_id: Uuid,
    ) -> Result<Option<TargetProxyAssignment>, AppError> {
        self.repository.get_target_proxy_assignment(target_id).await
    }

    pub async fn set_target_proxy_assignment(
        &self,
        target_id: Uuid,
        proxy_node_id: Uuid,
    ) -> Result<TargetProxyAssignment, AppError> {
        self.ensure_proxy_node_enabled(proxy_node_id).await?;
        self.direct_target_proxy_scope(target_id).await?;

        let assignment = TargetProxyAssignment {
            target_id,
            proxy_node_id,
            updated_at: Utc::now(),
        };
        self.repository
            .upsert_target_proxy_assignment(&assignment)
            .await?;
        Ok(assignment)
    }

    pub async fn clear_target_proxy_assignment(&self, target_id: Uuid) -> Result<(), AppError> {
        self.direct_target_proxy_scope(target_id).await?;
        self.repository
            .clear_target_proxy_assignment(target_id)
            .await
    }

    pub async fn get_session(
        &self,
        session_id: Uuid,
    ) -> Result<Option<ConnectionSession>, AppError> {
        self.repository.get_session(session_id).await
    }

    pub async fn require_session(&self, session_id: Uuid) -> Result<ConnectionSession, AppError> {
        self.repository
            .get_session(session_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Connection session not found".to_string()))
    }

    pub async fn touch_session(&self, session_id: Uuid) -> Result<(), AppError> {
        self.repository
            .update_last_activity(session_id, Utc::now())
            .await
    }

    pub async fn list_sessions(
        &self,
        filter: ConnectionSessionFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ConnectionSession>, AppError> {
        self.repository.list_sessions(filter, limit, offset).await
    }

    pub async fn count_sessions(&self, filter: ConnectionSessionFilter) -> Result<u64, AppError> {
        self.repository.count_sessions(filter).await
    }

    pub async fn resolve_session_proxy_route(
        &self,
        session: &ConnectionSession,
    ) -> Result<ConnectionProxyRoute, AppError> {
        let decision = self
            .routing_decisions
            .resolve_for_existing_session(session)
            .await?;
        self.proxy_route_for_decision(&decision).await
    }

    pub(crate) async fn proxy_route_for_decision(
        &self,
        decision: &ConnectionRoutingDecision,
    ) -> Result<ConnectionProxyRoute, AppError> {
        let credential = match decision.proxy_node_id {
            Some(proxy_node_id) => Some(self.proxy_node_credential(proxy_node_id).await?),
            None => None,
        };
        Ok(ConnectionProxyRoute {
            proxy_base_url: decision.proxy_base_url.clone(),
            credential,
        })
    }

    pub async fn mark_session_active(
        &self,
        session_id: Uuid,
        activated_by: Uuid,
    ) -> Result<ConnectionSession, AppError> {
        let span = operation_span(
            "mark_session_active",
            &[("session_id", session_id.to_string())],
        );
        return instrument_span(span, async {
            let mut session = self
                .repository
                .get_session(session_id)
                .await?
                .ok_or_else(|| AppError::NotFound("Connection session not found".to_string()))?;

            if matches!(
                session.status,
                ConnectionStatus::Active
                    | ConnectionStatus::Disconnected
                    | ConnectionStatus::Terminated
                    | ConnectionStatus::Failed
            ) {
                log_connection_event(log_connection_event::Input {
                    event: "connection.session.activate_skipped",
                    session_id: Some(session.id.to_string()),
                    host_id: Some(session.target_id.to_string()),
                    user_id: Some(session.user_id.to_string()),
                    protocol: Some(format!("{:?}", session.protocol)),
                    status: Some(format!("{:?}", session.status)),
                    correlation_id: None,
                });
                return Ok(session);
            }

            Self::ensure_transition(&session.status, &ConnectionStatus::Active)?;
            session.status = ConnectionStatus::Active;
            session.last_activity = Utc::now();
            self.repository
                .update_status(session.id, session.status.clone(), session.ended_at)
                .await?;
            self.repository
                .update_last_activity(session.id, session.last_activity)
                .await?;

            let event = AuditEvent {
                id: Uuid::now_v7(),
                event_type: "connection.session.activated".to_string(),
                actor_id: Some(activated_by),
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
                event: "connection.session.activated",
                session_id: Some(session.id.to_string()),
                host_id: Some(session.target_id.to_string()),
                user_id: Some(session.user_id.to_string()),
                protocol: Some(format!("{:?}", session.protocol)),
                status: Some(format!("{:?}", session.status)),
                correlation_id: None,
            });
            Ok(session)
        })
        .await;
    }

    pub async fn mark_session_failed(
        &self,
        session_id: Uuid,
        failed_by: Uuid,
    ) -> Result<ConnectionSession, AppError> {
        let span = operation_span(
            "mark_session_failed",
            &[("session_id", session_id.to_string())],
        );
        return instrument_span(span, async {
            let mut session = self
                .repository
                .get_session(session_id)
                .await?
                .ok_or_else(|| AppError::NotFound("Connection session not found".to_string()))?;

            if session.status.is_terminal() {
                self.forget_client_credentials(&session);
                log_connection_event(log_connection_event::Input {
                    event: "connection.session.fail_skipped",
                    session_id: Some(session.id.to_string()),
                    host_id: Some(session.target_id.to_string()),
                    user_id: Some(session.user_id.to_string()),
                    protocol: Some(format!("{:?}", session.protocol)),
                    status: Some(format!("{:?}", session.status)),
                    correlation_id: None,
                });
                return Ok(session);
            }

            Self::ensure_transition(&session.status, &ConnectionStatus::Failed)?;
            let now = Utc::now();
            session.status = ConnectionStatus::Failed;
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
                event_type: "connection.session.failed".to_string(),
                actor_id: Some(failed_by),
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
                event: "connection.session.failed",
                session_id: Some(session.id.to_string()),
                host_id: Some(session.target_id.to_string()),
                user_id: Some(session.user_id.to_string()),
                protocol: Some(format!("{:?}", session.protocol)),
                status: Some(format!("{:?}", session.status)),
                correlation_id: None,
            });
            Ok(session)
        })
        .await;
    }

    pub async fn mark_session_disconnected(
        &self,
        session_id: Uuid,
        disconnected_by: Uuid,
    ) -> Result<ConnectionSession, AppError> {
        let span = operation_span(
            "mark_session_disconnected",
            &[("session_id", session_id.to_string())],
        );
        return instrument_span(span, async {
            let mut session = self
                .repository
                .get_session(session_id)
                .await?
                .ok_or_else(|| AppError::NotFound("Connection session not found".to_string()))?;

            if session.status.is_terminal() {
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
                actor_id: Some(disconnected_by),
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
            Ok(session)
        })
        .await;
    }

    pub async fn terminate_session(
        &self,
        session_id: Uuid,
        terminated_by: Uuid,
    ) -> Result<ConnectionSession, AppError> {
        let span = operation_span(
            "terminate_session",
            &[("session_id", session_id.to_string())],
        );
        return instrument_span(span, async {
            let mut session = self
                .repository
                .get_session(session_id)
                .await?
                .ok_or_else(|| AppError::NotFound("Connection session not found".to_string()))?;

            if matches!(
                session.status,
                ConnectionStatus::Terminated
                    | ConnectionStatus::Failed
                    | ConnectionStatus::Disconnected
            ) {
                self.forget_client_credentials(&session);
                log_connection_event(log_connection_event::Input {
                    event: "connection.session.terminate_skipped",
                    session_id: Some(session.id.to_string()),
                    host_id: Some(session.target_id.to_string()),
                    user_id: Some(session.user_id.to_string()),
                    protocol: Some(format!("{:?}", session.protocol)),
                    status: Some(format!("{:?}", session.status)),
                    correlation_id: None,
                });
                return Ok(session);
            }

            Self::ensure_transition(&session.status, &ConnectionStatus::Terminated)?;

            if let Some(proxy_id) = session.proxy_id {
                let proxy_route = self.resolve_session_proxy_route(&session).await?;
                let proxy_span =
                    proxy_call_span("terminate_session", &[("proxy_id", proxy_id.to_string())]);
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
            session.status = ConnectionStatus::Terminated;
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
                event_type: "connection.session.terminated".to_string(),
                actor_id: Some(terminated_by),
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
                event: "connection.session.terminated",
                session_id: Some(session.id.to_string()),
                host_id: Some(session.target_id.to_string()),
                user_id: Some(session.user_id.to_string()),
                protocol: Some(format!("{:?}", session.protocol)),
                status: Some(format!("{:?}", session.status)),
                correlation_id: None,
            });
            Ok(session)
        })
        .await;
    }
}

fn validate_proxy_assignment_scope(
    scope_type: ConnConfScope,
    scope_id: Option<Uuid>,
) -> Result<(), AppError> {
    if scope_type == ConnConfScope::Global && scope_id.is_some() {
        return Err(AppError::Validation(
            "global scope must not carry a scope_id".to_string(),
        ));
    }
    if scope_type != ConnConfScope::Global && scope_id.is_none() {
        return Err(AppError::Validation(format!(
            "scope_type '{}' requires a scope_id",
            scope_type.as_str()
        )));
    }
    Ok(())
}
