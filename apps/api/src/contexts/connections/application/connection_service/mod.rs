//! Connection application service for sessions, proxy nodes and routing.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;

use chrono::Utc;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::routing_decision_service::{ConnectionRoutingDecision, RoutingDecisionService};
use crate::contexts::audit::application::AuditRecorder;
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::connection_config::{
    ConnConfAssignment, ConnConfAssignmentRepository, ConnConfProtocol, ConnConfRecord,
    ConnConfRecordRepository, ConnConfScope, LaunchAccount, ResolveInput, resolve_launch_accounts,
};
use crate::contexts::connections::domain::{
    ConnectionLaunchParams, ConnectionProtocol, ConnectionSession, ConnectionStatus,
    ConnectionTemplate, NewProxyNodeAssignment, ProxyNode, ProxyNodeAssignment,
    TargetProxyAssignment, USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_PASSWORD,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_USERNAME,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PASSWORD,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PRIVATE_KEY,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_VNC_PASSWORD,
    USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE, UpdateProxyNodeAssignment, UserCredentialSet,
};
use crate::contexts::connections::infrastructure::persistence::ConnectionRepository;
use crate::contexts::connections::infrastructure::{
    ConnectionSessionFilter, ConnectionTemplateFilter, ProxyNodeAssignmentFilter, ProxyNodeFilter,
    UserCredentialSetFilter,
};
use crate::contexts::inventory::application::TargetMaterializer;
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, PROXY_NODE_CREDENTIAL_CONSUMER_TYPE, SecretFieldIntent,
    SecretFieldIntentService, SecretStore, SecretVisibilityChecker,
};
use crate::contexts::secrets::infrastructure::persistence::SecretRepository;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::integrations::proxy::{
    PROXY_NODE_ID_ENV_KEY, PROXY_NODE_SECRET_ENV_KEY, ProxyClient, ProxyCredential,
    generate_proxy_secret_material,
};
use crate::observability::{
    EventFields, LogLevel, instrument_span, log_connection_event, log_error_event, log_event,
    operation_span, proxy_call_span,
};

mod connconf_resolve;
mod entry_selector;
mod lifecycle;
mod logging;
mod proxy_payload;
mod routing;
mod session_credentials;
mod sessions;
mod validation;
pub(crate) use proxy_payload::normalize_launch_params;

#[cfg(test)]
mod tests;

use entry_selector::{EntrySelectorKind, parse_connection_entry_selector};
use logging::summarize_payload_for_log;
use routing::{resolve_primary_endpoint, substitute_config_placeholders};
use validation::{
    ENDPOINT_REASON_NOT_BOUND_TO_TARGET, ENDPOINT_REASON_UNRESOLVED, endpoint_validation_error,
};

pub struct ConnectionService<R, P, A, M, SR, SS>
where
    R: ConnectionRepository,
    P: ProxyClient,
    A: AuditRecorder,
    M: TargetMaterializer,
    SR: SecretRepository,
    SS: SecretStore,
{
    repository: Arc<R>,
    proxy_client: Arc<P>,
    routing_decisions: Arc<RoutingDecisionService<R>>,
    audit_service: Arc<A>,
    materializer: Arc<M>,
    secret_repository: Arc<SR>,
    secret_store: Arc<SS>,
    session_credentials: session_credentials::SessionCredentials,
    /// Connection-config (`connconf_*`) repositories: the canonical source for
    /// connect-time effective config. The connect
    /// path resolves effective config from these and no longer reads the
    /// `piping.connections` config-context envelope.
    connconf_records: Arc<dyn ConnConfRecordRepository>,
    connconf_assignments: Arc<dyn ConnConfAssignmentRepository>,
}

