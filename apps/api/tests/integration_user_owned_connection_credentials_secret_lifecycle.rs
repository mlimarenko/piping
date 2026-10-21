//! Integration test for user-owned connection credentials resolved through the unified secrets engine.
//!
//! This is intentionally self-contained (no Postgres/Redis/proxy) and validates the server-side
//! launch payload resolution behavior by stubbing the proxy client.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use uuid::Uuid;

use piping_backend::contexts::audit::application::AuditRecorder;
use piping_backend::contexts::audit::domain::AuditEvent;
use piping_backend::contexts::connection_config::{
    ConnConfAssignment, ConnConfAssignmentFilter, ConnConfAssignmentRepository, ConnConfProtocol,
    ConnConfRecord, ConnConfRecordFilter, ConnConfRecordRepository, ConnConfScope, DeviceScopeIds,
    NewConnConfAssignment, NewConnConfRecord, UpdateConnConfAssignment, UpdateConnConfRecord,
};
use piping_backend::contexts::connections::application::connection_service::ConnectionServiceDeps;
use piping_backend::contexts::connections::application::{ConnectionService, CreateSessionInput};
use piping_backend::contexts::connections::domain::{
    ConnectionProtocol, ConnectionSession, ConnectionStatus, ConnectionTemplate,
    NewProxyNodeAssignment, ProxyNode, ProxyNodeAssignment, RoutingSource, TargetProxyAssignment,
    USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE, UpdateProxyNodeAssignment, UserCredentialSet,
};
use piping_backend::contexts::connections::infrastructure::ConnectionRepository;
use piping_backend::contexts::connections::infrastructure::persistence::{
    ConnectionSessionFilter, ConnectionTemplateFilter, ProxyNodeAssignmentFilter, ProxyNodeFilter,
    UserCredentialSetFilter,
};
use piping_backend::contexts::inventory::application::{
    MaterializedTargetConfig, TargetMaterializer, materialization::MaterializeTargetConfigInput,
};
use piping_backend::contexts::secrets::application::{
    PROXY_NODE_CREDENTIAL_CONSUMER_TYPE, SecretStore,
};
use piping_backend::contexts::secrets::domain::{
    SecretBinding, SecretUsageBinding, SecretVariable,
};
use piping_backend::contexts::secrets::infrastructure::SecretRepository;
use piping_backend::errors::AppError;
use piping_backend::integrations::proxy::{
    ProxyClient, ProxyCredential, ProxyMetricsResponse, ProxyMetricsSample, ProxySession,
};

const USER_CRED_SET_CONSUMER_TYPE: &str = USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE;
const FIELD_KEY_SSH_PASSWORD: &str = "ssh_password";
const FIELD_KEY_SSH_PRIVATE_KEY: &str = "ssh_private_key";
const FIELD_KEY_RDP_USERNAME: &str = "rdp_username";
const FIELD_KEY_RDP_PASSWORD: &str = "rdp_password";
const FIELD_KEY_VNC_PASSWORD: &str = "vnc_password";
const FIELD_KEY_PROXY_NODE_SECRET: &str = "node_secret";
const SESSION_METADATA_ENTRY_ID_KEY: &str = "entryId";
const SESSION_METADATA_MERGE_POLICY_KEY: &str = "credentialMergePolicy";
const SESSION_MERGE_POLICY_PERSONAL_FIRST: &str = "personal_first";
const ENTRY_ID_CONFIG_CONTEXT_SSH_PRIMARY: &str = "config:config-context:ssh:0";
const ENTRY_ID_CONFIG_CONTEXT_RDP_PRIMARY: &str = "config:config-context:rdp:0";
const ENTRY_ID_CONFIG_CONTEXT_VNC_PRIMARY: &str = "config:config-context:vnc:0";
const LOOPBACK_HOST: &str = "127.0.0.1";
const SSH_PORT_DEFAULT: i64 = 22;
const SSH_PORT_ALT: i64 = 2222;
const SSH_PORT_PERSONAL_FIRST: i64 = 2202;
const RDP_PORT_DEFAULT: i64 = 3389;
const VNC_PORT_DEFAULT: i64 = 5900;
const SCREEN_WIDTH_1366: i64 = 1366;
const SCREEN_HEIGHT_768: i64 = 768;
const SESSION_CREATED_EXPECTATION: &str = "session created";
type TestConnectionService = ConnectionService<
    StubConnectionRepository,
    CapturingProxyClient,
    StubAuditRecorder,
    StubMaterializer,
    StubSecretRepository,
    StubSecretStore,
>;

#[derive(Default)]
struct StubConnectionRepository {
    user_sets: HashMap<Uuid, UserCredentialSet>,
    proxy_nodes: HashMap<Uuid, ProxyNode>,
    proxy_assignments: Vec<ProxyNodeAssignment>,
}

impl ConnectionRepository for StubConnectionRepository {
    async fn create_template(&self, _template: &ConnectionTemplate) -> Result<(), AppError> {
        Ok(())
    }

    async fn get_template(
        &self,
        _template_id: Uuid,
    ) -> Result<Option<ConnectionTemplate>, AppError> {
        Ok(None)
    }

    async fn list_templates(
        &self,
        _filter: ConnectionTemplateFilter,
        _limit: Option<u32>,
        _offset: Option<u32>,
    ) -> Result<Vec<ConnectionTemplate>, AppError> {
        Ok(Vec::new())
    }

