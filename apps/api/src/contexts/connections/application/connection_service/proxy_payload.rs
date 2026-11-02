use super::*;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommonLaunchParamsInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SshCredentialsInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    private_key: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RdpCredentialsInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VncCredentialsInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RdpOptionsInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    color_depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dpi: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ignore_cert: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_auth: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_gfx: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    security: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_drive: Option<bool>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SshLaunchParamsInput {
    #[serde(flatten)]
    common: CommonLaunchParamsInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    credentials: Option<SshCredentialsInput>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RdpLaunchParamsInput {
    #[serde(flatten)]
    common: CommonLaunchParamsInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    credentials: Option<RdpCredentialsInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<RdpOptionsInput>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VncLaunchParamsInput {
    #[serde(flatten)]
    common: CommonLaunchParamsInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    credentials: Option<VncCredentialsInput>,
}

pub(crate) fn normalize_launch_params(
    protocol: &ConnectionProtocol,
    params: Option<Value>,
) -> Result<Option<ConnectionLaunchParams>, AppError> {
    let Some(raw) = params.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let normalized = match protocol {
        ConnectionProtocol::Ssh => {
            parse_typed_launch_params::<SshLaunchParamsInput>(protocol, raw)?
        }
        ConnectionProtocol::Rdp => {
            parse_typed_launch_params::<RdpLaunchParamsInput>(protocol, raw)?
        }
        ConnectionProtocol::Vnc => {
            parse_typed_launch_params::<VncLaunchParamsInput>(protocol, raw)?
        }
        ConnectionProtocol::App => {
            return Err(AppError::Validation(
                "APP protocol does not create connection sessions; use direct app launch flow"
                    .to_string(),
            ));
        }
        ConnectionProtocol::Cctv => return Err(AppError::Validation(
            "VIDEO protocol does not create connection sessions; use the device-management flow"
                .to_string(),
        )),
    };
    Ok(Some(normalized))
}

fn parse_typed_launch_params<T: for<'de> Deserialize<'de> + Serialize>(
    protocol: &ConnectionProtocol,
    raw: Value,
) -> Result<ConnectionLaunchParams, AppError> {
    let typed = serde_json::from_value::<T>(raw).map_err(|error| {
        AppError::Validation(format!(
            "Connection params for protocol '{}' are invalid: {}",
            protocol.as_str(),
            error
        ))
    })?;
    let encoded = serde_json::to_value(typed).map_err(|error| {
        AppError::Internal(format!(
            "Failed to encode normalized '{}' params: {}",
            protocol.as_str(),
            error
        ))
    })?;
    match encoded {
        Value::Object(map) => Ok(map),
        _ => Err(AppError::Internal(format!(
            "Failed to encode normalized '{}' params: expected object",
            protocol.as_str()
        ))),
    }
}

use uuid::Uuid;

use super::connconf_resolve::SecretLeafAttribution;
use super::routing::resolve_proxy_host;
use crate::contexts::secrets::application::{
    CONNECTION_CONFIG_RECORD_CONSUMER_TYPE, CONNECTION_TARGET_SECRET_CONSUMER_TYPE,
};
use crate::contexts::shared::host_key_pins;
use crate::observability::{EventFields, LogLevel, log_event};

pub(super) fn parse_secret_link_id(value: &str) -> Result<Option<Uuid>, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    if let Ok(secret_id) = Uuid::parse_str(trimmed) {
        return Ok(Some(secret_id));
    }

    Ok(None)
}

/// Collects all UUID-like string values from a JSON subtree (e.g. piping.connections).
/// Used to allow resolution of secrets that appear in the host's materialized config
/// (Constitution 3.2: connect flow driven by materialized config + secrets).
pub(super) fn collect_uuid_values_from_config(value: &Value, out: &mut HashSet<Uuid>) {
    match value {
        Value::String(raw) => {
            if let Ok(Some(secret_id)) = parse_secret_link_id(raw) {
                out.insert(secret_id);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_uuid_values_from_config(item, out);
            }
        }
        Value::Object(map) => {
            for child in map.values() {
                collect_uuid_values_from_config(child, out);
            }
        }
        _ => {}
    }
}

pub(super) fn collect_secret_link_ids(
    value: &Value,
    allowed: &HashSet<Uuid>,
    ids: &mut HashSet<Uuid>,
) {
    match value {
        Value::String(raw) => {
            if let Ok(Some(secret_id)) = parse_secret_link_id(raw)
                && allowed.contains(&secret_id)
            {
                ids.insert(secret_id);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_secret_link_ids(item, allowed, ids);
            }
        }
        Value::Object(map) => {
            for child in map.values() {
                collect_secret_link_ids(child, allowed, ids);
            }
        }
        _ => {}
    }
}

pub(super) fn replace_secret_link_values(
    value: &mut Value,
    resolved_values: &HashMap<Uuid, String>,
) {
    match value {
        Value::String(raw) => {
            if let Ok(Some(secret_id)) = parse_secret_link_id(raw)
                && let Some(resolved) = resolved_values.get(&secret_id)
            {
                *value = Value::String(resolved.clone());
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                replace_secret_link_values(item, resolved_values);
            }
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                replace_secret_link_values(child, resolved_values);
            }
        }
        _ => {}
    }
}