/// Constructor dependencies for [`ConnectionService`], grouped so the
/// constructor stays within the argument-count bound. Field names mirror the
/// stored service fields.
pub struct ConnectionServiceDeps<R, P, A, M, SR, SS> {
    pub repository: Arc<R>,
    pub proxy_client: Arc<P>,
    pub audit_service: Arc<A>,
    pub materializer: Arc<M>,
    pub secret_repository: Arc<SR>,
    pub secret_store: Arc<SS>,
    pub auth_session_ttl_seconds: std::num::NonZeroU64,
    pub connconf_records: Arc<dyn ConnConfRecordRepository>,
    pub connconf_assignments: Arc<dyn ConnConfAssignmentRepository>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum UserCredentialSetMergePolicy {
    FillMissing,
    OverrideExisting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialMergePreference {
    ContextFirst,
    PersonalFirst,
}

#[derive(Debug, Clone)]
pub struct CreateProxyNodeInput {
    pub name: String,
    pub slug: String,
    pub base_url: String,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct ProxyNodeBootstrap {
    pub node_id: String,
    pub node_slug: String,
    pub node_secret: String,
    pub env: BTreeMap<String, String>,
    pub docker_run_command: String,
}

#[derive(Debug, Clone)]
pub struct CreatedProxyNode {
    pub node: ProxyNode,
    pub bootstrap: ProxyNodeBootstrap,
}

#[derive(Debug, Clone)]
pub struct ConnectionProxyRoute {
    pub proxy_base_url: Option<String>,
    pub credential: Option<ProxyCredential>,
}

#[derive(Debug, Clone)]
pub struct UpdateProxyNodeInput {
    pub name: String,
    pub slug: String,
    pub base_url: String,
    pub enabled: bool,
}

fn overlay_endpoint_aliases_from_target_namespace(
    resolution_context: &mut Map<String, Value>,
    namespace: &Value,
) {
    let Value::Object(target) = namespace else {
        return;
    };

    for key in ["primary_ip4", "primary_ip6"] {
        let current_is_missing = resolution_context
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .is_none_or(str::is_empty);
        if !current_is_missing {
            continue;
        }

        let Some(value) = target
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        resolution_context.insert(key.to_string(), Value::String(value.to_string()));
    }
}

#[derive(Debug, Clone)]
pub struct CreateProxyNodeAssignmentInput {
    pub proxy_node_id: Uuid,
    pub scope_type: ConnConfScope,
    pub scope_id: Option<Uuid>,
    pub weight: Option<i32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateProxyNodeAssignmentInput {
    pub proxy_node_id: Option<Uuid>,
    pub scope_type: Option<ConnConfScope>,
    pub scope_id: Option<Uuid>,
    pub weight: Option<i32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct CreateSessionInput {
    pub target_id: Uuid,
    pub user_id: Uuid,
    pub protocol: ConnectionProtocol,
    pub client_ip: Option<String>,
    pub audit_auth_method: Option<String>,
    pub audit_api_token_id: Option<Uuid>,
    pub session_metadata: Option<Value>,
    pub template_id: Option<Uuid>,
    pub protocol_params: Option<ConnectionLaunchParams>,
    pub requested_proxy_node_id: Option<Uuid>,
    pub require_embedded_proxy: bool,
}

#[derive(Debug, Clone)]
pub struct ResolveSshExecutionLaunchInput {
    pub execution_id: Uuid,
    pub target_id: Uuid,
    pub credential_user_id: Option<Uuid>,
    pub session_metadata: Option<Value>,
    pub protocol_params: Option<ConnectionLaunchParams>,
    pub requested_proxy_node_id: Option<Uuid>,
}

/// In-memory SSH launch material for non-interactive workers.
///
/// `proxy_payload` contains resolved credentials and must never be persisted or
/// returned by REST/MCP handlers. It exists so durable workers can reuse the
/// same connection-config, user credential set, secret fence, endpoint-bound,
/// and proxy-routing hardening as interactive SSH without creating a browser
/// connection session.
#[derive(Debug, Clone)]
pub struct ResolvedSshExecutionLaunch {
    pub target_id: Uuid,
    pub credential_user_id: Option<Uuid>,
    pub endpoint_host: String,
    pub routing_decision: ConnectionRoutingDecision,
    pub proxy_payload: Value,
}

/// Resolved connection-config (`connconf_*`) source for one connect attempt.
struct ConnConfLaunchResolution {
    /// Launch-param fields in the shape `build_proxy_payload` consumes.
    fields: ConnectionLaunchParams,
    /// Per-secret-UUID attribution: each candidate secret leaf maps to the
    /// `(record_id, field_key)` sources that produced it. The fence resolves a
    /// UUID only when one of those records has a matching
    /// `connection_config:record` binding — never a blanket trust.
    secret_attributions: HashMap<Uuid, Vec<connconf_resolve::SecretLeafAttribution>>,
}

/// Resolved launch options for one device + protocol: the ordered shared
/// accounts (Option A, non-merging) plus the caller's personal credential sets
/// only when a connection-config record makes this protocol available for the
/// target.
///
/// Returned in-process to the REST handler, which redacts secret leaves down to
/// presence-only before serialization (secrets are NEVER returned over the
/// wire). The ordering of [`Self::shared_accounts`] is the entry_id index
/// contract: index `i` maps to `config:connconf:<proto>:<i>`.
pub struct ConnectionLaunchOptions {
    /// Ordered shared launch accounts from `resolve_launch_accounts`. The slice
    /// index is the `config:connconf:<proto>:<index>` entry index.
    pub shared_accounts: Vec<LaunchAccount>,
    /// The caller's personal credential sets for this protocol, in the order the
    /// repository returns them. This is empty when no shared connection-config
    /// account applies; personal sets are credential overlays, not a source of
    /// target/protocol availability. The slice index is the
    /// `user:connconf:<proto>:<index>:<uuid>` entry index.
    pub personal_credential_sets: Vec<UserCredentialSet>,
}

/// The protocol-INDEPENDENT DB inputs the shared launch-account resolver needs
/// for one target: its scope ancestry, the assignments matching those scopes,
/// and the connection-config records.
///
/// None of the three depends on the protocol, so a caller resolving several
/// protocols for the same target (the launch-options endpoint walks all five)
/// loads this ONCE via
/// [`ConnectionService::load_shared_launch_inputs`] and reuses it, instead of
/// repeating the three round-trips per protocol.
pub struct SharedLaunchInputs {
    device_scopes: Vec<(ConnConfScope, Option<Uuid>)>,
    assignments: Vec<ConnConfAssignment>,
    records: Vec<ConnConfRecord>,
}

#[derive(Debug, Clone)]
struct LaunchCredentialSelection {
    /// Entry index from any (`config:`/`user:`) selector matching the protocol.
    /// Kind-agnostic; gates entry-driven override validation only.
    selected_entry_index: Option<usize>,
    /// Shared-account index, set ONLY for a `config:` selector. This is the index
    /// into the ordered launch-account list (the entry_id ordering contract); a
    /// `user:` selector's index addresses a personal credential set, NOT a shared
    /// account, so it must not drive shared-account selection.
    selected_config_entry_index: Option<usize>,
    /// Legacy `user:config-context:*` selectors encoded the shared entry index
    /// plus a personal credential set id. If there are no shared entries at all,
    /// fall back to the personal set instead of treating index 0 as invalid.
    allow_missing_selected_config_entry: bool,
    selected_user_credential_set: Option<UserCredentialSet>,
    selected_user_credential_set_params: Option<ConnectionLaunchParams>,
    credential_set_merge_policy: UserCredentialSetMergePolicy,
}

/// Select ONE launch account from the ordered shared-account list, honoring the
/// entry_id ordering contract (index = position in the ordered `Vec`, see
/// `connection_config::resolve_launch_accounts`).
///
/// * `Some(i)`: the account at index `i`, or [`AppError::Validation`] when `i`
///   is out of range (never an out-of-bounds panic).
/// * `None` (default launch): the account at index 0 when accounts exist. With
///   no identity accounts the resolver returns exactly ONE fallback account
///   equal to the merged config, so default selection of index 0 reproduces the
///   single-config connect behavior with no regression.
///
/// Returns `Ok(None)` only when the account list is empty (no `connconf` config
/// applies). Personal credential sets are identity overlays, not protocol
/// availability; callers must not treat this as permission to launch a
/// personal-only session.
/// Pure: no I/O, so the selection logic is unit-testable without a database.
fn select_launch_account(
    accounts: &[LaunchAccount],
    selected_entry_index: Option<usize>,
) -> Result<Option<&LaunchAccount>, AppError> {
    match selected_entry_index {
        Some(index) => accounts.get(index).map(Some).ok_or_else(|| {
            AppError::Validation(format!(
                "Selected connection-config account index {index} is out of range (have {})",
                accounts.len()
            ))
        }),
        None => Ok(accounts.first()),
    }
}

fn merge_launch_params_map_values(
    base: &mut ConnectionLaunchParams,
    overlay: ConnectionLaunchParams,
    merge_policy: UserCredentialSetMergePolicy,
) {
    for (key, value) in overlay {
        if matches!(key.as_str(), "credentials" | "options") {
            match (base.get_mut(&key), value) {
                (Some(Value::Object(base_object)), Value::Object(overlay_object)) => {
                    for (child_key, child_value) in overlay_object {
                        if merge_policy == UserCredentialSetMergePolicy::OverrideExisting
                            || !base_object.contains_key(&child_key)
                        {
                            base_object.insert(child_key, child_value);
                        }
                    }
                    continue;
                }
                (_, replacement) => {
                    base.insert(key, replacement);
                    continue;
                }
            }
        }

        if merge_policy == UserCredentialSetMergePolicy::OverrideExisting
            || !base.contains_key(&key)
        {
            base.insert(key, value);
        }
    }
}

fn merge_required_connconf_params(
    protocol: &ConnectionProtocol,
    host_default_params: Option<ConnectionLaunchParams>,
    user_set_params: Option<ConnectionLaunchParams>,
    merge_policy: UserCredentialSetMergePolicy,
) -> Result<ConnectionLaunchParams, AppError> {
    match (host_default_params, user_set_params) {
        (Some(mut defaults), Some(user_set_params)) => {
            merge_launch_params_map_values(&mut defaults, user_set_params, merge_policy);
            Ok(defaults)
        }
        (Some(defaults), None) => Ok(defaults),
        (None, _) => Err(AppError::Validation(format!(
            "No default {} connection config found in host config",
            protocol.as_str()
        ))),
    }
}

fn filter_client_credentials(
    protocol: &ConnectionProtocol,
    mut credentials: Map<String, Value>,
) -> Map<String, Value> {
    // Minimize what leaves the server (constitution 1 + 3.3): only what the browser-side
    // client must know for protocol auth.
    let allowed: &[&str] = match protocol {
        ConnectionProtocol::Vnc => &["password"],
        ConnectionProtocol::Rdp => &["username", "password", "domain"],
        ConnectionProtocol::Ssh => &["username", "password", "private_key"],
        ConnectionProtocol::App | ConnectionProtocol::Cctv => &[],
    };

    credentials.retain(|key, _| allowed.contains(&key.as_str()));
    credentials
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
    pub fn new(deps: ConnectionServiceDeps<R, P, A, M, SR, SS>) -> Self {
        let ConnectionServiceDeps {
            repository,
            proxy_client,
            audit_service,
            materializer,
            secret_repository,
            secret_store,
            auth_session_ttl_seconds,
            connconf_records,
            connconf_assignments,
        } = deps;
        let routing_decisions = Arc::new(RoutingDecisionService::new(repository.clone()));
        Self {
            repository,
            proxy_client,
            routing_decisions,
            audit_service,
            materializer,
            secret_repository,
            secret_store,
            session_credentials: session_credentials::SessionCredentials::new(
                auth_session_ttl_seconds,
            ),
            connconf_records,
            connconf_assignments,
        }
    }

    fn connconf_protocol(protocol: &ConnectionProtocol) -> Option<ConnConfProtocol> {
        match protocol {
            ConnectionProtocol::Ssh => Some(ConnConfProtocol::Ssh),
            ConnectionProtocol::Rdp => Some(ConnConfProtocol::Rdp),
            ConnectionProtocol::Vnc => Some(ConnConfProtocol::Vnc),
            ConnectionProtocol::App => Some(ConnConfProtocol::App),
            ConnectionProtocol::Cctv => Some(ConnConfProtocol::Cctv),
        }
    }

    async fn proxy_assignment_scopes(
        &self,
        target_id: Uuid,
    ) -> Result<Vec<(ConnConfScope, Option<Uuid>)>, AppError> {
        let Some(device_scope_ids) = self
            .connconf_assignments
            .get_device_scopes(target_id)
            .await?
        else {
            return Ok(vec![(ConnConfScope::Global, None)]);
        };
        Ok(connconf_resolve::build_device_scopes(&device_scope_ids))
    }

    /// Resolve the connection-config (`connconf_*`) launch params for a target +
    /// protocol, honoring the SELECTED account. It builds the SAME ordered
    /// shared-account list the read
    /// endpoint exposes (via [`Self::resolve_shared_launch_accounts`] — the one
    /// resolver that owns the entry_id ordering contract), selects ONE account
    /// from it, then maps that account's params + per-leaf attribution onto
    /// launch-param fields (strips `ui`, preserves the `jump` chain, pairs each
    /// bare-UUID secret leaf with its contributing record so the connect-time
    /// fence can enforce per-record bindings).
    ///
    /// Selection (see the entry_id ordering contract in
    /// `connection_config::resolve_launch_accounts`):
    /// * `selected_entry_index = Some(i)` (a `config:…` selector): account `i`.
    ///   An out-of-range index is a [`AppError::Validation`], never a panic.
    /// * `None` (default launch): account at index 0. When there are no identity
    ///   accounts, the resolver returns exactly ONE fallback account equal
    ///   to today's merged config, so index 0 reproduces the prior single-config
    ///   behavior with no regression.
    ///
    /// Returns `Ok(None)` when no `connconf` config applies to the target (no
    /// matching records), so the caller can fall back to user credential sets.
    /// The returned secret ids must be allowed through the connect-time fence.
    async fn resolve_connconf_launch_params(
        &self,
        target_id: Uuid,
        protocol: &ConnectionProtocol,
        selected_entry_index: Option<usize>,
        allow_missing_selected_entry: bool,
    ) -> Result<Option<ConnConfLaunchResolution>, AppError> {
        let accounts = self
            .resolve_shared_launch_accounts(target_id, protocol)
            .await?;

        // No participating records -> no connconf config for this target/protocol.
        if accounts.is_empty() && selected_entry_index.is_some() && allow_missing_selected_entry {
            return Ok(None);
        }
        let Some(account) = select_launch_account(&accounts, selected_entry_index)? else {
            return Ok(None);
        };

        let Some(mapped) =
            connconf_resolve::map_effective_params(&account.params, &account.attribution)
        else {
            return Ok(None);
        };
        Ok(Some(ConnConfLaunchResolution {
            fields: mapped.fields,
            secret_attributions: mapped.secret_attributions,
        }))
    }

    fn resolve_config_placeholders_in_launch_params(
        params: Option<ConnectionLaunchParams>,
        resolution_context: &Value,
    ) -> Result<Option<ConnectionLaunchParams>, AppError> {
        let Some(params) = params else {
            return Ok(None);
        };

        let mut value = Value::Object(params);
        substitute_config_placeholders(&mut value, resolution_context)?;
        let Value::Object(map) = value else {
            return Err(AppError::Internal(
                "Failed to normalize launch params after placeholder substitution".to_string(),
            ));
        };
        Ok(Some(map))
    }

    fn launch_params_have_endpoint_host(params: Option<&ConnectionLaunchParams>) -> bool {
        params
            .and_then(|params| params.get("host"))
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|host| !host.is_empty())
    }

    fn fill_host_from_primary_endpoint(
        template_defaults: Option<&ConnectionLaunchParams>,
        protocol_params: Option<ConnectionLaunchParams>,
        resolution_context: &Value,
    ) -> Option<ConnectionLaunchParams> {
        if Self::launch_params_have_endpoint_host(template_defaults)
            || Self::launch_params_have_endpoint_host(protocol_params.as_ref())
        {
            return protocol_params;
        }

        let Some(endpoint_host) = resolve_primary_endpoint(resolution_context) else {
            return protocol_params;
        };

        let mut params = protocol_params.unwrap_or_default();
        params.insert("host".to_string(), Value::String(endpoint_host));
        Some(params)
    }

    /// Builds the per-launch placeholder resolution context: a CLONE of the
    /// materialized `host_config` (which already carries the top-level
    /// `primary_ip4`/`primary_ip6`) with the reserved `target` key overlaid from
    /// the authoritative per-launch namespace. The overlay is never persisted and
    /// never merged into the cached config. When the target matches neither a
    /// device nor a VM, the `target` key is omitted and `host_config` is returned
    /// unchanged.
    async fn build_launch_resolution_context(
        &self,
        target_id: Uuid,
        host_config: &Value,
    ) -> Result<Value, AppError> {
        let mut resolution_context = host_config.clone();
        if let Some(namespace) = self
            .materializer
            .load_target_variable_namespace(target_id)
            .await?
            && let Value::Object(map) = &mut resolution_context
        {
            overlay_endpoint_aliases_from_target_namespace(map, &namespace);
            map.insert("target".to_string(), namespace);
        }
        Ok(resolution_context)
    }

    fn normalize_endpoint_host_value(host: &str) -> Option<String> {
        let trimmed = host.trim();
        if trimmed.is_empty() {
            return None;
        }
        let without_brackets = trimmed
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .unwrap_or(trimmed);
        let ip_part = without_brackets
            .split('/')
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        IpAddr::from_str(ip_part)
            .ok()
            .map(|value| value.to_string())
    }

    async fn ensure_endpoint_bound_to_target(
        &self,
        target_id: Uuid,
        protocol: &ConnectionProtocol,
        payload_host: Option<&str>,
    ) -> Result<String, AppError> {
        let Some(raw_host) = payload_host
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Err(endpoint_validation_error(
                ENDPOINT_REASON_UNRESOLVED,
                "Connection host is required after payload normalization",
            ));
        };

        let Some(endpoint_host) = Self::normalize_endpoint_host_value(raw_host) else {
            return Err(endpoint_validation_error(
                ENDPOINT_REASON_UNRESOLVED,
                format!("Connection endpoint '{raw_host}' is not a valid IP address"),
            ));
        };

        let is_bound = self
            .repository
            .is_endpoint_bound_to_target(target_id, &endpoint_host)
            .await?;
        if is_bound {
            return Ok(endpoint_host);
        }

        log_event(
            LogLevel::Warning,
            "connection.endpoint.validation.failed",
            EventFields::default()
                .with_operation("create_session")
                .with_resource("target", target_id.to_string())
                .with_context("target_entity_type", "device_or_vm")
                .with_context("target_entity_id", target_id.to_string())
                .with_context("endpoint_field", "host")
                .with_context("endpoint_value", endpoint_host.clone())
                .with_context("protocol", protocol.as_str().to_string())
                .with_context("reason_code", ENDPOINT_REASON_NOT_BOUND_TO_TARGET),
        );

        Err(endpoint_validation_error(
            ENDPOINT_REASON_NOT_BOUND_TO_TARGET,
            format!(
                "Target address ({endpoint_host}) is not assigned to any of the target's inventory interfaces, so the connection is blocked. Assign the address to a device/VM interface and try again."
            ),
        ))
    }

    async fn resolve_user_credential_set_for_launch(
        &self,
        user_id: Uuid,
        protocol: &ConnectionProtocol,
        selected_credential_set_id: Uuid,
    ) -> Result<UserCredentialSet, AppError> {
        let set = self
            .repository
            .get_user_credential_set(selected_credential_set_id)
            .await?
            .ok_or_else(|| AppError::Validation("Selected credential set not found".to_string()))?;
        if set.user_id != user_id {
            return Err(AppError::Forbidden(
                "Selected credential set is owned by another user".to_string(),
            ));
        }
        if set.protocol != *protocol {
            return Err(AppError::Validation(format!(
                "Selected credential set protocol '{}' does not match launch protocol '{}'",
                set.protocol.as_str(),
                protocol.as_str()
            )));
        }
        Ok(set)
    }

    async fn resolve_default_user_credential_set_for_launch(
        &self,
        user_id: Uuid,
        protocol: &ConnectionProtocol,
    ) -> Result<Option<UserCredentialSet>, AppError> {
        // Unpaged: default resolution must see every set for the protocol.
        let (sets, _) = self
            .repository
            .list_user_credential_sets(
                UserCredentialSetFilter {
                    user_id: vec![user_id],
                    protocol: vec![protocol.clone()],
                    is_default: Vec::new(),
                    sort: None,
                },
                None,
                None,
            )
            .await?;
        let mut defaults = sets.into_iter().filter(|set| set.is_default);
        let Some(default_set) = defaults.next() else {
            return Ok(None);
        };
        if defaults.next().is_some() {
            return Err(AppError::Conflict(format!(
                "Multiple default credential sets found for protocol '{}'",
                protocol.as_str()
            )));
        }
        Ok(Some(default_set))
    }

    async fn resolve_launch_credential_selection(
        &self,
        user_id: Option<Uuid>,
        protocol: &ConnectionProtocol,
        session_metadata: Option<&Value>,
    ) -> Result<LaunchCredentialSelection, AppError> {
        let entry_selector = session_metadata
            .and_then(|value| value.get("entryId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let entry_selector = match entry_selector {
            Some(raw) => parse_connection_entry_selector(raw)?,
            None => None,
        };
        let selected_entry_index = entry_selector
            .as_ref()
            .filter(|sel| sel.protocol == protocol.as_str())
            .map(|sel| sel.index);
        // Only a `config:` selector's index addresses the ordered shared-account
        // list in the connconf contract. Legacy `user:config-context:*`
        // selectors used the index for the shared entry and appended a personal
        // credential set id; keep that compatibility without applying it to the
        // new `user:connconf:*` personal-set index.
        let selected_config_entry_index = entry_selector
            .as_ref()
            .filter(|sel| {
                sel.protocol == protocol.as_str()
                    && (sel.kind == EntrySelectorKind::Config
                        || (sel.kind == EntrySelectorKind::User
                            && sel.source_ref == "config-context"))
            })
            .map(|sel| sel.index);
        let allow_missing_selected_config_entry = entry_selector.as_ref().is_some_and(|sel| {
            sel.protocol == protocol.as_str()
                && sel.kind == EntrySelectorKind::User
                && sel.source_ref == "config-context"
        });
        let selected_user_credential_set_id = entry_selector
            .as_ref()
            .filter(|sel| sel.kind == EntrySelectorKind::User && sel.protocol == protocol.as_str())
            .and_then(|sel| sel.credential_set_id);
        let explicit_user_credential_set_selected = selected_user_credential_set_id.is_some();
        let selected_user_credential_set = match user_id {
            Some(user_id) => {
                if let Some(selected_user_credential_set_id) = selected_user_credential_set_id {
                    Some(
                        self.resolve_user_credential_set_for_launch(
                            user_id,
                            protocol,
                            selected_user_credential_set_id,
                        )
                        .await?,
                    )
                } else {
                    self.resolve_default_user_credential_set_for_launch(user_id, protocol)
                        .await?
                }
            }
            // No credential user means the shared connection-config account only:
            // no personal user-credential-set overlay.
            None => None,
        };
        let selected_user_credential_set_params = selected_user_credential_set
            .as_ref()
            .map(|set| {
                Self::value_to_launch_params_map(set.params.clone(), "user credential set params")
            })
            .transpose()?
            .filter(|map| !map.is_empty());
        let merge_preference = Self::session_credential_merge_preference(session_metadata);
        let credential_set_merge_policy = if selected_user_credential_set.is_some()
            && (merge_preference == CredentialMergePreference::PersonalFirst
                || explicit_user_credential_set_selected)
        {
            UserCredentialSetMergePolicy::OverrideExisting
        } else {
            UserCredentialSetMergePolicy::FillMissing
        };

        Ok(LaunchCredentialSelection {
            selected_entry_index,
            selected_config_entry_index,
            allow_missing_selected_config_entry,
            selected_user_credential_set,
            selected_user_credential_set_params,
            credential_set_merge_policy,
        })
    }

    async fn apply_user_credential_set_to_payload(
        &self,
        protocol: &ConnectionProtocol,
        selected_credential_set_id: Uuid,
        merge_policy: UserCredentialSetMergePolicy,
        payload: &mut Value,
    ) -> Result<(), AppError> {
        let mappings: &[(&str, &str, bool)] = match protocol {
            ConnectionProtocol::Ssh => &[
                (
                    "password",
                    USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PASSWORD,
                    true,
                ),
                (
                    "private_key",
                    USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PRIVATE_KEY,
                    true,
                ),
            ],
            ConnectionProtocol::Rdp => &[
                (
                    "username",
                    USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_USERNAME,
                    false,
                ),
                (
                    "password",
                    USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_PASSWORD,
                    true,
                ),
            ],
            ConnectionProtocol::Vnc => &[(
                "password",
                USER_CONNECTION_CREDENTIAL_FIELD_KEY_VNC_PASSWORD,
                true,
            )],
            ConnectionProtocol::App | ConnectionProtocol::Cctv => return Ok(()),
        };

        let Value::Object(map) = payload else {
            return Ok(());
        };
        let credentials_value = map
            .entry("credentials".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(credentials) = credentials_value else {
            return Err(AppError::Validation(
                "Proxy payload credentials must be a JSON object".to_string(),
            ));
        };

        // Secret credential fields must not inherit host-level values when a personal set is
        // available. Non-secret fields (e.g. RDP username) remain unless explicitly overridden by
        // set binding.
        for &(payload_key, _, clear_existing) in mappings {
            if clear_existing && merge_policy == UserCredentialSetMergePolicy::OverrideExisting {
                credentials.remove(payload_key);
            }
        }

        for &(payload_key, field_key, _) in mappings {
            if merge_policy == UserCredentialSetMergePolicy::FillMissing
                && credentials.contains_key(payload_key)
            {
                continue;
            }

            let Some(binding) = self
                .secret_repository
                .get_usage_binding(
                    USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE,
                    selected_credential_set_id,
                    field_key,
                )
                .await?
            else {
                continue;
            };

            credentials.insert(
                payload_key.to_string(),
                Value::String(binding.secret_id.to_string()),
            );
        }

        Ok(())
    }

    async fn materialize_target_config_for_connection(
        &self,
        session_id: Uuid,
        target_id: Uuid,
        actor_id: Option<Uuid>,
        access_control: Option<&dyn SecretVisibilityChecker>,
        request_context: Option<&RequestContext>,
    ) -> Result<crate::contexts::inventory::application::MaterializedTargetConfig, AppError> {
        let secret_scope = self
            .secret_repository
            .list_scope_secrets("connection_target", target_id)
            .await
            .inspect_err(|error| {
                log_error_event(
                    "connections.target_secret_scope.lookup.failed",
                    "target_secret_scope_error",
                    error.to_string(),
                    EventFields::default()
                        .with_operation("connections.materialize_target_config")
                        .with_resource("connection_session", session_id.to_string())
                        .with_context("target_id", target_id.to_string())
                        .with_message("Failed to load connection target secret scope"),
                );
            })?;

        self.materializer
            .materialize_target_config(
                crate::contexts::inventory::application::materialization::MaterializeTargetConfigInput {
                    redis: None,
                    target_id,
                    local_context_data: None,
                    secret_variables: &secret_scope.variables,
                    secret_bindings: &secret_scope.bindings,
                    secret_store: Some(self.secret_store.as_ref()),
                    access_control,
                    request_context,
                    audit: Some(self.audit_service.as_ref()),
                    actor_id,
                },
            )
            .await
    }

    fn sanitize_session_metadata(mut metadata: Value) -> Value {
        let Value::Object(map) = &mut metadata else {
            return metadata;
        };

        // Defense-in-depth: never persist sensitive connection credentials in session_metadata.
        // The canonical place for ephemeral secrets is `params` -> proxy payload, not DB metadata.
        // Session metadata launch params are accepted only under the canonical `launchParams` key.
        let launch_params = map.get_mut("launchParams");
        let Some(launch_params) = launch_params else {
            return metadata;
        };
        let Value::Object(launch_map) = launch_params else {
            return metadata;
        };

        for key in ["password", "private_key"] {
            launch_map.remove(key);
        }
        if let Some(Value::Object(credentials)) = launch_map.get_mut("credentials") {
            for key in ["password", "private_key"] {
                credentials.remove(key);
            }
        }
        metadata
    }

    fn validate_session_metadata(metadata: &Value) -> Result<(), AppError> {
        let Value::Object(map) = metadata else {
            return Ok(());
        };

        let allowed_keys = ["entryId", "template_id", "credentialMergePolicy"];
        for key in map.keys() {
            if !allowed_keys.contains(&key.as_str())
                && key != "launch_params"
                && key != "launchParams"
            {
                return Err(AppError::Validation(format!(
                    "Unsupported session_metadata key '{}'",
                    key
                )));
            }
        }

        if map.contains_key("launch_params") {
            return Err(AppError::Validation(
                "session_metadata.launch_params is not supported; use launchParams".to_string(),
            ));
        }

        if map.contains_key("launchParams") {
            return Err(AppError::Validation(
                "session_metadata.launchParams is not supported".to_string(),
            ));
        }

        if let Some(value) = map.get("entryId")
            && !matches!(value, Value::String(raw) if !raw.trim().is_empty())
        {
            return Err(AppError::Validation(
                "session_metadata.entryId must be a non-empty string".to_string(),
            ));
        }

        if let Some(value) = map.get("template_id")
            && !matches!(value, Value::String(raw) if !raw.trim().is_empty())
        {
            return Err(AppError::Validation(
                "session_metadata.template_id must be a non-empty string".to_string(),
            ));
        }
        if let Some(value) = map.get("credentialMergePolicy") {
            let Value::String(raw) = value else {
                return Err(AppError::Validation(
                    "session_metadata.credentialMergePolicy must be a string".to_string(),
                ));
            };
            let normalized = raw.trim().to_ascii_lowercase();
            if normalized != "context_first" && normalized != "personal_first" {
                return Err(AppError::Validation(
                    "session_metadata.credentialMergePolicy must be one of: context_first, personal_first"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }

    fn session_credential_merge_preference(metadata: Option<&Value>) -> CredentialMergePreference {
        let Some(Value::Object(map)) = metadata else {
            return CredentialMergePreference::ContextFirst;
        };
        let Some(Value::String(raw)) = map.get("credentialMergePolicy") else {
            return CredentialMergePreference::ContextFirst;
        };
        match raw.trim().to_ascii_lowercase().as_str() {
            "personal_first" => CredentialMergePreference::PersonalFirst,
            _ => CredentialMergePreference::ContextFirst,
        }
    }

    fn value_to_launch_params_map(
        value: Value,
        source: &str,
    ) -> Result<ConnectionLaunchParams, AppError> {
        match value {
            Value::Object(map) => Ok(map),
            Value::Null => Ok(ConnectionLaunchParams::new()),
            _ => Err(AppError::Validation(format!(
                "{} must be a JSON object",
                source
            ))),
        }
    }

    fn merge_launch_params_map(
        base: &mut ConnectionLaunchParams,
        overlay: ConnectionLaunchParams,
        merge_policy: UserCredentialSetMergePolicy,
    ) {
        merge_launch_params_map_values(base, overlay, merge_policy);
    }

    fn filter_client_credentials(
        protocol: &ConnectionProtocol,
        credentials: Map<String, Value>,
    ) -> Map<String, Value> {
        filter_client_credentials(protocol, credentials)
    }

    /// Resolve non-interactive SSH launch material without creating a browser
    /// connection session or proxy session.
    ///
    /// This is the worker-safe sibling of [`Self::create_session`]: it reuses
    /// the same connection-config account selection, user credential set merge,
    /// secret fence, endpoint-bound validation, and proxy routing decision, then
    /// returns only in-memory launch material for the caller to execute
    /// immediately.
    /// Resolve the connection-config default params (and per-secret attribution)
    /// for a launch, collapsing the "no record applies" case to `(None, empty)`.
    async fn resolve_connconf_defaults(
        &self,
        target_id: Uuid,
        protocol: &ConnectionProtocol,
        selected_entry_index: Option<usize>,
        allow_missing_selected_entry: bool,
    ) -> Result<
        (
            Option<ConnectionLaunchParams>,
            HashMap<Uuid, Vec<connconf_resolve::SecretLeafAttribution>>,
        ),
        AppError,
    > {
        Ok(
            match self
                .resolve_connconf_launch_params(
                    target_id,
                    protocol,
                    selected_entry_index,
                    allow_missing_selected_entry,
                )
                .await?
            {
                Some(resolution) => (Some(resolution.fields), resolution.secret_attributions),
                None => (None, HashMap::new()),
            },
        )
    }

    /// Merge connconf defaults, the selected user credential set and any
    /// entry-driven overrides into the effective launch params. The base is
    /// always present because [`merge_required_connconf_params`] errors on a
    /// missing connconf default, so the old `(None, _)` arms were unreachable.
    fn resolve_effective_launch_params(
        protocol: &ConnectionProtocol,
        host_default_params: Option<ConnectionLaunchParams>,
        user_set_params: Option<ConnectionLaunchParams>,
        merge_policy: UserCredentialSetMergePolicy,
        entry_overrides: Option<ConnectionLaunchParams>,
    ) -> Result<ConnectionLaunchParams, AppError> {
        let mut defaults = merge_required_connconf_params(
            protocol,
            host_default_params,
            user_set_params,
            merge_policy,
        )?;
        if let Some(overrides) = entry_overrides {
            Self::merge_launch_params_map(
                &mut defaults,
                overrides,
                UserCredentialSetMergePolicy::OverrideExisting,
            );
        }
        Ok(defaults)
    }

    /// Canonicalize the payload host against the target's endpoints and write it
    /// back into the payload. Falls back to the primary endpoint when the payload
    /// carries no host.
    async fn ensure_launch_payload_host(
        &self,
        payload: &mut Value,
        target_id: Uuid,
        protocol: &ConnectionProtocol,
        resolution_context: &Value,
    ) -> Result<String, AppError> {
        let payload_host = payload
            .as_object()
            .and_then(|map| map.get("host"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let fallback_host = if payload_host.is_none() {
            resolve_primary_endpoint(resolution_context)
        } else {
            None
        };
        let candidate_host = payload_host.or(fallback_host.as_deref());
        let endpoint_host = self
            .ensure_endpoint_bound_to_target(target_id, protocol, candidate_host)
            .await?;
        if let Value::Object(map) = payload {
            map.insert("host".to_string(), Value::String(endpoint_host.clone()));
        }
        Ok(endpoint_host)
    }

    // SECURITY: `access_control` is `Option` to match the established
    // connections-context convention, but for playbook execution it MUST NOT be
    // `None`. The (future) playbook worker MUST pass a real `SecretVisibilityChecker`
    // here so secrets are fenced against the original requester's visibility; the
    // run-group create path only authorizes the *reference* (see
    // `routes/playbooks.rs::create_run_group`), not the at-execution resolution.
    pub async fn resolve_ssh_execution_launch(
        &self,
        input: ResolveSshExecutionLaunchInput,
        access_control: Option<&dyn SecretVisibilityChecker>,
        request_context: Option<&RequestContext>,
    ) -> Result<ResolvedSshExecutionLaunch, AppError> {
        let ResolveSshExecutionLaunchInput {
            execution_id,
            target_id,
            credential_user_id,
            session_metadata,
            protocol_params,
            requested_proxy_node_id,
        } = input;
        let protocol = ConnectionProtocol::Ssh;

        if let Some(metadata) = session_metadata.as_ref() {
            Self::validate_session_metadata(metadata)?;
        }
        let credential_selection = self
            .resolve_launch_credential_selection(
                credential_user_id,
                &protocol,
                session_metadata.as_ref(),
            )
            .await?;
        let materialized = self
            .materialize_target_config_for_connection(
                execution_id,
                target_id,
                credential_user_id,
                access_control,
                request_context,
            )
            .await?;

        let entry_overrides = if credential_selection.selected_entry_index.is_some() {
            Self::validate_entry_driven_overrides(&protocol, protocol_params)?
        } else {
            protocol_params
        };

        let (host_default_params, connconf_secret_attributions) = self
            .resolve_connconf_defaults(
                target_id,
                &protocol,
                credential_selection.selected_config_entry_index,
                credential_selection.allow_missing_selected_config_entry,
            )
            .await?;

        let effective_protocol_params = Some(Self::resolve_effective_launch_params(
            &protocol,
            host_default_params,
            credential_selection
                .selected_user_credential_set_params
                .clone(),
            credential_selection.credential_set_merge_policy,
            entry_overrides,
        )?);

        let resolution_context = self
            .build_launch_resolution_context(target_id, &materialized.config)
            .await?;
        let effective_protocol_params = Self::resolve_config_placeholders_in_launch_params(
            effective_protocol_params,
            &resolution_context,
        )?;
        let effective_protocol_params = Self::fill_host_from_primary_endpoint(
            None,
            effective_protocol_params,
            &resolution_context,
        );
        let mut payload =
            Self::build_proxy_payload(protocol.clone(), None, effective_protocol_params)?;

        if let Some(selected_user_credential_set_id) = credential_selection
            .selected_user_credential_set
            .as_ref()
            .map(|set| set.id)
        {
            self.apply_user_credential_set_to_payload(
                &protocol,
                selected_user_credential_set_id,
                credential_selection.credential_set_merge_policy,
                &mut payload,
            )
            .await?;
        }

        let connconf_secret_attributions_ref = if connconf_secret_attributions.is_empty() {
            None
        } else {
            Some(&connconf_secret_attributions)
        };
        payload = self
            .resolve_proxy_payload_secrets(
                target_id,
                credential_user_id,
                payload,
                Some(&materialized.config),
                connconf_secret_attributions_ref,
            )
            .await?;
        Self::ensure_required_proxy_payload_credentials(&protocol, &payload)?;

        let endpoint_host = self
            .ensure_launch_payload_host(&mut payload, target_id, &protocol, &resolution_context)
            .await?;

        // Pins for the endpoint and every jump hop, resolved after secrets so a
        // bastion key is seeded too and after the host is canonicalized.
        payload =
            proxy_payload::resolve_and_inject_host_key_pins(payload, self.repository.as_ref())
                .await?;

        let proxy_assignment_scopes = self.proxy_assignment_scopes(target_id).await?;
        let routing_decision = self
            .routing_decisions
            .resolve_for_session_create(
                requested_proxy_node_id,
                Some(endpoint_host.as_str()),
                &protocol,
                &proxy_assignment_scopes,
            )
            .await?;

        Ok(ResolvedSshExecutionLaunch {
            target_id,
            credential_user_id,
            endpoint_host,
            routing_decision,
            proxy_payload: payload,
        })
    }

    pub async fn resolve_session_client_credentials(
        &self,
        session_id: Uuid,
        actor_id: Uuid,
        access_control: Option<&dyn SecretVisibilityChecker>,
        request_context: Option<&RequestContext>,
    ) -> Result<Map<String, Value>, AppError> {
        let session = self.require_session(session_id).await?;
        if session.protocol != ConnectionProtocol::Vnc
            && session.protocol != ConnectionProtocol::Rdp
        {
            return Err(AppError::Validation(
                "Client credentials are only supported for VNC/RDP sessions".to_string(),
            ));
        }

        self.read_client_credentials(&session, actor_id, access_control, request_context)
            .await
    }

    pub async fn create_session(
        &self,
        input: CreateSessionInput,
        access_control: Option<&dyn SecretVisibilityChecker>,
        request_context: Option<&RequestContext>,
    ) -> Result<ConnectionSession, AppError> {
        let CreateSessionInput {
            target_id,
            user_id,
            protocol,
            client_ip,
            audit_auth_method,
            audit_api_token_id,
            session_metadata,
            template_id,
            protocol_params,
            requested_proxy_node_id,
            require_embedded_proxy,
        } = input;
        let span = operation_span(
            "create_session",
            &[
                ("target_id", target_id.to_string()),
                ("user_id", user_id.to_string()),
            ],
        );
        return instrument_span(span, async {
            let proxy_span = proxy_call_span(
                "create_session",
                &[
                    ("target_id", target_id.to_string()),
                    ("user_id", user_id.to_string()),
                ],
            );
            let session_id = Uuid::now_v7();
            if let Some(metadata) = session_metadata.as_ref() {
                Self::validate_session_metadata(metadata)?;
            }
            let (mut payload, replay) = self.prepare_session_launch(
                session_credentials::LaunchSubject { session_id, target_id, user_id, protocol: &protocol },
                session_credentials::LaunchPreparation::Create {
                    metadata: session_metadata.as_ref(), template_id, overrides: protocol_params,
                }, access_control, request_context,
            ).await?;
            let endpoint_host = replay.endpoint_host.clone();
            let client_credentials = if matches!(protocol, ConnectionProtocol::Vnc | ConnectionProtocol::Rdp) {
                Some(self.session_credentials.prepare(&replay)?)
            } else { None };
            // SSH interactive sessions seed the pins the proxy must trust. The
            // payload is sent to the proxy unchanged, so its stored
            // `ProxyConnection` carries the same list.
            if protocol == ConnectionProtocol::Ssh {
                payload = proxy_payload::resolve_and_inject_host_key_pins(
                    payload,
                    self.repository.as_ref(),
                )
                .await?;
            }
            let proxy_assignment_scopes = self.proxy_assignment_scopes(target_id).await?;
            let routing_decision = self
                .routing_decisions
                .resolve_for_session_create(
                    requested_proxy_node_id,
                    Some(endpoint_host.as_str()),
                    &protocol,
                    &proxy_assignment_scopes,
                )
                .await?;
            if require_embedded_proxy && routing_decision.proxy_node_id.is_some() {
                return Err(AppError::Validation(
                    "Connection session requires embedded proxy routing".to_string(),
                ));
            }
            let proxy_route = self.proxy_route_for_decision(&routing_decision).await?;
            let payload_summary = summarize_payload_for_log(&payload);
            if let Value::Object(map) = &mut payload {
                map.insert(
                    "connection_id".to_string(),
                    Value::String(session_id.to_string()),
                );
            }
            tracing::debug!(
                operation = "connections.create_session.prepare_payload",
                session_id = %session_id,
                target_id = %target_id,
                user_id = %user_id,
                protocol = %protocol.as_str(),
                routing_source = %routing_decision.routing_source.as_str(),
                proxy_node_id = %routing_decision
                    .proxy_node_id
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "embedded".to_string()),
                payload_summary = %payload_summary,
                "Connection session payload prepared"
            );
            let proxy_session = instrument_span(
                proxy_span,
                self.proxy_client.create_connection(
                    payload,
                    proxy_route.proxy_base_url.as_deref(),
                    proxy_route.credential.as_ref(),
                ),
            )
            .await?;
            tracing::debug!(
                operation = "connections.create_session.proxy_allocated",
                session_id = %session_id,
                proxy_id = %proxy_session.id,
                routing_source = %routing_decision.routing_source.as_str(),
                proxy_node_slug = %routing_decision
                    .proxy_node_slug
                    .as_deref()
                    .unwrap_or("embedded"),
                "Proxy session allocated"
            );
            let now = Utc::now();
            let metadata = Self::sanitize_session_metadata(session_metadata.unwrap_or_else(|| json!({})));
            let routing_source = routing_decision.routing_source.clone();
            let routing_proxy_node_id = routing_decision.proxy_node_id;
            let routing_proxy_node_name = routing_decision.proxy_node_name.clone();
            let routing_proxy_node_slug = routing_decision.proxy_node_slug.clone();
            let session = ConnectionSession {
                id: session_id,
                target_id,
                user_id,
                protocol,
                status: ConnectionStatus::Pending,
                proxy_id: Some(proxy_session.id),
                proxy_node_id: routing_proxy_node_id,
                routing_source,
                client_ip,
                last_activity: now,
                session_metadata: metadata,
                started_at: now,
                ended_at: None,
            };

            self.repository
                .create_session(&session)
                .await
                .inspect_err(|error| {
                log_error_event(
                    "connections.connection_session.persist.failed",
                    "connection_session_persistence_error",
                    error.to_string(),
                    EventFields::default()
                        .with_operation("connections.create_session.persist")
                        .with_resource("connection_session", session.id.to_string())
                        .with_context("target_id", target_id.to_string())
                        .with_message("Failed to persist connection session"),
                );
                })?;
            let event = AuditEvent {
                id: Uuid::now_v7(),
                event_type: "connection.session.created".to_string(),
                actor_id: Some(user_id),
            actor_kind: "user".to_string(),
                target_type: Some("connection_session".to_string()),
                target_id: Some(session.id),
                metadata: json!({
                    "target_id": target_id.to_string(),
                    "protocol": format!("{:?}", session.protocol),
                    "status": format!("{:?}", session.status),
                    "proxy_id": session.proxy_id.map(|proxy_id| proxy_id.to_string()),
                    "proxy_node_id": session.proxy_node_id.map(|proxy_node_id| proxy_node_id.to_string()),
                    "proxy_node_name": routing_proxy_node_name,
                    "proxy_node_slug": routing_proxy_node_slug,
                    "routing_source": session.routing_source.as_str(),
                    "client_ip": session.client_ip.clone(),
                    "auth_method": audit_auth_method,
                    "api_token_id": audit_api_token_id.map(|value| value.to_string()),
                }),
                created_at: Utc::now(),
            };
            self.audit_service
                .record_event(event)
                .await
                .inspect_err(|error| {
                log_error_event(
                    "connections.connection_session.audit_record.failed",
                    "connection_session_audit_error",
                    error.to_string(),
                    EventFields::default()
                        .with_operation("connections.create_session.record_audit")
                        .with_resource("connection_session", session.id.to_string())
                        .with_context("target_id", target_id.to_string())
                        .with_message("Failed to record connection session audit"),
                );
                })?;
            log_connection_event(log_connection_event::Input {
                event: "connection.session.created",
                session_id: Some(session.id.to_string()),
                host_id: Some(session.target_id.to_string()),
                user_id: Some(session.user_id.to_string()),
                protocol: Some(format!("{:?}", session.protocol)),
                status: Some(format!("{:?}", session.status)),
                correlation_id: None,
            });
            log_event(
                LogLevel::Info,
                "connection.session.created",
                EventFields::default()
                    .with_operation("create_connection_session")
                    .with_resource("connection", session.id.to_string())
                    .with_status(format!("{:?}", session.status)),
            );
            if let Some(snapshot) = client_credentials {
                self.session_credentials.publish(session.id, snapshot)?;
            }
            Ok(session)
        })
        .await;
    }

    pub async fn get_template(
        &self,
        template_id: Uuid,
    ) -> Result<Option<ConnectionTemplate>, AppError> {
        self.repository.get_template(template_id).await
    }

    pub async fn list_templates(
        &self,
        filter: ConnectionTemplateFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ConnectionTemplate>, AppError> {
        self.repository.list_templates(filter, limit, offset).await
    }

    pub async fn count_templates(&self, filter: ConnectionTemplateFilter) -> Result<u64, AppError> {
        self.repository.count_templates(filter).await
    }

    /// Resolve the read-only launch options for a device + protocol: the ordered
    /// shared launch-accounts (Option A, non-merging — see
    /// `connection_config::resolve_launch_accounts`) plus the caller's personal
    /// credential sets for that protocol when connection-config makes the
    /// protocol available on this target.
    ///
    /// Performs the SAME DB round-trips as the connect-path
    /// [`Self::resolve_connconf_launch_params`] (device scope ancestry,
    /// scope-matched assignments, all records) but runs the NON-merging account
    /// resolver, so a host+protocol can surface several distinct launchable
    /// accounts. This is additive: it does NOT change the connect path.
    ///
    /// The returned [`ConnectionLaunchOptions::shared_accounts`] ordering is the
    /// `config:connconf:<proto>:<index>` entry-id contract; the personal sets are
    /// appended after, addressed by `user:connconf:<proto>:<index>:<uuid>`, only
    /// when at least one shared account/fallback exists.
    /// Secret leaves in the returned params are bare-UUID references — the caller
    /// MUST redact them to presence-only before returning over the wire.
    pub async fn resolve_launch_options(
        &self,
        target_id: Uuid,
        user_id: Uuid,
        protocol: &ConnectionProtocol,
    ) -> Result<ConnectionLaunchOptions, AppError> {
        let shared_accounts = self
            .resolve_shared_launch_accounts(target_id, protocol)
            .await?;
        self.launch_options_from_shared_accounts(shared_accounts, target_id, user_id, protocol)
            .await
    }

    /// [`Self::resolve_launch_options`] over preloaded, protocol-independent
    /// inputs (see [`Self::load_shared_launch_inputs`]). A caller resolving all
    /// launch protocols for one target uses this so the three connconf
    /// round-trips happen once per request instead of once per protocol.
    pub async fn resolve_launch_options_with_inputs(
        &self,
        inputs: Option<&SharedLaunchInputs>,
        target_id: Uuid,
        user_id: Uuid,
        protocol: &ConnectionProtocol,
    ) -> Result<ConnectionLaunchOptions, AppError> {
        let shared_accounts = Self::shared_launch_accounts_from_inputs(inputs, protocol)?;
        self.launch_options_from_shared_accounts(shared_accounts, target_id, user_id, protocol)
            .await
    }

    /// Shared tail of the two `resolve_launch_options*` entry points: app-URI
    /// placeholder substitution plus the caller's personal credential sets.
    async fn launch_options_from_shared_accounts(
        &self,
        mut shared_accounts: Vec<LaunchAccount>,
        target_id: Uuid,
        user_id: Uuid,
        protocol: &ConnectionProtocol,
    ) -> Result<ConnectionLaunchOptions, AppError> {
        // For app protocol, substitute {{primary_ip4}}/{{primary_ip6}} placeholders
        // in params.url so the response carries a fully-resolved launch URI.
        if matches!(protocol, ConnectionProtocol::App) && !shared_accounts.is_empty() {
            let materialized = self
                .materializer
                .materialize_target_config(
                    crate::contexts::inventory::application::materialization::MaterializeTargetConfigInput {
                        redis: None,
                        target_id,
                        local_context_data: None,
                        secret_variables: &[],
                        secret_bindings: &[],
                        secret_store: None,
                        access_control: None,
                        request_context: None,
                        audit: None,
                        actor_id: None,
                    },
                )
                .await?;
            let resolution_context = self
                .build_launch_resolution_context(target_id, &materialized.config)
                .await?;
            for account in &mut shared_accounts {
                substitute_config_placeholders(&mut account.params, &resolution_context)?;
            }
        }

        let personal_credential_sets = if shared_accounts.is_empty() {
            Vec::new()
        } else {
            // Unpaged: the launch picker must offer every personal set.
            self.repository
                .list_user_credential_sets(
                    UserCredentialSetFilter {
                        user_id: vec![user_id],
                        protocol: vec![protocol.clone()],
                        is_default: Vec::new(),
                        sort: None,
                    },
                    None,
                    None,
                )
                .await?
                .0
        };
        Ok(ConnectionLaunchOptions {
            shared_accounts,
            personal_credential_sets,
        })
    }

    /// Run the non-merging account resolver for a device + protocol, doing the
    /// DB round-trips the pure resolver needs. Returns an empty vec when no
    /// `connconf` config applies (no matching records), mirroring
    /// [`Self::resolve_connconf_launch_params`]'s `Ok(None)`.
    ///
    /// Delegates the ordering contract to
    /// [`Self::shared_launch_accounts_from_inputs`], which owns it.
    async fn resolve_shared_launch_accounts(
        &self,
        target_id: Uuid,
        protocol: &ConnectionProtocol,
    ) -> Result<Vec<LaunchAccount>, AppError> {
        if Self::connconf_protocol(protocol).is_none() {
            return Ok(Vec::new());
        }
        let inputs = self.load_shared_launch_inputs(target_id).await?;
        Self::shared_launch_accounts_from_inputs(inputs.as_ref(), protocol)
    }

    /// Load the three protocol-independent DB inputs the shared launch-account
    /// resolver needs for `target_id`. `Ok(None)` means the target has no scope
    /// ancestry, i.e. NO shared account for ANY protocol.
    ///
    /// A caller resolving several protocols for one target must call this once
    /// and pass the result to [`Self::resolve_launch_options_with_inputs`];
    /// the single-protocol paths go through the wrappers and keep their three
    /// round-trips.
    pub async fn load_shared_launch_inputs(
        &self,
        target_id: Uuid,
    ) -> Result<Option<SharedLaunchInputs>, AppError> {
        let Some(device_scope_ids) = self
            .connconf_assignments
            .get_device_scopes(target_id)
            .await?
        else {
            return Ok(None);
        };
        let device_scopes = connconf_resolve::build_device_scopes(&device_scope_ids);

        let assignments = self
            .connconf_assignments
            .list_for_scopes(&device_scopes)
            .await?;
        // Deliberately the FULL record set, not `list_by_protocol`: the resolver
        // follows each candidate's `base_record_id` template chain, and a base
        // record need not share the requested protocol.
        let records = self.connconf_records.list_all().await?;

        Ok(Some(SharedLaunchInputs {
            device_scopes,
            assignments,
            records,
        }))
    }

    /// Run the non-merging account resolver over preloaded inputs. Returns an
    /// empty vec when no `connconf` config applies (no inputs / no matching
    /// records), mirroring [`Self::resolve_connconf_launch_params`]'s `Ok(None)`.
    ///
    /// This is the SINGLE owner of the entry_id ordering contract: BOTH the
    /// read endpoint ([`Self::resolve_launch_options`]) and the connect path
    /// ([`Self::resolve_connconf_launch_params`]) build their ordered account
    /// list here, so an index from a `config:` selector means the same account
    /// in both. Do not duplicate the ordering logic elsewhere.
    fn shared_launch_accounts_from_inputs(
        inputs: Option<&SharedLaunchInputs>,
        protocol: &ConnectionProtocol,
    ) -> Result<Vec<LaunchAccount>, AppError> {
        let Some(connconf_protocol) = Self::connconf_protocol(protocol) else {
            return Ok(Vec::new());
        };
        let Some(inputs) = inputs else {
            return Ok(Vec::new());
        };

        let input = ResolveInput {
            protocol: connconf_protocol,
            device_scopes: &inputs.device_scopes,
            assignments: &inputs.assignments,
            records: &inputs.records,
        };
        resolve_launch_accounts(&input).map_err(|error| AppError::Validation(error.to_string()))
    }

    pub async fn create_proxy_node(
        &self,
        input: CreateProxyNodeInput,
    ) -> Result<ProxyNode, AppError> {
        self.create_proxy_node_with_bootstrap(input)
            .await
            .map(|created| created.node)
    }

    pub async fn create_proxy_node_with_bootstrap(
        &self,
        input: CreateProxyNodeInput,
    ) -> Result<CreatedProxyNode, AppError> {
        let now = Utc::now();
        let mut proxy_node = ProxyNode {
            id: Uuid::now_v7(),
            name: input.name.trim().to_string(),
            slug: input.slug.trim().to_string(),
            base_url: input.base_url.trim().to_string(),
            enabled: input.enabled,
            secret_version: 0,
            secret_rotated_at: None,
            created_at: now,
            updated_at: now,
        };
        proxy_node
            .validate()
            .map_err(|error| AppError::Validation(error.to_string()))?;

        if self
            .repository
            .get_proxy_node_by_slug(&proxy_node.slug)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict(format!(
                "Proxy node slug '{}' already exists",
                proxy_node.slug
            )));
        }

        self.repository.create_proxy_node(&proxy_node).await?;
        let bootstrap = self.issue_proxy_node_secret(&mut proxy_node).await?;
        Ok(CreatedProxyNode {
            node: proxy_node,
            bootstrap,
        })
    }

    pub async fn rotate_proxy_node_secret(
        &self,
        proxy_node_id: Uuid,
    ) -> Result<CreatedProxyNode, AppError> {
        let mut proxy_node = self
            .repository
            .get_proxy_node(proxy_node_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Proxy node not found".to_string()))?;
        let bootstrap = self.issue_proxy_node_secret(&mut proxy_node).await?;
        Ok(CreatedProxyNode {
            node: proxy_node,
            bootstrap,
        })
    }

    pub async fn proxy_node_credential(
        &self,
        proxy_node_id: Uuid,
    ) -> Result<ProxyCredential, AppError> {
        let Some(binding) = self
            .secret_repository
            .get_usage_binding(
                PROXY_NODE_CREDENTIAL_CONSUMER_TYPE,
                proxy_node_id,
                PROXY_NODE_SECRET_FIELD_KEY,
            )
            .await?
        else {
            return Err(AppError::InvalidConfig(format!(
                "Proxy node {} has no issued secret",
                proxy_node_id
            )));
        };
        let secret = self.secret_store.get_secret(binding.secret_id).await?;
        ProxyCredential::new(proxy_node_id.to_string(), secret)
    }

    async fn issue_proxy_node_secret(
        &self,
        proxy_node: &mut ProxyNode,
    ) -> Result<ProxyNodeBootstrap, AppError> {
        let secret = generate_proxy_node_token(proxy_node.id)?;
        let existing = self
            .secret_repository
            .get_usage_binding(
                PROXY_NODE_CREDENTIAL_CONSUMER_TYPE,
                proxy_node.id,
                PROXY_NODE_SECRET_FIELD_KEY,
            )
            .await?;
        let intent = if existing.is_some() {
            SecretFieldIntent::Rotate
        } else {
            SecretFieldIntent::Set
        };
        let secret_fields = SecretFieldIntentService::new(self.secret_repository.clone());
        let _ = secret_fields
            .apply_intent(ApplySecretFieldIntentInput {
                consumer_type: PROXY_NODE_CREDENTIAL_CONSUMER_TYPE.to_string(),
                consumer_id: proxy_node.id,
                owner_scope_type: PROXY_NODE_SECRET_OWNER_SCOPE_TYPE.to_string(),
                owner_scope_id: proxy_node.id,
                field_key: PROXY_NODE_SECRET_FIELD_KEY.to_string(),
                intent,
                value: Some(secret.clone()),
                secret_id: None,
            })
            .await?;

        let rotated_at = Utc::now();
        self.repository
            .mark_proxy_node_secret_rotated(proxy_node.id, rotated_at)
            .await?;
        proxy_node.secret_version += 1;
        proxy_node.secret_rotated_at = Some(rotated_at);
        proxy_node.updated_at = rotated_at;
        Ok(build_proxy_node_bootstrap(proxy_node, secret))
    }
}

const PROXY_NODE_SECRET_FIELD_KEY: &str = "node_secret";
const PROXY_NODE_SECRET_OWNER_SCOPE_TYPE: &str = "proxy_node";
const DEFAULT_PROXY_NODE_IMAGE: &str = "mlimarenko/piping:backend-latest";

fn generate_proxy_node_token(proxy_node_id: Uuid) -> Result<String, AppError> {
    Ok(format!(
        "pn_{}.{}",
        proxy_node_id,
        generate_proxy_secret_material()?
    ))
}

fn build_proxy_node_bootstrap(proxy_node: &ProxyNode, secret: String) -> ProxyNodeBootstrap {
    let image = std::env::var("PIPING_PROXY_NODE_IMAGE")
        .unwrap_or_else(|_| DEFAULT_PROXY_NODE_IMAGE.into());
    let mut env = BTreeMap::new();
    env.insert("PIPING_RUN_MODE".to_string(), "proxy".to_string());
    env.insert(
        "PIPING_SERVER__BIND".to_string(),
        "0.0.0.0:8080".to_string(),
    );
    env.insert(PROXY_NODE_ID_ENV_KEY.to_string(), proxy_node.id.to_string());
    env.insert(PROXY_NODE_SECRET_ENV_KEY.to_string(), secret.clone());

    let docker_run_command = format!(
        "docker run -d --name {} \\\n  -e PIPING_RUN_MODE=proxy \\\n  -e PIPING_SERVER__BIND=0.0.0.0:8080 \\\n  -e {}={} \\\n  -e {}={} \\\n  -p 8080:8080 {}",
        shell_arg(&format!("piping-proxy-{}", proxy_node.slug)),
        PROXY_NODE_ID_ENV_KEY,
        shell_arg(&proxy_node.id.to_string()),
        PROXY_NODE_SECRET_ENV_KEY,
        shell_arg(&secret),
        shell_arg(&image),
    );

    ProxyNodeBootstrap {
        node_id: proxy_node.id.to_string(),
        node_slug: proxy_node.slug.clone(),
        node_secret: secret,
        env,
        docker_run_command,
    }
}

fn shell_arg(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