    async fn count_templates(&self, _filter: ConnectionTemplateFilter) -> Result<u64, AppError> {
        Ok(0)
    }

    async fn create_session(&self, _session: &ConnectionSession) -> Result<(), AppError> {
        Ok(())
    }

    async fn get_session(&self, _session_id: Uuid) -> Result<Option<ConnectionSession>, AppError> {
        Ok(None)
    }

    async fn list_sessions(
        &self,
        _filter: ConnectionSessionFilter,
        _limit: Option<u32>,
        _offset: Option<u32>,
    ) -> Result<Vec<ConnectionSession>, AppError> {
        Ok(Vec::new())
    }

    async fn count_sessions(&self, _filter: ConnectionSessionFilter) -> Result<u64, AppError> {
        Ok(0)
    }

    async fn update_session(&self, _session: &ConnectionSession) -> Result<(), AppError> {
        Ok(())
    }

    async fn update_status(
        &self,
        _session_id: Uuid,
        _status: ConnectionStatus,
        _ended_at: Option<DateTime<Utc>>,
    ) -> Result<(), AppError> {
        Ok(())
    }

    async fn update_last_activity(
        &self,
        _session_id: Uuid,
        _last_activity: DateTime<Utc>,
    ) -> Result<(), AppError> {
        Ok(())
    }

    async fn create_proxy_node(&self, _proxy_node: &ProxyNode) -> Result<(), AppError> {
        Ok(())
    }

    async fn get_proxy_node(&self, proxy_node_id: Uuid) -> Result<Option<ProxyNode>, AppError> {
        Ok(self.proxy_nodes.get(&proxy_node_id).cloned())
    }

    async fn get_proxy_node_by_slug(&self, _slug: &str) -> Result<Option<ProxyNode>, AppError> {
        Ok(None)
    }

    async fn list_proxy_nodes(
        &self,
        _filter: ProxyNodeFilter,
        _limit: Option<u32>,
        _offset: Option<u32>,
    ) -> Result<Vec<ProxyNode>, AppError> {
        Ok(Vec::new())
    }

    async fn count_proxy_nodes(&self, _filter: ProxyNodeFilter) -> Result<u64, AppError> {
        Ok(0)
    }

    async fn update_proxy_node(&self, _proxy_node: &ProxyNode) -> Result<(), AppError> {
        Ok(())
    }

    async fn delete_proxy_node(&self, _proxy_node_id: Uuid) -> Result<bool, AppError> {
        Ok(true)
    }

    async fn create_proxy_node_assignment(
        &self,
        _input: &NewProxyNodeAssignment,
    ) -> Result<ProxyNodeAssignment, AppError> {
        Err(AppError::Internal(
            "proxy node assignments are not used in this test".to_string(),
        ))
    }

    async fn get_proxy_node_assignment(
        &self,
        assignment_id: Uuid,
    ) -> Result<Option<ProxyNodeAssignment>, AppError> {
        Ok(self
            .proxy_assignments
            .iter()
            .find(|assignment| assignment.id == assignment_id)
            .cloned())
    }

    async fn list_proxy_node_assignments(
        &self,
        filter: ProxyNodeAssignmentFilter,
        _limit: Option<u32>,
        _offset: Option<u32>,
    ) -> Result<Vec<ProxyNodeAssignment>, AppError> {
        let mut assignments: Vec<_> = self.proxy_assignments.clone();
        if !filter.proxy_node_id.is_empty() {
            assignments
                .retain(|assignment| filter.proxy_node_id.contains(&assignment.proxy_node_id));
        }
        if !filter.scope_type.is_empty() {
            assignments.retain(|assignment| filter.scope_type.contains(&assignment.scope_type));
        }
        if let Some(scope_id) = filter.scope_id {
            assignments.retain(|assignment| assignment.scope_id == Some(scope_id));
        }
        if !filter.enabled.is_empty() {
            assignments.retain(|assignment| {
                filter
                    .enabled
                    .iter()
                    .any(|enabled| enabled == &assignment.enabled.to_string())
            });
        }
        Ok(assignments)
    }

    async fn count_proxy_node_assignments(
        &self,
        filter: ProxyNodeAssignmentFilter,
    ) -> Result<u64, AppError> {
        Ok(self
            .list_proxy_node_assignments(filter, None, None)
            .await?
            .len() as u64)
    }

    async fn update_proxy_node_assignment(
        &self,
        _assignment_id: Uuid,
        _input: &UpdateProxyNodeAssignment,
    ) -> Result<Option<ProxyNodeAssignment>, AppError> {
        Ok(None)
    }

    async fn delete_proxy_node_assignment(&self, _assignment_id: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }

    async fn list_proxy_node_assignments_for_scopes(
        &self,
        scopes: &[(ConnConfScope, Option<Uuid>)],
    ) -> Result<Vec<ProxyNodeAssignment>, AppError> {
        Ok(self
            .proxy_assignments
            .iter()
            .filter(|assignment| {
                assignment.enabled
                    && scopes.iter().any(|(scope_type, scope_id)| {
                        assignment.scope_type == *scope_type && assignment.scope_id == *scope_id
                    })
            })
            .cloned()
            .collect())
    }

    async fn get_target_proxy_assignment(
        &self,
        _target_id: Uuid,
    ) -> Result<Option<TargetProxyAssignment>, AppError> {
        Ok(None)
    }

    async fn upsert_target_proxy_assignment(
        &self,
        _assignment: &TargetProxyAssignment,
    ) -> Result<(), AppError> {
        Ok(())
    }