/// The `(host, port)` SSH endpoints a proxy payload must pin: the primary
/// endpoint followed by every `jump` hop. A missing, non-numeric or
/// out-of-range port is [`host_key_pins::DEFAULT_SSH_PORT`] (22), matching how
/// the proxy interprets an omitted port.
pub(super) fn payload_host_key_endpoints(payload: &Value) -> Vec<(String, u16)> {
    let Some(map) = payload.as_object() else {
        return Vec::new();
    };

    let mut endpoints = Vec::new();
    if let Some(host) = map
        .get("host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|host| !host.is_empty())
    {
        endpoints.push((host.to_string(), payload_endpoint_port(map.get("port"))));
    }

    if let Some(Value::Array(hops)) = map.get("jump") {
        for hop in hops {
            let Some(hop) = hop.as_object() else {
                continue;
            };
            let Some(host) = hop
                .get("host")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|host| !host.is_empty())
            else {
                continue;
            };
            endpoints.push((host.to_string(), payload_endpoint_port(hop.get("port"))));
        }
    }

    endpoints
}

fn payload_endpoint_port(value: Option<&Value>) -> u16 {
    value
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .unwrap_or(host_key_pins::DEFAULT_SSH_PORT)
}

/// Insert the resolved `known_hosts` lines for `payload`'s endpoints under
/// [`host_key_pins::PROXY_PAYLOAD_KNOWN_HOSTS_LINES`]. Always writes an array —
/// empty when no endpoint is pinned — so the proxy and the worker read one
/// stable shape. First-seen order is preserved and duplicate lines are dropped.
pub(super) fn merge_known_hosts_lines_into_payload(payload: &mut Value, lines: Vec<String>) {
    let mut merged: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        if !merged.contains(&line) {
            merged.push(line);
        }
    }
    if let Value::Object(map) = payload {
        map.insert(
            host_key_pins::PROXY_PAYLOAD_KNOWN_HOSTS_LINES.to_string(),
            Value::Array(merged.into_iter().map(Value::String).collect()),
        );
    }
}