    async fn clear_target_proxy_assignment(&self, _target_id: Uuid) -> Result<(), AppError> {
        Ok(())
    }

    async fn mark_proxy_node_secret_rotated(
        &self,
        _proxy_node_id: Uuid,
        _rotated_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        Ok(())
    }

    async fn is_endpoint_bound_to_target(
        &self,
        _target_id: Uuid,
        _endpoint_host: &str,
    ) -> Result<bool, AppError> {
        Ok(true)
    }

    async fn create_user_credential_set(&self, _set: &UserCredentialSet) -> Result<(), AppError> {
        Ok(())
    }

    async fn get_user_credential_set(
        &self,
        set_id: Uuid,
    ) -> Result<Option<UserCredentialSet>, AppError> {
        Ok(self.user_sets.get(&set_id).cloned())
    }

    async fn list_user_credential_sets(
        &self,
        filter: UserCredentialSetFilter,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<(Vec<UserCredentialSet>, i64), AppError> {
        let matches: Vec<UserCredentialSet> = self
            .user_sets
            .values()
            .filter(|set| {
                filter.user_id.contains(&set.user_id)
                    && (filter.protocol.is_empty() || filter.protocol.contains(&set.protocol))
                    && (filter.is_default.is_empty()
                        || filter.is_default.contains(&set.is_default.to_string()))
            })
            .cloned()
            .collect();
        // Mirror the SQL contract: `total` is the unpaged match count, the
        // page is cut after filtering.
        // Saturating conversions rather than fallible ones: this is a mock over
        // an in-memory map, so a count that does not fit is unreachable, and a
        // panicking conversion here would report as a test failure in whatever
        // suite happened to trip it rather than as the impossible state it is.
        let total = i64::try_from(matches.len()).unwrap_or(i64::MAX);
        let offset = usize::try_from(offset.unwrap_or(0)).unwrap_or(0);
        let limit = limit.map_or(usize::MAX, |value| {
            usize::try_from(value).unwrap_or(usize::MAX)
        });
        Ok((
            matches.into_iter().skip(offset).take(limit).collect(),
            total,
        ))
    }

    async fn update_user_credential_set(&self, _set: &UserCredentialSet) -> Result<(), AppError> {
        Ok(())
    }

    async fn delete_user_credential_set(&self, _set_id: Uuid) -> Result<(), AppError> {
        Ok(())
    }

    async fn set_default_user_credential_set(
        &self,
        _user_id: Uuid,
        _protocol: ConnectionProtocol,
        _set_id: Uuid,
        _updated_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        Ok(())
    }
}

#[derive(Default)]
struct StubAuditRecorder;

#[async_trait]
impl AuditRecorder for StubAuditRecorder {
    async fn record_event(&self, _event: AuditEvent) -> Result<(), AppError> {
        Ok(())
    }
}

struct StubMaterializer {
    config: Value,
}

impl TargetMaterializer for StubMaterializer {
    async fn materialize_target_config(
        &self,
        _input: MaterializeTargetConfigInput<'_>,
    ) -> Result<MaterializedTargetConfig, AppError> {
        Ok(MaterializedTargetConfig {
            config: self.config.clone(),
            secret_values: Value::Null,
        })
    }

    async fn invalidate_target_cache(
        &self,
        _conn: &mut redis::aio::ConnectionManager,
        _target_id: Uuid,
    ) -> Result<(), AppError> {
        Ok(())
    }

    async fn load_target_variable_namespace(
        &self,
        _target_id: Uuid,
    ) -> Result<Option<Value>, AppError> {
        Ok(None)
    }
}

#[derive(Default)]
struct StubSecretRepository {
    set_bindings: HashMap<Uuid, HashMap<&'static str, Uuid>>,
    proxy_node_secret_bindings: HashMap<Uuid, Uuid>,
    host_secret_ids: Vec<Uuid>,
}

#[async_trait]
impl SecretRepository for StubSecretRepository {
    async fn apply_field_intent(
        &self,
        _input: piping_backend::contexts::secrets::application::ApplySecretFieldIntentInput,
    ) -> Result<piping_backend::contexts::secrets::application::SecretFieldState, AppError> {
        Err(AppError::Internal(
            "read-only credential-resolution fixture does not apply intents".into(),
        ))
    }
    async fn create_variable(&self, _variable: SecretVariable) -> Result<SecretVariable, AppError> {
        Err(AppError::Internal("not implemented".to_string()))
    }

    async fn get_variable_by_key(&self, _key: &str) -> Result<Option<SecretVariable>, AppError> {
        Ok(None)
    }

    async fn get_variable_by_id(
        &self,
        _secret_id: Uuid,
    ) -> Result<Option<SecretVariable>, AppError> {
        Ok(None)
    }

    async fn variable_exists(&self, _secret_id: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }

    async fn list_variables(&self, _key: Option<&str>) -> Result<Vec<SecretVariable>, AppError> {
        Ok(Vec::new())
    }

    async fn list_named(
        &self,
    ) -> Result<Vec<piping_backend::contexts::secrets::domain::NamedSecretSummary>, AppError> {
        Ok(Vec::new())
    }

    async fn create_named(
        &self,
        _key: &str,
        _description: Option<&str>,
        _created_by: Option<Uuid>,
    ) -> Result<piping_backend::contexts::secrets::domain::NamedSecretSummary, AppError> {
        Err(AppError::Internal("not implemented".to_string()))
    }

    async fn update_description(
        &self,
        _secret_id: Uuid,
        _description: Option<&str>,
    ) -> Result<Option<piping_backend::contexts::secrets::domain::NamedSecretSummary>, AppError>
    {
        Ok(None)
    }

    async fn delete_named(&self, _secret_id: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }

    async fn list_bindings_for_secret(
        &self,
        _secret_id: Uuid,
    ) -> Result<Vec<SecretBinding>, AppError> {
        Ok(Vec::new())
    }

    async fn list_bindings_for_scope(
        &self,
        _scope_type: &str,
        _scope_id: Uuid,
    ) -> Result<Vec<SecretBinding>, AppError> {
        Ok(Vec::new())
    }

    async fn list_bindings_for_scope_hierarchy(
        &self,
        _scope_type: &str,
        _scope_id: Uuid,
    ) -> Result<Vec<SecretBinding>, AppError> {
        Ok(Vec::new())
    }

    async fn list_scope_secrets(
        &self,
        _scope_type: &str,
        _scope_id: Uuid,
    ) -> Result<piping_backend::contexts::secrets::infrastructure::SecretScopeSecrets, AppError>
    {
        Ok(
            piping_backend::contexts::secrets::infrastructure::SecretScopeSecrets {
                variables: Vec::new(),
                bindings: Vec::new(),
            },
        )
    }

    async fn create_binding(&self, _binding: SecretBinding) -> Result<SecretBinding, AppError> {
        Err(AppError::Internal("not implemented".to_string()))
    }

    async fn delete_binding(&self, _binding_id: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }

    async fn get_usage_binding(
        &self,
        consumer_type: &str,
        consumer_id: Uuid,
        field_key: &str,
    ) -> Result<Option<SecretUsageBinding>, AppError> {
        if consumer_type == USER_CRED_SET_CONSUMER_TYPE {
            let Some(binding_map) = self.set_bindings.get(&consumer_id) else {
                return Ok(None);
            };
            let Some(secret_id) = binding_map.get(field_key).copied() else {
                return Ok(None);
            };
            return Ok(Some(SecretUsageBinding {
                id: Uuid::now_v7(),
                consumer_type: consumer_type.to_string(),
                consumer_id,
                field_key: field_key.to_string(),
                owner_scope_type: "user".to_string(),
                owner_scope_id: consumer_id,
                secret_id,
            }));
        }
        if consumer_type == PROXY_NODE_CREDENTIAL_CONSUMER_TYPE
            && field_key == FIELD_KEY_PROXY_NODE_SECRET
        {
            let Some(secret_id) = self.proxy_node_secret_bindings.get(&consumer_id).copied() else {
                return Ok(None);
            };
            return Ok(Some(SecretUsageBinding {
                id: Uuid::now_v7(),
                consumer_type: consumer_type.to_string(),
                consumer_id,
                field_key: field_key.to_string(),
                owner_scope_type: "proxy_node".to_string(),
                owner_scope_id: consumer_id,
                secret_id,
            }));
        }

        Ok(None)
    }

    async fn list_usage_bindings_for_consumer(
        &self,
        consumer_type: &str,
        consumer_id: Uuid,
    ) -> Result<Vec<SecretUsageBinding>, AppError> {
        if consumer_type == USER_CRED_SET_CONSUMER_TYPE {
            let Some(binding_map) = self.set_bindings.get(&consumer_id) else {
                return Ok(Vec::new());
            };
            return Ok(binding_map
                .iter()
                .map(|(field_key, secret_id)| SecretUsageBinding {
                    id: Uuid::now_v7(),
                    consumer_type: consumer_type.to_string(),
                    consumer_id,
                    field_key: (*field_key).to_string(),
                    owner_scope_type: "user".to_string(),
                    owner_scope_id: consumer_id,
                    secret_id: *secret_id,
                })
                .collect());
        }

        if consumer_type == "connection_scope:connection_target" {
            return Ok(self
                .host_secret_ids
                .iter()
                .map(|secret_id| SecretUsageBinding {
                    id: Uuid::now_v7(),
                    consumer_type: consumer_type.to_string(),
                    consumer_id,
                    field_key: "target_secret".to_string(),
                    owner_scope_type: "connection_target".to_string(),
                    owner_scope_id: consumer_id,
                    secret_id: *secret_id,
                })
                .collect());
        }

        Ok(Vec::new())
    }

    async fn list_usage_bindings_for_consumers(
        &self,
        consumer_type: &str,
        consumer_ids: &[Uuid],
    ) -> Result<Vec<SecretUsageBinding>, AppError> {
        let mut bindings = Vec::new();
        for consumer_id in consumer_ids {
            bindings.extend(
                self.list_usage_bindings_for_consumer(consumer_type, *consumer_id)
                    .await?,
            );
        }
        Ok(bindings)
    }

    async fn upsert_usage_binding(
        &self,
        _binding: SecretUsageBinding,
    ) -> Result<SecretUsageBinding, AppError> {
        Err(AppError::Internal("not implemented".to_string()))
    }

    async fn delete_usage_binding(&self, _binding_id: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }
}

#[derive(Clone, Default)]
struct StubSecretStore {
    values: Arc<Mutex<HashMap<Uuid, String>>>,
}

#[async_trait]
impl SecretStore for StubSecretStore {
    async fn get_secret(&self, secret_id: Uuid) -> Result<String, AppError> {
        let guard = self.values.lock().await;
        guard
            .get(&secret_id)
            .cloned()
            .ok_or_else(|| AppError::MissingSecret(format!("Secret not found: {}", secret_id)))
    }