/// Resolve the pins for every endpoint in `payload` and inject them as
/// `known_hosts_lines`. The proxy trusts only what this list carries, so an
/// unpinned endpoint contributes nothing and an empty list is still written.
pub(super) async fn resolve_and_inject_host_key_pins<R>(
    mut payload: Value,
    repository: &R,
) -> Result<Value, AppError>
where
    R: ConnectionRepository + ?Sized,
{
    let endpoints = payload_host_key_endpoints(&payload);
    let mut lines = Vec::new();
    for (host, port) in endpoints {
        lines.extend(repository.resolve_host_key_lines(&host, port).await?);
    }
    merge_known_hosts_lines_into_payload(&mut payload, lines);
    Ok(payload)
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
    /// Populate `cache` with `record_id`'s `(field_key, secret_id)` bindings once,
    /// reusing the existing entry on repeat leaves of the same record.
    async fn ensure_record_bindings(
        &self,
        cache: &mut HashMap<Uuid, Vec<(String, Uuid)>>,
        record_id: Uuid,
    ) -> Result<(), AppError> {
        if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(record_id) {
            let loaded = self
                .secret_repository
                .list_usage_bindings_for_consumer(CONNECTION_CONFIG_RECORD_CONSUMER_TYPE, record_id)
                .await?
                .into_iter()
                .map(|b| (b.field_key, b.secret_id))
                .collect();
            entry.insert(loaded);
        }
        Ok(())
    }

    /// Fence connection-config-sourced secret UUIDs: a UUID is allowed ONLY when
    /// the record that contributed that leaf path has a `connection_config:record`
    /// binding whose field_key equals the leaf path and whose secret_id equals the
    /// candidate. Never trusts a resolver UUID merely because it is UUID-shaped.
    async fn allowed_connconf_secret_ids(
        &self,
        attributions: &HashMap<Uuid, Vec<SecretLeafAttribution>>,
    ) -> Result<HashSet<Uuid>, AppError> {
        let mut allowed = HashSet::new();
        // Cache bindings per record to avoid repeated lookups across leaves.
        let mut bindings_by_record: HashMap<Uuid, Vec<(String, Uuid)>> = HashMap::new();
        for (candidate_secret_id, sources) in attributions {
            for source in sources {
                self.ensure_record_bindings(&mut bindings_by_record, source.record_id)
                    .await?;
                let bound = bindings_by_record
                    .get(&source.record_id)
                    .is_some_and(|bindings| {
                        bindings.iter().any(|(field_key, secret_id)| {
                            field_key == &source.field_key && secret_id == candidate_secret_id
                        })
                    });
                if bound {
                    allowed.insert(*candidate_secret_id);
                    break;
                }
            }
        }
        Ok(allowed)
    }

    /// Secret link ids referenced by any of `user_id`'s credential sets.
    /// Unpaged: every set can carry a secret link that must stay resolvable.
    async fn allowed_user_credential_set_secret_ids(
        &self,
        user_id: Option<Uuid>,
    ) -> Result<HashSet<Uuid>, AppError> {
        let mut allowed = HashSet::new();
        // A `None` credential user applies no personal overlay, so no set is scanned.
        let Some(user_id) = user_id else {
            return Ok(allowed);
        };
        for set in self
            .repository
            .list_user_credential_sets(
                UserCredentialSetFilter {
                    user_id: vec![user_id],
                    protocol: Vec::new(),
                    is_default: Vec::new(),
                    sort: None,
                },
                None,
                None,
            )
            .await?
            .0
        {
            for binding in self
                .secret_repository
                .list_usage_bindings_for_consumer(
                    USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE,
                    set.id,
                )
                .await?
            {
                allowed.insert(binding.secret_id);
            }
        }
        Ok(allowed)
    }

    /// Add every UUID-like value under `piping.connections` in the host's
    /// materialized config (Constitution 3.2).
    fn collect_config_secret_ids(config: &Value, allowed: &mut HashSet<Uuid>) {
        if let Some(connections) = config
            .get("piping")
            .and_then(|p| p.get("connections"))
            .and_then(Value::as_object)
        {
            for connections_value in connections.values() {
                collect_uuid_values_from_config(connections_value, allowed);
            }
        }
    }

    /// Resolve every secret link id through the secret store, mapping a missing
    /// secret to a validation error.
    async fn resolve_secret_values(
        &self,
        secret_ids: HashSet<Uuid>,
    ) -> Result<HashMap<Uuid, String>, AppError> {
        let mut resolved_values = HashMap::new();
        for secret_id in secret_ids {
            let value = self
                .secret_store
                .get_secret(secret_id)
                .await
                .map_err(|error| {
                    if matches!(error, AppError::MissingSecret(_)) {
                        AppError::Validation(format!(
                            "Secret value for {} is missing or unavailable",
                            secret_id
                        ))
                    } else {
                        error
                    }
                })?;
            resolved_values.insert(secret_id, value);
        }
        Ok(resolved_values)
    }

    /// Resolves secret-link UUIDs in the proxy payload with values from the secret store.
    /// Allowed secrets: (1) connection-target scope bindings for this connection target,
    /// (2) user credential-set bindings, (3) any secret ID present in the target's
    /// materialized config under piping.connections
    /// (Constitution 3.2: connect flow driven by materialized config + secrets; all entries/protocols),
    /// (4) connection-config-sourced secrets, allowed ONLY when the record that
    /// actually contributed that leaf has a matching `secret_usage_binding`
    /// (consumer_type `connection_config:record`, field_key = the leaf path).
    pub(super) async fn resolve_proxy_payload_secrets(
        &self,
        target_id: Uuid,
        user_id: Option<Uuid>,
        mut payload: Value,
        host_config: Option<&Value>,
        // Per-secret-UUID attribution from the connection-config resolver: each
        // candidate UUID -> the `(record_id, field_key)` sources that produced
        // it. A connconf-sourced UUID is allowed through the fence ONLY when one
        // of its contributing records has a `connection_config:record` binding
        // for that field_key pointing at the SAME secret id. This closes the
        // confused-deputy hole where any resolver UUID was blanket-trusted.
        connconf_secret_attributions: Option<&HashMap<Uuid, Vec<SecretLeafAttribution>>>,
    ) -> Result<Value, AppError> {
        let mut allowed_secret_ids = HashSet::new();
        for binding in self
            .secret_repository
            .list_usage_bindings_for_consumer(CONNECTION_TARGET_SECRET_CONSUMER_TYPE, target_id)
            .await?
        {
            allowed_secret_ids.insert(binding.secret_id);
        }
        // Connection-config-sourced secrets: a UUID leaf is allowed ONLY if the
        // record that contributed THAT leaf path has a `connection_config:record`
        // binding whose field_key == the leaf path AND whose secret_id == the
        // UUID. We never trust a resolver UUID just because it is UUID-shaped or
        // because some other record happens to bind it elsewhere.
        if let Some(attributions) = connconf_secret_attributions {
            let allowed = self.allowed_connconf_secret_ids(attributions).await?;
            if matches!(
                payload.get("connection_type").and_then(Value::as_str),
                Some("vnc" | "rdp")
            ) && let Some(credentials) = payload.get("credentials").and_then(Value::as_object)
            {
                for (key, value) in credentials {
                    let Some(id) = value
                        .as_str()
                        .and_then(|raw| Uuid::parse_str(raw.trim()).ok())
                    else {
                        continue;
                    };
                    let is_record_reference = attributions.get(&id).is_some_and(|sources| {
                        sources
                            .iter()
                            .any(|source| source.field_key == format!("credentials.{key}"))
                    });
                    if is_record_reference && !allowed.contains(&id) {
                        return Err(AppError::Validation(
                            "Connection credential reference is no longer bound to its source"
                                .to_string(),
                        ));
                    }
                }
            }
            allowed_secret_ids.extend(allowed);
        }
        // Include secret links from user credential sets so selected-set launches resolve
        // private_key/password UUID links before proxy execution.
        // Unpaged: every set can carry a secret link that must stay resolvable.
        allowed_secret_ids.extend(self.allowed_user_credential_set_secret_ids(user_id).await?);
        if let Some(config) = host_config {
            Self::collect_config_secret_ids(config, &mut allowed_secret_ids);
        }

        let mut secret_ids = HashSet::new();
        collect_secret_link_ids(&payload, &allowed_secret_ids, &mut secret_ids);

        let creds_before = payload.get("credentials").and_then(Value::as_object);
        let password_before = creds_before
            .and_then(|c| c.get("password"))
            .and_then(Value::as_str);
        let password_before_is_uuid =
            password_before.is_some_and(|s| Uuid::parse_str(s.trim()).is_ok());
        let password_before_in_allowed = password_before
            .and_then(|s| Uuid::parse_str(s.trim()).ok())
            .is_some_and(|id| allowed_secret_ids.contains(&id));

        log_event(
            LogLevel::Debug,
            "connection.secrets.resolve.debug",
            EventFields::default()
                .with_operation("resolve_proxy_payload_secrets")
                .with_resource("target_id", target_id.to_string())
                .with_context(
                    "user_id",
                    user_id.map_or_else(|| "none".to_string(), |id| id.to_string()),
                )
                .with_context(
                    "allowed_secrets_count",
                    allowed_secret_ids.len().to_string(),
                )
                .with_context("collected_secret_ids_count", secret_ids.len().to_string())
                .with_context(
                    "credentials_has_password",
                    password_before.is_some().to_string(),
                )
                .with_context(
                    "password_value_is_uuid",
                    password_before_is_uuid.to_string(),
                )
                .with_context(
                    "password_uuid_in_allowed",
                    password_before_in_allowed.to_string(),
                ),
        );

        if secret_ids.is_empty() {
            return Ok(payload);
        }

        let resolved_values = self.resolve_secret_values(secret_ids).await?;

        replace_secret_link_values(&mut payload, &resolved_values);

        let creds_after = payload.get("credentials").and_then(Value::as_object);
        let password_after = creds_after
            .and_then(|c| c.get("password"))
            .and_then(Value::as_str);
        let password_after_still_uuid =
            password_after.is_some_and(|s| Uuid::parse_str(s.trim()).is_ok());
        log_event(
            LogLevel::Debug,
            "connection.secrets.resolve.after",
            EventFields::default()
                .with_operation("resolve_proxy_payload_secrets")
                .with_resource("target_id", target_id.to_string())
                .with_context(
                    "credentials_password_set",
                    password_after.is_some().to_string(),
                )
                .with_context(
                    "password_still_uuid_after_replace",
                    password_after_still_uuid.to_string(),
                ),
        );

        Ok(payload)
    }

    pub(super) fn build_proxy_payload(
        protocol: ConnectionProtocol,
        template_defaults: Option<ConnectionLaunchParams>,
        protocol_params: Option<ConnectionLaunchParams>,
    ) -> Result<Value, AppError> {
        let mut params_map = template_defaults.unwrap_or_default();
        if let Some(params) = protocol_params {
            Self::merge_launch_params_map(
                &mut params_map,
                params,
                UserCredentialSetMergePolicy::OverrideExisting,
            );
        }

        let host = resolve_proxy_host(Self::extract_string(&params_map, &["host"]))?;
        let port = Self::extract_optional_port(&params_map, "port")?;
        // The connection-config resolver emits `timeout_secs`; the proxy contract
        // consumes `timeout`. Accept either, preferring the explicit `timeout`.
        let timeout = Self::extract_optional_f64(&params_map, &["timeout", "timeout_secs"])?;
        let probe_only =
            Self::extract_optional_bool(&params_map, &["probe_only"])?.unwrap_or(false);
        // Bastion/jump chain from the connection-config resolver (SSH only). Each
        // hop is a flat object the SSH proxy turns into a ProxyJump destination;
        // secret refs inside have already been allowed through the fence.
        let jump = params_map.remove("jump");
        let credentials_map = Self::extract_normalized_credentials(&protocol, &mut params_map)?;
        let options_map = Self::extract_normalized_options(&protocol, &mut params_map)?;

        let mut payload = Map::new();
        payload.insert(
            "connection_type".to_string(),
            Value::String(protocol.as_str().to_string()),
        );
        payload.insert("host".to_string(), Value::String(host));
        if let Some(port) = port {
            payload.insert("port".to_string(), Value::Number(port.into()));
        }
        if let Some(timeout) = timeout {
            payload.insert(
                "timeout".to_string(),
                Value::Number(
                    serde_json::Number::from_f64(timeout)
                        .ok_or_else(|| AppError::Validation("Invalid timeout value".to_string()))?,
                ),
            );
        }
        if probe_only {
            payload.insert("probe_only".to_string(), Value::Bool(true));
        }
        payload.insert("credentials".to_string(), Value::Object(credentials_map));
        if !options_map.is_empty() {
            payload.insert("options".to_string(), Value::Object(options_map));
        }
        // Forward a non-empty jump chain to the proxy verbatim (SSH consumes it).
        if let Some(Value::Array(hops)) = jump
            && !hops.is_empty()
        {
            payload.insert("jump".to_string(), Value::Array(hops));
        }

        params_map.remove("host");
        params_map.remove("port");
        params_map.remove("timeout");
        params_map.remove("timeout_secs");
        params_map.remove("probe_only");
        // UI metadata keys are allowed in host-config entries and ignored for launch payload.
        params_map.remove("label");
        params_map.remove("name");
        params_map.remove("icon");
        params_map.remove("color");

        Self::reject_unknown_launch_params(&protocol, &params_map)?;
        Ok(Value::Object(payload))
    }

    pub(super) fn ensure_required_proxy_payload_credentials(
        protocol: &ConnectionProtocol,
        payload: &Value,
    ) -> Result<(), AppError> {
        let Some(payload) = payload.as_object() else {
            return Err(AppError::Validation(
                "Proxy payload must be a JSON object".to_string(),
            ));
        };
        let empty_credentials = Map::new();
        let empty_options = Map::new();
        let credentials = payload
            .get("credentials")
            .and_then(Value::as_object)
            .unwrap_or(&empty_credentials);
        let options = payload
            .get("options")
            .and_then(Value::as_object)
            .unwrap_or(&empty_options);
        ensure_required_proxy_credentials(protocol, credentials, options)
    }

    fn extract_string(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|key| match map.get(*key) {
            Some(Value::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_string())
            }
            _ => None,
        })
    }

    fn extract_optional_f64(
        map: &Map<String, Value>,
        keys: &[&str],
    ) -> Result<Option<f64>, AppError> {
        for key in keys {
            if let Some(value) = map.get(*key) {
                return match value {
                    Value::Number(number) => number
                        .as_f64()
                        .filter(|parsed| *parsed > 0.0)
                        .ok_or_else(|| AppError::Validation(format!("Invalid {} value", key)))
                        .map(Some),
                    Value::Null => Ok(None),
                    _ => Err(AppError::Validation(format!("Invalid {} value", key))),
                };
            }
        }
        Ok(None)
    }

    fn extract_optional_bool(
        map: &Map<String, Value>,
        keys: &[&str],
    ) -> Result<Option<bool>, AppError> {
        for key in keys {
            if let Some(value) = map.get(*key) {
                return match value {
                    Value::Bool(parsed) => Ok(Some(*parsed)),
                    Value::Null => Ok(None),
                    _ => Err(AppError::Validation(format!("Invalid {} value", key))),
                };
            }
        }
        Ok(None)
    }

    fn extract_optional_port(map: &Map<String, Value>, key: &str) -> Result<Option<i64>, AppError> {
        let Some(value) = map.get(key) else {
            return Ok(None);
        };

        let parsed = match value {
            Value::Number(number) => number
                .as_i64()
                .ok_or_else(|| AppError::Validation(format!("Invalid {} value", key)))?,
            Value::Null => return Ok(None),
            _ => return Err(AppError::Validation(format!("Invalid {} value", key))),
        };

        if !(1..=65535).contains(&parsed) {
            return Err(AppError::Validation(format!("Invalid {} value", key)));
        }
        Ok(Some(parsed))
    }

    pub(super) fn extract_normalized_credentials(
        protocol: &ConnectionProtocol,
        params_map: &mut Map<String, Value>,
    ) -> Result<Map<String, Value>, AppError> {
        let credentials_map = match params_map.remove("credentials") {
            Some(Value::Object(map)) => map,
            Some(Value::Null) | None => Map::new(),
            Some(_) => {
                return Err(AppError::Validation(
                    "Connection credentials must be a JSON object".to_string(),
                ));
            }
        };

        Self::normalize_credentials_map(protocol, credentials_map)
    }

    pub(super) fn extract_normalized_options(
        protocol: &ConnectionProtocol,
        params_map: &mut Map<String, Value>,
    ) -> Result<Map<String, Value>, AppError> {
        let options_map = match params_map.remove("options") {
            Some(Value::Object(map)) => map,
            Some(Value::Null) | None => Map::new(),
            Some(_) => {
                return Err(AppError::Validation(
                    "Connection options must be a JSON object".to_string(),
                ));
            }
        };

        Self::normalize_options_map(protocol, options_map)
    }

    pub(super) fn normalize_credentials_map(
        protocol: &ConnectionProtocol,
        credentials: Map<String, Value>,
    ) -> Result<Map<String, Value>, AppError> {
        let allowed: &[&str] = match protocol {
            ConnectionProtocol::Ssh => &["username", "password", "private_key"],
            ConnectionProtocol::Rdp => &["username", "password", "domain"],
            ConnectionProtocol::Vnc => &["password"],
            ConnectionProtocol::App | ConnectionProtocol::Cctv => &[],
        };

        let mut normalized = Map::new();
        for (key, value) in credentials {
            if !allowed.contains(&key.as_str()) {
                return Err(AppError::Validation(format!(
                    "Unsupported credential key '{}' for protocol '{}'",
                    key,
                    protocol.as_str()
                )));
            }

            match key.as_str() {
                "username" | "domain" => {
                    if let Some(value) =
                        Self::parse_trimmed_string_value(&value, &format!("credentials.{key}"))?
                    {
                        normalized.insert(key, Value::String(value));
                    }
                }
                "password" | "private_key" => {
                    if let Some(value) =
                        Self::parse_string_value(&value, &format!("credentials.{key}"))?
                    {
                        normalized.insert(key, Value::String(value));
                    }
                }
                _ => {}
            }
        }
        if *protocol == ConnectionProtocol::Rdp {
            normalize_rdp_credentials_map(&mut normalized)?;
        }
        Ok(normalized)
    }

    pub(super) fn normalize_options_map(
        protocol: &ConnectionProtocol,
        options: Map<String, Value>,
    ) -> Result<Map<String, Value>, AppError> {
        if options.is_empty() {
            return Ok(Map::new());
        }
        if *protocol != ConnectionProtocol::Rdp {
            let mut keys: Vec<&str> = options.keys().map(String::as_str).collect();
            keys.sort_unstable();
            return Err(AppError::Validation(format!(
                "Unsupported connection options for protocol '{}': {}",
                protocol.as_str(),
                keys.join(", ")
            )));
        }

        let allowed: &[&str] = &[
            "color_depth",
            "width",
            "height",
            "dpi",
            "ignore_cert",
            "disable_auth",
            "disable_gfx",
            "security",
            "enable_drive",
        ];
        let mut normalized = Map::new();
        for (key, value) in options {
            if !allowed.contains(&key.as_str()) {
                return Err(AppError::Validation(format!(
                    "Unsupported connection option '{}' for protocol '{}'",
                    key,
                    protocol.as_str()
                )));
            }

            match key.as_str() {
                "color_depth" => {
                    if let Some(value) =
                        Self::parse_color_depth_value(&value, "options.color_depth")?
                    {
                        normalized.insert(key, Value::Number(value.into()));
                    }
                }
                "width" | "height" | "dpi" => {
                    if let Some(value) =
                        Self::parse_positive_i64_value(&value, &format!("options.{key}"))?
                    {
                        normalized.insert(key, Value::Number(value.into()));
                    }
                }
                "ignore_cert" | "disable_auth" | "disable_gfx" | "enable_drive" => {
                    if let Some(value) = Self::parse_bool_value(&value, &format!("options.{key}"))?
                    {
                        normalized.insert(key, Value::Bool(value));
                    }
                }
                "security" => {
                    if let Some(value) = Self::parse_rdp_security_value(&value, "options.security")?
                    {
                        normalized.insert(key, Value::String(value));
                    }
                }
                _ => {}
            }
        }
        Ok(normalized)
    }

    fn parse_trimmed_string_value(value: &Value, field: &str) -> Result<Option<String>, AppError> {
        match value {
            Value::Null => Ok(None),
            Value::String(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Ok(None);
                }
                Ok(Some(trimmed.to_string()))
            }
            _ => Err(AppError::Validation(format!("Invalid {field} value"))),
        }
    }

    fn parse_string_value(value: &Value, field: &str) -> Result<Option<String>, AppError> {
        match value {
            Value::Null => Ok(None),
            Value::String(raw) => {
                if raw.trim().is_empty() {
                    return Ok(None);
                }
                Ok(Some(raw.clone()))
            }
            _ => Err(AppError::Validation(format!("Invalid {field} value"))),
        }
    }

    fn parse_positive_i64_value(value: &Value, field: &str) -> Result<Option<i64>, AppError> {
        match value {
            Value::Null => Ok(None),
            Value::Number(number) => number
                .as_i64()
                .filter(|value| *value > 0)
                .map(Some)
                .ok_or_else(|| AppError::Validation(format!("Invalid {field} value"))),
            _ => Err(AppError::Validation(format!("Invalid {field} value"))),
        }
    }

    fn parse_color_depth_value(value: &Value, field: &str) -> Result<Option<i64>, AppError> {
        let Some(value) = Self::parse_positive_i64_value(value, field)? else {
            return Ok(None);
        };
        if matches!(value, 8 | 16 | 24 | 32) {
            Ok(Some(value))
        } else {
            Err(AppError::Validation(format!("Invalid {field} value")))
        }
    }

    fn parse_bool_value(value: &Value, field: &str) -> Result<Option<bool>, AppError> {
        match value {
            Value::Null => Ok(None),
            Value::Bool(value) => Ok(Some(*value)),
            _ => Err(AppError::Validation(format!("Invalid {field} value"))),
        }
    }

    fn parse_rdp_security_value(value: &Value, field: &str) -> Result<Option<String>, AppError> {
        match value {
            Value::Null => Ok(None),
            Value::String(raw) => {
                let normalized = raw.trim().to_ascii_lowercase();
                if normalized.is_empty() {
                    return Ok(None);
                }
                match normalized.as_str() {
                    "any" | "nla" | "rdp" | "tls" => Ok(Some(normalized)),
                    _ => Err(AppError::Validation(format!("Invalid {field} value"))),
                }
            }
            _ => Err(AppError::Validation(format!("Invalid {field} value"))),
        }
    }

    fn reject_unknown_launch_params(
        protocol: &ConnectionProtocol,
        params: &Map<String, Value>,
    ) -> Result<(), AppError> {
        if params.is_empty() {
            return Ok(());
        }

        let mut keys: Vec<&str> = params.keys().map(String::as_str).collect();
        keys.sort_unstable();
        Err(AppError::Validation(format!(
            "Unsupported launch params for protocol '{}': {}",
            protocol.as_str(),
            keys.join(", ")
        )))
    }
}