    async fn set_secret(&self, _secret_id: Uuid, _value: &str) -> Result<(), AppError> {
        Ok(())
    }

    async fn delete_secret(&self, _secret_id: Uuid) -> Result<(), AppError> {
        Ok(())
    }

    async fn ensure_unique_key(
        &self,
        _scope_type: &str,
        _scope_id: Uuid,
        _key: &str,
    ) -> Result<(), AppError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct CapturingProxyClient {
    last_payload: Arc<Mutex<Option<Value>>>,
    last_proxy_base_url: Arc<Mutex<Option<Option<String>>>>,
    last_credential: Arc<Mutex<Option<Option<ProxyCredential>>>>,
}

impl CapturingProxyClient {
    async fn take_payload(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        self.last_payload
            .lock()
            .await
            .take()
            .ok_or_else(|| "payload captured".into())
    }

    async fn take_proxy_base_url(
        &self,
    ) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
        self.last_proxy_base_url
            .lock()
            .await
            .take()
            .ok_or_else(|| "proxy base URL captured".into())
    }

    async fn take_credential(
        &self,
    ) -> Result<Option<ProxyCredential>, Box<dyn std::error::Error + Send + Sync>> {
        self.last_credential
            .lock()
            .await
            .take()
            .ok_or_else(|| "proxy credential captured".into())
    }
}

impl ProxyClient for CapturingProxyClient {
    async fn create_connection(
        &self,
        payload: Value,
        proxy_base_url: Option<&str>,
        credential: Option<&ProxyCredential>,
    ) -> Result<ProxySession, AppError> {
        let mut guard = self.last_payload.lock().await;
        *guard = Some(payload);
        let mut proxy_base_url_guard = self.last_proxy_base_url.lock().await;
        *proxy_base_url_guard = Some(proxy_base_url.map(str::to_string));
        let mut credential_guard = self.last_credential.lock().await;
        *credential_guard = Some(credential.cloned());
        Ok(ProxySession { id: Uuid::now_v7() })
    }

    async fn terminate_connection(
        &self,
        _proxy_session_id: Uuid,
        _proxy_base_url: Option<&str>,
        _credential: Option<&ProxyCredential>,
    ) -> Result<(), AppError> {
        Ok(())
    }

    async fn get_metrics(
        &self,
        proxy_session_id: Uuid,
        _proxy_base_url: Option<&str>,
        _credential: Option<&ProxyCredential>,
    ) -> Result<ProxyMetricsSample, AppError> {
        Ok(ProxyMetricsSample {
            metrics: ProxyMetricsResponse {
                connection_id: proxy_session_id.to_string(),
                updated_at: Utc::now().to_rfc3339(),
                overall_status: "unknown".to_string(),
                links: Vec::new(),
                observed_host_keys: Vec::new(),
            },
            request_latency_ms: Some(1),
        })
    }
}

/// Stub connection-config record repo seeded from the test's `piping.connections`
/// envelope. `connconf_*` is the connect-time source, so these tests
/// drive that source by translating each envelope entry into a `connconf_record`.
#[derive(Default)]
struct StubConnConfRecordRepository {
    records: Vec<ConnConfRecord>,
}

#[async_trait]
impl ConnConfRecordRepository for StubConnConfRecordRepository {
    async fn list(&self, filter: ConnConfRecordFilter) -> Result<Vec<ConnConfRecord>, AppError> {
        Ok(self
            .records
            .iter()
            .filter(|record| {
                filter.protocol.is_empty() || filter.protocol.contains(&record.protocol)
            })
            .filter(|record| {
                filter.base_record_id.is_empty()
                    || record
                        .base_record_id
                        .is_some_and(|id| filter.base_record_id.contains(&id))
            })
            .filter(|record| {
                filter.owner_user_id.is_empty()
                    || record
                        .owner_user_id
                        .is_some_and(|id| filter.owner_user_id.contains(&id))
            })
            .filter(|record| {
                filter.enabled.is_empty()
                    || filter.enabled.iter().any(|value| match value.as_str() {
                        "true" => record.enabled,
                        "false" => !record.enabled,
                        _ => false,
                    })
            })
            .cloned()
            .collect())
    }

    async fn list_all(&self) -> Result<Vec<ConnConfRecord>, AppError> {
        Ok(self.records.clone())
    }

    async fn list_by_protocol(
        &self,
        protocol: ConnConfProtocol,
    ) -> Result<Vec<ConnConfRecord>, AppError> {
        Ok(self
            .records
            .iter()
            .filter(|record| record.protocol == protocol)
            .cloned()
            .collect())
    }

    async fn list_by_protocol_with_bases(
        &self,
        protocol: ConnConfProtocol,
    ) -> Result<Vec<ConnConfRecord>, AppError> {
        let mut selected_ids: HashSet<Uuid> = self
            .records
            .iter()
            .filter(|record| record.protocol == protocol)
            .map(|record| record.id)
            .collect();

        let mut pending_base_ids: Vec<Uuid> = self
            .records
            .iter()
            .filter(|record| selected_ids.contains(&record.id))
            .filter_map(|record| record.base_record_id)
            .collect();

        while let Some(base_id) = pending_base_ids.pop() {
            if !selected_ids.insert(base_id) {
                continue;
            }
            if let Some(base) = self.records.iter().find(|record| record.id == base_id)
                && let Some(parent_id) = base.base_record_id
            {
                pending_base_ids.push(parent_id);
            }
        }

        Ok(self
            .records
            .iter()
            .filter(|record| selected_ids.contains(&record.id))
            .cloned()
            .collect())
    }

    async fn find(&self, id: Uuid) -> Result<Option<ConnConfRecord>, AppError> {
        Ok(self.records.iter().find(|r| r.id == id).cloned())
    }

    async fn has_direct_children(&self, id: Uuid) -> Result<bool, AppError> {
        Ok(self
            .records
            .iter()
            .any(|record| record.base_record_id == Some(id)))
    }

    async fn create(&self, _input: &NewConnConfRecord) -> Result<ConnConfRecord, AppError> {
        Err(AppError::Internal("not implemented".to_string()))
    }

    async fn update(
        &self,
        _id: Uuid,
        _input: &UpdateConnConfRecord,
    ) -> Result<Option<ConnConfRecord>, AppError> {
        Err(AppError::Internal("not implemented".to_string()))
    }

    async fn delete(&self, _id: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }
}

/// Stub assignment repo. Every seeded record is bound at `global` scope, so it
/// applies to any target; `get_device_scopes` returns the queried target so the
/// resolver proceeds. Entry order is preserved as ascending `weight` (later
/// envelope entries win), mirroring the legacy entry-index precedence.
#[derive(Default)]
struct StubConnConfAssignmentRepository {
    assignments: Vec<ConnConfAssignment>,
}

#[async_trait]
impl ConnConfAssignmentRepository for StubConnConfAssignmentRepository {
    async fn list(
        &self,
        filter: ConnConfAssignmentFilter,
    ) -> Result<Vec<ConnConfAssignment>, AppError> {
        Ok(self
            .assignments
            .iter()
            .filter(|assignment| {
                filter.record_id.is_empty() || filter.record_id.contains(&assignment.record_id)
            })
            .filter(|assignment| {
                filter.scope_type.is_empty() || filter.scope_type.contains(&assignment.scope_type)
            })
            .filter(|assignment| {
                filter.scope_id.is_empty()
                    || assignment
                        .scope_id
                        .is_some_and(|id| filter.scope_id.contains(&id))
            })
            .filter(|assignment| {
                filter.enabled.is_empty()
                    || filter.enabled.iter().any(|value| match value.as_str() {
                        "true" => assignment.enabled,
                        "false" => !assignment.enabled,
                        _ => false,
                    })
            })
            .cloned()
            .collect())
    }

    async fn list_all(&self) -> Result<Vec<ConnConfAssignment>, AppError> {
        Ok(self.assignments.clone())
    }

    async fn list_for_record(&self, record_id: Uuid) -> Result<Vec<ConnConfAssignment>, AppError> {
        Ok(self
            .assignments
            .iter()
            .filter(|a| a.record_id == record_id)
            .cloned()
            .collect())
    }

    async fn find(&self, id: Uuid) -> Result<Option<ConnConfAssignment>, AppError> {
        Ok(self.assignments.iter().find(|a| a.id == id).cloned())
    }

    async fn create(&self, _input: &NewConnConfAssignment) -> Result<ConnConfAssignment, AppError> {
        Err(AppError::Internal("not implemented".to_string()))
    }

    async fn update(
        &self,
        _id: Uuid,
        _input: &UpdateConnConfAssignment,
    ) -> Result<Option<ConnConfAssignment>, AppError> {
        Err(AppError::Internal("not implemented".to_string()))
    }

    async fn delete(&self, _id: Uuid) -> Result<bool, AppError> {
        Ok(false)
    }

    async fn list_for_scopes(
        &self,
        _scopes: &[(ConnConfScope, Option<Uuid>)],
    ) -> Result<Vec<ConnConfAssignment>, AppError> {
        // All seeded assignments are global; return them all (the resolver
        // re-checks scope membership defensively).
        Ok(self.assignments.clone())
    }