fn credential_leaf_present(credentials: &Map<String, Value>, key: &str) -> bool {
    credentials
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn rdp_disable_auth_enabled(options: &Map<String, Value>) -> bool {
    matches!(options.get("disable_auth"), Some(Value::Bool(true)))
}

fn normalize_rdp_credentials_map(credentials: &mut Map<String, Value>) -> Result<(), AppError> {
    let username = credentials
        .remove("username")
        .and_then(|value| value.as_str().map(str::to_string));
    let domain = credentials
        .remove("domain")
        .and_then(|value| value.as_str().map(str::to_string));
    let (username, domain) =
        crate::contexts::connections::domain::normalize_rdp_username_domain(username, domain)
            .map_err(|message| AppError::Validation(message.to_string()))?;
    if let Some(username) = username {
        credentials.insert("username".to_string(), Value::String(username));
    }
    if let Some(domain) = domain {
        credentials.insert("domain".to_string(), Value::String(domain));
    }
    Ok(())
}

fn ensure_required_proxy_credentials(
    protocol: &ConnectionProtocol,
    credentials: &Map<String, Value>,
    options: &Map<String, Value>,
) -> Result<(), AppError> {
    match protocol {
        ConnectionProtocol::Ssh if !credential_leaf_present(credentials, "username") => Err(
            AppError::Validation("SSH credentials.username is required".to_string()),
        ),
        ConnectionProtocol::Rdp
            if !rdp_disable_auth_enabled(options)
                && !credential_leaf_present(credentials, "password") =>
        {
            Err(AppError::Validation(
                "RDP credentials.password is required".to_string(),
            ))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().cloned().expect("test object")
    }

    #[test]
    fn rdp_requires_password_unless_auth_is_disabled() {
        let credentials = object(json!({ "username": "operator@example.test" }));
        let options = Map::new();

        let error =
            ensure_required_proxy_credentials(&ConnectionProtocol::Rdp, &credentials, &options)
                .expect_err("RDP without password should be rejected");

        match error {
            AppError::Validation(message) => {
                assert!(
                    message.contains("credentials.password"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("unexpected error variant: {other:?}"),
        }

        let options = object(json!({ "disable_auth": true }));
        ensure_required_proxy_credentials(&ConnectionProtocol::Rdp, &credentials, &options)
            .expect("disable_auth RDP should not require password");
    }

    #[test]
    fn rdp_accepts_present_password_leaf() {
        let credentials = object(json!({
            "username": "operator@example.test",
            "password": "secret-ref"
        }));

        ensure_required_proxy_credentials(&ConnectionProtocol::Rdp, &credentials, &Map::new())
            .expect("an RDP payload with a password should pass");
    }

    #[test]
    fn normalize_rdp_credentials_map_canonicalizes_backslash_username() {
        let mut credentials = object(json!({
            "username": "GRAD\\operator",
            "password": "secret-ref"
        }));

        normalize_rdp_credentials_map(&mut credentials)
            .expect("domain-qualified username should normalize");

        assert_eq!(
            credentials.get("username").and_then(Value::as_str),
            Some("operator")
        );
        assert_eq!(
            credentials.get("domain").and_then(Value::as_str),
            Some("GRAD")
        );
    }
}