    async fn get_device_scopes(&self, target_id: Uuid) -> Result<Option<DeviceScopeIds>, AppError> {
        Ok(Some(DeviceScopeIds {
            device_id: Some(target_id),
            ..DeviceScopeIds::default()
        }))
    }
}

#[tokio::test]
async fn stub_connconf_record_list_by_protocol_with_bases_includes_base_chain() {
    let base_id = Uuid::now_v7();
    let child_id = Uuid::now_v7();
    let unrelated_id = Uuid::now_v7();
    let now = Utc::now();
    let record = |id: Uuid,
                  name: &str,
                  protocol: ConnConfProtocol,
                  base_record_id: Option<Uuid>| ConnConfRecord {
        id,
        name: name.to_string(),
        description: String::new(),
        protocol,
        base_record_id,
        enabled: true,
        owner_user_id: None,
        params: json!({ "name": name }),
        custom_field_data: json!({}),
        created_at: now,
        updated_at: now,
    };
    let repo = StubConnConfRecordRepository {
        records: vec![
            record(base_id, "ssh-base", ConnConfProtocol::Ssh, None),
            record(child_id, "ssh-child", ConnConfProtocol::Ssh, Some(base_id)),
            record(unrelated_id, "rdp-record", ConnConfProtocol::Rdp, None),
        ],
    };

    let records = repo
        .list_by_protocol_with_bases(ConnConfProtocol::Ssh)
        .await
        .expect("list records");
    let ids: HashSet<Uuid> = records.into_iter().map(|record| record.id).collect();

    assert!(ids.contains(&base_id), "base record must be loaded");
    assert!(ids.contains(&child_id), "protocol record must be loaded");
    assert!(
        !ids.contains(&unrelated_id),
        "unrelated protocol records must stay unloaded"
    );
}

/// Translate a test `piping.connections` envelope into seeded connection-config
/// records + global assignments (one per protocol entry, weight = entry index).
fn seed_connconf_from_envelope(
    host_config: &Value,
) -> (
    StubConnConfRecordRepository,
    StubConnConfAssignmentRepository,
) {
    let mut records = Vec::new();
    let mut assignments = Vec::new();

    let connections = host_config
        .get("piping")
        .and_then(|p| p.get("connections"))
        .and_then(Value::as_object);

    if let Some(connections) = connections {
        for (protocol_key, entries) in connections {
            let Some(protocol) = ConnConfProtocol::parse(protocol_key) else {
                continue;
            };
            let entry_list: Vec<Value> = match entries {
                Value::Array(items) => items.clone(),
                Value::Object(_) => vec![entries.clone()],
                _ => continue,
            };
            for (index, params) in entry_list.into_iter().enumerate() {
                let record_id = Uuid::now_v7();
                records.push(ConnConfRecord {
                    id: record_id,
                    name: format!("{protocol_key}-{index}"),
                    description: String::new(),
                    protocol,
                    base_record_id: None,
                    enabled: true,
                    owner_user_id: None,
                    params,
                    custom_field_data: json!({}),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                });
                assignments.push(ConnConfAssignment {
                    id: Uuid::now_v7(),
                    record_id,
                    scope_type: ConnConfScope::Global,
                    scope_id: None,
                    weight: index as i32,
                    enabled: true,
                    created_at: Utc::now(),
                });
            }
        }
    }

    (
        StubConnConfRecordRepository { records },
        StubConnConfAssignmentRepository { assignments },
    )
}

#[allow(
    clippy::expect_used,
    reason = "fixture helper outside #[test]: unreadable settings, cipher material or a zero TTL must fail the calling lifecycle test"
)]
fn build_connection_service(
    connection_repository: Arc<StubConnectionRepository>,
    proxy_client: Arc<CapturingProxyClient>,
    host_config: Value,
    secret_repository: Arc<StubSecretRepository>,
    secret_store: Arc<StubSecretStore>,
) -> TestConnectionService {
    let mut settings = piping_backend::config::load_settings().expect("test settings");
    common::configure_test_cipher(&mut settings);
    piping_backend::contexts::secrets::infrastructure::crypto::configure_cipher_key_material_ref(
        &settings.secrets.cipher.key_material_ref,
    )
    .expect("test cipher");
    let (connconf_records, connconf_assignments) = seed_connconf_from_envelope(&host_config);
    ConnectionService::new(ConnectionServiceDeps {
        auth_session_ttl_seconds: std::num::NonZeroU64::new(
            piping_backend::config::AuthConfig::default().session_ttl_seconds,
        )
        .expect("positive default TTL"),
        repository: connection_repository,
        proxy_client,
        audit_service: Arc::new(StubAuditRecorder),
        materializer: Arc::new(StubMaterializer {
            config: host_config,
        }),
        secret_repository,
        secret_store,
        connconf_records: Arc::new(connconf_records),
        connconf_assignments: Arc::new(connconf_assignments),
    })
}

fn build_connection_service_with_defaults(
    proxy_client: Arc<CapturingProxyClient>,
    host_config: Value,
) -> TestConnectionService {
    build_connection_service(
        Arc::new(StubConnectionRepository::default()),
        proxy_client,
        host_config,
        Arc::new(StubSecretRepository::default()),
        Arc::new(StubSecretStore::default()),
    )
}

struct ConnectionServiceFixture {
    service: TestConnectionService,
    proxy_client: Arc<CapturingProxyClient>,
}

struct ConnectionServiceTestData {
    host_config: Value,
    user_sets: HashMap<Uuid, UserCredentialSet>,
    set_bindings: HashMap<Uuid, HashMap<&'static str, Uuid>>,
    secret_values: HashMap<Uuid, String>,
    host_secret_ids: Vec<Uuid>,
}

impl ConnectionServiceTestData {
    fn new(host_config: Value) -> Self {
        Self {
            host_config,
            user_sets: HashMap::new(),
            set_bindings: HashMap::new(),
            secret_values: HashMap::new(),
            host_secret_ids: Vec::new(),
        }
    }

    fn with_user_set(mut self, set: UserCredentialSet) -> Self {
        self.user_sets.insert(set.id, set);
        self
    }

    fn with_bindings(mut self, set_id: Uuid, bindings: HashMap<&'static str, Uuid>) -> Self {
        self.set_bindings.insert(set_id, bindings);
        self
    }

    fn with_secret_value(mut self, secret_id: Uuid, value: impl Into<String>) -> Self {
        self.secret_values.insert(secret_id, value.into());
        self
    }

    fn with_host_secret_ids(mut self, ids: impl IntoIterator<Item = Uuid>) -> Self {
        self.host_secret_ids.extend(ids);
        self
    }
}

fn build_connection_service_fixture(data: ConnectionServiceTestData) -> ConnectionServiceFixture {
    let proxy_client = Arc::new(CapturingProxyClient::default());
    let service = build_connection_service(
        Arc::new(StubConnectionRepository {
            user_sets: data.user_sets,
            ..StubConnectionRepository::default()
        }),
        proxy_client.clone(),
        data.host_config,
        Arc::new(StubSecretRepository {
            set_bindings: data.set_bindings,
            host_secret_ids: data.host_secret_ids,
            ..Default::default()
        }),
        Arc::new(StubSecretStore {
            values: Arc::new(Mutex::new(data.secret_values)),
        }),
    );

    ConnectionServiceFixture {
        service,
        proxy_client,
    }
}

fn test_user_set(
    set_id: Uuid,
    user_id: Uuid,
    protocol: ConnectionProtocol,
    name: &str,
    params: Value,
    is_default: bool,
) -> UserCredentialSet {
    UserCredentialSet {
        id: set_id,
        user_id,
        protocol,
        name: name.to_string(),
        params,
        is_default,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn session_metadata_with_entry_id(entry_id: impl Into<String>) -> Value {
    json!({ SESSION_METADATA_ENTRY_ID_KEY: entry_id.into() })
}

fn session_metadata_with_policy(entry_id: impl Into<String>, merge_policy: &str) -> Value {
    json!({
        SESSION_METADATA_ENTRY_ID_KEY: entry_id.into(),
        SESSION_METADATA_MERGE_POLICY_KEY: merge_policy
    })
}

async fn create_session_for_test(
    service: &TestConnectionService,
    target_id: Uuid,
    user_id: Uuid,
    protocol: ConnectionProtocol,
    session_metadata: Option<Value>,
) -> Result<ConnectionSession, AppError> {
    service
        .create_session(
            CreateSessionInput {
                target_id,
                user_id,
                protocol,
                client_ip: None,
                audit_auth_method: None,
                audit_api_token_id: None,
                session_metadata,
                template_id: None,
                protocol_params: None,
                requested_proxy_node_id: None,
                require_embedded_proxy: false,
            },
            None,
            None,
        )
        .await
}

#[tokio::test]
async fn scoped_proxy_assignment_routes_ssh_rdp_and_vnc_through_proxy_node()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let target_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    let proxy_node_id = Uuid::now_v7();
    let proxy_secret_id = Uuid::now_v7();
    let proxy_secret = "per-node-proxy-secret";
    let proxy_base_url = "http://edge-proxy:7100";
    let now = Utc::now();
    let proxy_node = ProxyNode {
        id: proxy_node_id,
        name: "Edge proxy".to_string(),
        slug: "edge-proxy".to_string(),
        base_url: proxy_base_url.to_string(),
        enabled: true,
        secret_version: 1,
        secret_rotated_at: Some(now),
        created_at: now,
        updated_at: now,
    };
    let proxy_assignment = ProxyNodeAssignment {
        id: Uuid::now_v7(),
        proxy_node_id,
        scope_type: ConnConfScope::Device,
        scope_id: Some(target_id),
        weight: 0,
        enabled: true,
        created_at: now,
        updated_at: now,
    };
    let proxy_client = Arc::new(CapturingProxyClient::default());
    let service = build_connection_service(
        Arc::new(StubConnectionRepository {
            user_sets: HashMap::new(),
            proxy_nodes: HashMap::from([(proxy_node_id, proxy_node)]),
            proxy_assignments: vec![proxy_assignment],
        }),
        proxy_client.clone(),
        json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": LOOPBACK_HOST,
                        "port": SSH_PORT_DEFAULT,
                        "credentials": { "username": "root", "password": "secret" }
                    }],
                    "rdp": [{
                        "host": LOOPBACK_HOST,
                        "port": RDP_PORT_DEFAULT,
                        "credentials": { "username": "operator", "password": "secret" }
                    }],
                    "vnc": [{
                        "host": LOOPBACK_HOST,
                        "port": VNC_PORT_DEFAULT,
                        "credentials": { "password": "secret" }
                    }]
                }
            }
        }),
        Arc::new(StubSecretRepository {
            proxy_node_secret_bindings: HashMap::from([(proxy_node_id, proxy_secret_id)]),
            ..Default::default()
        }),
        Arc::new(StubSecretStore {
            values: Arc::new(Mutex::new(HashMap::from([(
                proxy_secret_id,
                proxy_secret.to_string(),
            )]))),
        }),
    );

    for protocol in [
        ConnectionProtocol::Ssh,
        ConnectionProtocol::Rdp,
        ConnectionProtocol::Vnc,
    ] {
        let session = create_session_for_test(&service, target_id, user_id, protocol.clone(), None)
            .await
            .expect(SESSION_CREATED_EXPECTATION);

        assert_eq!(session.proxy_node_id, Some(proxy_node_id));
        assert_eq!(session.protocol, protocol);
        assert_eq!(session.routing_source, RoutingSource::ProxyAssignment);
        assert_eq!(
            proxy_client.take_proxy_base_url().await?.as_deref(),
            Some(proxy_base_url)
        );
        let credential = proxy_client
            .take_credential()
            .await?
            .expect("per-node proxy credential");
        assert_eq!(credential.node_id, proxy_node_id.to_string());
        assert_eq!(credential.secret, proxy_secret);
        let _payload = proxy_client.take_payload().await?;
    }
    Ok(())
}

#[path = "integration_user_owned_connection_credentials_secret_lifecycle/session_merge_tests.rs"]
mod session_merge_tests;
#[path = "integration_user_owned_connection_credentials_secret_lifecycle/validation_tests.rs"]
mod validation_tests;
