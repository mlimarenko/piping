//! Map resolved connection configuration to launch params and resolve SSH hop records.
//!
//! The connection-config resolver (`connection_config::resolve_effective_config`)
//! produces a nested param tree whose top-level keys (`host`, `port`,
//! `credentials`, `options`, `timeout_secs`, `jump`, `ui`) already match what the
//! downstream `build_proxy_payload` consumes — EXCEPT:
//!
//! * `ui.*` is presentation-only and must be stripped before launch;
//! * `jump` is a bastion chain that the SSH proxy consumes as a sibling field.
//!
//! Secret leaves are stored canonically as bare-UUID strings, the same shape the
//! proxy payload and the secret-resolution fence already expect (a field is
//! marked secret by a `secret_usage_binding` keyed by field path under the
//! `connection_config:record` consumer, not by any inline marker). This module
//! threads the resolver's per-leaf attribution (which record contributed each
//! leaf, at which dotted field path) into the connect-time fence so each secret
//! UUID can be checked against the binding of the record that ACTUALLY produced
//! it — never blanket-trusted. Hop references are expanded through the record and
//! binding repositories before the same secret fence handles their credentials.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value};
use uuid::Uuid;

use super::*;
use crate::contexts::connection_config::{
    ConnConfScope, ConnConfSource, DeviceScopeIds, ResolvedRecordParams, resolve_record_params,
};
use crate::contexts::secrets::application::CONNECTION_CONFIG_RECORD_CONSUMER_TYPE;
use crate::contexts::secrets::domain::SecretUsageBinding;
use crate::errors::AppError;

/// Authorizes every record context before any SSH credential is retrieved.
#[async_trait::async_trait]
pub trait SshRecordDelegationAuthorizer: Send + Sync {
    /// Apply the caller's live delegation policy to all supplied record IDs.
    async fn authorize_records(&self, record_ids: &[Uuid]) -> Result<(), AppError>;
}

/// Scope of the delegated SSH record's host-key projection.
#[derive(Clone, Copy)]
pub enum SshRecordPurpose {
    /// Inventory supplies endpoints; explicit record trust endpoints are included.
    NativeDefaults,
    /// The record's own endpoint is the only target.
    EndpointBound,
}

struct PreparedRecordSsh {
    payload: Value,
    keys: Vec<(Option<usize>, Uuid, SecretLeafAttribution)>,
    records: HashMap<Uuid, ConnConfRecord>,
    context_ids: Vec<Uuid>,
    native_endpoints: Vec<(String, u16)>,
}

/// Reject malformed routes before payload construction can discard them.
pub(super) fn validate_jump_shape(jump: &Value) -> Result<(), AppError> {
    let invalid = || AppError::Validation("Invalid SSH jump route".to_string());
    let hops = jump.as_array().ok_or_else(invalid)?;
    // Match the connection-config resolver's documented jump-chain cap.
    if hops.len() > 8 {
        return Err(invalid());
    }
    let mut records = HashSet::new();
    for hop in hops {
        let hop = hop.as_object().ok_or_else(invalid)?;
        if hop.contains_key("jump") {
            return Err(invalid());
        }
        if let Some(record) = hop.get("connconf_record_id") {
            let id = record
                .as_str()
                .and_then(|value| Uuid::parse_str(value).ok())
                .ok_or_else(invalid)?;
            if hop.len() != 1 || !records.insert(id) {
                return Err(invalid());
            }
        }
        if hop
            .get("credentials")
            .and_then(Value::as_object)
            .is_some_and(|credentials| credentials.contains_key("password"))
        {
            return Err(AppError::Validation(
                "SSH jump hops do not support password authentication".to_string(),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_resolved_jump_credentials(payload: &Value) -> Result<(), AppError> {
    let Some(jump) = payload.get("jump") else {
        return Ok(());
    };
    for hop in jump.as_array().ok_or_else(invalid_hop)? {
        let credentials = hop
            .get("credentials")
            .and_then(Value::as_object)
            .ok_or_else(invalid_hop)?;
        let key = credentials
            .get("private_key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid_hop)?;
        if Uuid::parse_str(key).is_ok() {
            return Err(invalid_hop());
        }
    }
    Ok(())
}

fn invalid_hop() -> AppError {
    AppError::Validation("SSH jump record is unavailable or invalid".to_string())
}

fn jump_key_is_bound(
    bindings: &[SecretUsageBinding],
    key_id: Uuid,
    source: &SecretLeafAttribution,
) -> bool {
    bindings.iter().any(|binding| {
        binding.consumer_type == CONNECTION_CONFIG_RECORD_CONSUMER_TYPE
            && binding.consumer_id == source.record_id
            && binding.field_key == source.field_key
            && binding.secret_id == key_id
    })
}

fn jump_host_for_payload(host: &str) -> Result<String, AppError> {
    normalize_endpoint_host_value(host)
        .ok_or_else(|| AppError::Validation("SSH jump host must be an IP literal".to_string()))
}

/// A launch override may retain a selected record's reference, but cannot
/// introduce a different record or move it to an unattributed route position.
fn trusted_jump_reference(
    attributions: &HashMap<Uuid, Vec<SecretLeafAttribution>>,
    index: usize,
    record_id: Uuid,
) -> Result<(), AppError> {
    let field_key = format!("jump.{index}.connconf_record_id");
    if attributions
        .get(&record_id)
        .is_some_and(|sources| sources.iter().any(|source| source.field_key == field_key))
    {
        Ok(())
    } else {
        Err(invalid_hop())
    }
}

fn validate_jump_record_access(
    record: &ConnConfRecord,
    user_id: Option<Uuid>,
) -> Result<(), AppError> {
    if !record.enabled
        || record.protocol != ConnConfProtocol::Ssh
        || record
            .owner_user_id
            .is_some_and(|owner| Some(owner) != user_id)
    {
        return Err(invalid_hop());
    }
    Ok(())
}

fn same_record_revision(prepared: &ConnConfRecord, current: &ConnConfRecord) -> bool {
    prepared.id == current.id
        && prepared.updated_at == current.updated_at
        && prepared.protocol == current.protocol
        && prepared.enabled == current.enabled
        && prepared.owner_user_id == current.owner_user_id
        && prepared.base_record_id == current.base_record_id
        && prepared.params == current.params
        && prepared.custom_field_data == current.custom_field_data
}

fn resolve_loaded_jump_record(
    id: Uuid,
    records: &[ConnConfRecord],
    user_id: Option<Uuid>,
) -> Result<(Value, Uuid, SecretLeafAttribution), AppError> {
    let by_id: HashMap<_, _> = records.iter().map(|record| (record.id, record)).collect();
    let leaf = by_id.get(&id).ok_or_else(invalid_hop)?;
    let resolved = resolve_record_params(leaf, &by_id).map_err(|_| invalid_hop())?;
    for id in &resolved.chain {
        validate_jump_record_access(by_id.get(id).ok_or_else(invalid_hop)?, user_id)?;
    }
    resolve_direct_jump_record(&resolved)
}

fn resolve_direct_jump_record(
    record: &ResolvedRecordParams,
) -> Result<(Value, Uuid, SecretLeafAttribution), AppError> {
    let params = record.params.as_object().ok_or_else(invalid_hop)?;
    if params
        .get("jump")
        .is_some_and(|jump| !jump.as_array().is_some_and(Vec::is_empty))
    {
        return Err(AppError::Validation(
            "Nested SSH jump routes are not supported".to_string(),
        ));
    }
    let mut result = Map::new();
    for field in ["host", "port"] {
        if let Some(value) = params.get(field) {
            result.insert(field.to_string(), value.clone());
        }
    }
    let credentials = params
        .get("credentials")
        .and_then(Value::as_object)
        .ok_or_else(invalid_hop)?
        .clone();
    if credentials
        .keys()
        .any(|field| !matches!(field.as_str(), "username" | "private_key" | "password"))
    {
        return Err(invalid_hop());
    }
    if credentials.contains_key("password") {
        return Err(AppError::Validation(
            "SSH jump hops do not support password authentication".to_string(),
        ));
    }
    for (map, field) in [(&result, "host"), (&credentials, "username")] {
        if !map
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty() && !s.contains("{{") && !s.contains("}}"))
        {
            return Err(invalid_hop());
        }
    }
    if let Some(port) = result.get("port")
        && !port.as_u64().is_some_and(|p| (1..=65535).contains(&p))
    {
        return Err(invalid_hop());
    }
    let key_id = credentials
        .get("private_key")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value.trim()).ok())
        .ok_or_else(invalid_hop)?;
    let source = SecretLeafAttribution {
        record_id: record
            .attribution
            .get("credentials.private_key")
            .ok_or_else(invalid_hop)?
            .record_id,
        field_key: "credentials.private_key".to_string(),
    };
    result.insert("credentials".to_string(), Value::Object(credentials));
    Ok((Value::Object(result), key_id, source))
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
    /// Resolve an SSH record for a caller that explicitly delegates all record contexts.
    ///
    /// # Errors
    /// Rejects inaccessible records, unsupported credentials, revoked bindings and denied delegation.
    pub async fn resolve_delegated_ssh_record(
        &self,
        record_id: Uuid,
        credential_user_id: Option<Uuid>,
        purpose: SshRecordPurpose,
        authorizer: &dyn SshRecordDelegationAuthorizer,
    ) -> Result<Value, AppError> {
        let mut prepared = self
            .prepare_delegated_ssh_record(record_id, credential_user_id)
            .await?;
        self.authorize_prepared_ssh_record(&prepared, credential_user_id, authorizer)
            .await?;
        prepared.payload = proxy_payload::resolve_and_inject_host_key_pins(
            prepared.payload,
            self.repository.as_ref(),
        )
        .await?;
        if matches!(purpose, SshRecordPurpose::NativeDefaults) {
            let mut lines = crate::contexts::shared::host_key_pins::known_hosts_lines_from_payload(
                &prepared.payload,
            );
            for (host, port) in &prepared.native_endpoints {
                let found = self.repository.resolve_host_key_lines(host, *port).await?;
                let usable: Vec<_> = found
                    .into_iter()
                    .filter(|line| {
                        crate::contexts::shared::host_key_pins::parse_known_hosts_line(line)
                            .is_ok_and(|pin| {
                                pin.host.eq_ignore_ascii_case(host) && pin.port == *port
                            })
                    })
                    .collect();
                if usable.is_empty() {
                    return Err(AppError::Validation(
                        "Declared native SSH endpoint has no usable host key pin".into(),
                    ));
                }
                lines.extend(usable);
            }
            proxy_payload::merge_known_hosts_lines_into_payload(&mut prepared.payload, lines);
        }
        for (index, key, _) in prepared.keys {
            let value = self
                .secret_store
                .get_secret(key)
                .await
                .map_err(|_| invalid_hop())?;
            let endpoint = match index {
                None => &mut prepared.payload,
                Some(index) => prepared
                    .payload
                    .get_mut("jump")
                    .and_then(Value::as_array_mut)
                    .and_then(|hops| hops.get_mut(index))
                    .ok_or_else(invalid_hop)?,
            };
            endpoint
                .get_mut("credentials")
                .and_then(Value::as_object_mut)
                .ok_or_else(invalid_hop)?
                .insert("private_key".into(), Value::String(value));
        }
        Ok(prepared.payload)
    }

    /// Authorize a record's exact inherited route without retrieving credential values.
    /// The returned contexts form the launch-time ceiling for a later dispatch.
    pub async fn authorize_ssh_record_delegation(
        &self,
        record_id: Uuid,
        credential_user_id: Option<Uuid>,
        authorizer: &dyn SshRecordDelegationAuthorizer,
    ) -> Result<Vec<Uuid>, AppError> {
        let prepared = self
            .prepare_delegated_ssh_record(record_id, credential_user_id)
            .await?;
        self.authorize_prepared_ssh_record(&prepared, credential_user_id, authorizer)
            .await?;
        Ok(prepared.context_ids)
    }

    async fn prepare_delegated_ssh_record(
        &self,
        record_id: Uuid,
        credential_user_id: Option<Uuid>,
    ) -> Result<PreparedRecordSsh, AppError> {
        let records = self
            .load_authorized_ssh_record_chain(record_id, credential_user_id)
            .await?;
        let by_id: HashMap<_, _> = records.iter().map(|r| (r.id, r)).collect();
        let root = resolve_record_params(by_id.get(&record_id).ok_or_else(invalid_hop)?, &by_id)
            .map_err(|_| invalid_hop())?;
        let native_endpoints =
            crate::contexts::connection_config::domain::record::native_host_key_endpoints(
                &root.params,
            )?;
        let mut prepared_records: HashMap<_, _> = records
            .iter()
            .map(|record| (record.id, record.clone()))
            .collect();
        let mut contexts: Vec<Uuid> = records.iter().map(|r| r.id).collect();
        let mut root_without_jump = root.clone();
        let jump = root_without_jump
            .params
            .as_object_mut()
            .ok_or_else(invalid_hop)?
            .remove("jump");
        let (mut payload, root_key, root_source) = resolve_direct_jump_record(&root_without_jump)?;
        payload
            .as_object_mut()
            .ok_or_else(invalid_hop)?
            .insert("connection_type".into(), Value::String("ssh".into()));
        let mut keys = vec![(None, root_key, root_source)];
        if let Some(jump) = jump {
            validate_jump_shape(&jump)?;
            let mut expanded = Vec::new();
            for hop in jump.as_array().ok_or_else(invalid_hop)? {
                let id = hop
                    .get("connconf_record_id")
                    .and_then(Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or_else(invalid_hop)?;
                let chain = self
                    .load_authorized_ssh_record_chain(id, credential_user_id)
                    .await?;
                for record in &chain {
                    if let Some(prepared) = prepared_records.get(&record.id)
                        && !same_record_revision(prepared, record)
                    {
                        return Err(AppError::Conflict(
                            "SSH record changed during credential preparation".into(),
                        ));
                    }
                    prepared_records.insert(record.id, record.clone());
                }
                contexts.extend(chain.iter().map(|r| r.id));
                let (hop, key, source) =
                    resolve_loaded_jump_record(id, &chain, credential_user_id)?;
                keys.push((Some(expanded.len()), key, source));
                expanded.push(hop);
            }
            if !expanded.is_empty() {
                payload
                    .as_object_mut()
                    .ok_or_else(invalid_hop)?
                    .insert("jump".into(), Value::Array(expanded));
            }
        }
        let mut endpoints = HashSet::new();
        let mut normalize = |endpoint: &mut Value| -> Result<(), AppError> {
            let host = endpoint
                .get("host")
                .and_then(Value::as_str)
                .ok_or_else(invalid_hop)?;
            let host = jump_host_for_payload(host)?;
            let port = match endpoint.get("port") {
                None => 22,
                Some(value) => value
                    .as_u64()
                    .filter(|p| (1..=65535).contains(p))
                    .ok_or_else(invalid_hop)?,
            };
            if !endpoints.insert((host.to_ascii_lowercase(), port)) {
                return Err(invalid_hop());
            }
            endpoint
                .as_object_mut()
                .ok_or_else(invalid_hop)?
                .insert("host".into(), Value::String(host));
            Ok(())
        };
        normalize(&mut payload)?;
        if let Some(hops) = payload.get_mut("jump").and_then(Value::as_array_mut) {
            for hop in hops {
                normalize(hop)?;
            }
        }
        contexts.sort_unstable();
        contexts.dedup();
        Ok(PreparedRecordSsh {
            payload,
            keys,
            records: prepared_records,
            context_ids: contexts,
            native_endpoints,
        })
    }

    async fn authorize_prepared_ssh_record(
        &self,
        prepared: &PreparedRecordSsh,
        credential_user_id: Option<Uuid>,
        authorizer: &dyn SshRecordDelegationAuthorizer,
    ) -> Result<(), AppError> {
        authorizer.authorize_records(&prepared.context_ids).await?;
        for id in &prepared.context_ids {
            let record = self
                .connconf_records
                .find(*id)
                .await?
                .ok_or_else(invalid_hop)?;
            validate_jump_record_access(&record, credential_user_id)?;
            if !prepared
                .records
                .get(id)
                .is_some_and(|snapshot| same_record_revision(snapshot, &record))
            {
                return Err(AppError::Conflict(
                    "SSH record changed during credential authorization".into(),
                ));
            }
        }
        // Recheck current ownership and binding after authorization, before any key retrieval.
        for (_, key, source) in &prepared.keys {
            let record = self
                .connconf_records
                .find(source.record_id)
                .await?
                .ok_or_else(invalid_hop)?;
            validate_jump_record_access(&record, credential_user_id)?;
            if !prepared
                .records
                .get(&source.record_id)
                .is_some_and(|snapshot| same_record_revision(snapshot, &record))
            {
                return Err(AppError::Conflict(
                    "SSH credential source changed during authorization".into(),
                ));
            }
            let (scope, scope_id) = record
                .owner_user_id
                .map_or(("connection_config", record.id), |id| ("user", id));
            let bindings = self
                .secret_repository
                .list_usage_bindings_for_consumer(
                    CONNECTION_CONFIG_RECORD_CONSUMER_TYPE,
                    source.record_id,
                )
                .await?;
            if !bindings.iter().any(|b| {
                b.consumer_type == CONNECTION_CONFIG_RECORD_CONSUMER_TYPE
                    && b.consumer_id == source.record_id
                    && b.field_key == source.field_key
                    && b.secret_id == *key
                    && b.owner_scope_type == scope
                    && b.owner_scope_id == scope_id
            }) {
                return Err(invalid_hop());
            }
        }
        Ok(())
    }

    /// Load an SSH record and its template chain without requiring a target.
    /// Each record is checked before its contents can contribute to the result.
    pub(super) async fn load_authorized_ssh_record_chain(
        &self,
        id: Uuid,
        user_id: Option<Uuid>,
    ) -> Result<Vec<ConnConfRecord>, AppError> {
        let mut records = Vec::new();
        let mut next = Some(id);
        let mut seen = HashSet::new();
        while let Some(record_id) = next {
            if !seen.insert(record_id) || records.len() >= 8 {
                return Err(invalid_hop());
            }
            let record = self
                .connconf_records
                .find(record_id)
                .await?
                .ok_or_else(invalid_hop)?;
            validate_jump_record_access(&record, user_id)?;
            next = record.base_record_id;
            records.push(record);
        }
        Ok(records)
    }

    /// Expand references once for all launch paths, before the existing secret fence.
    pub(super) async fn resolve_jump_references(
        &self,
        payload: &mut Value,
        user_id: Option<Uuid>,
        attributions: &mut HashMap<Uuid, Vec<SecretLeafAttribution>>,
    ) -> Result<(), AppError> {
        let Some(jump) = payload.get_mut("jump") else {
            return Ok(());
        };
        validate_jump_shape(jump)?;
        let hops = jump.as_array_mut().ok_or_else(invalid_hop)?;
        for (index, hop) in hops.iter_mut().enumerate() {
            let Some(raw_id) = hop.get("connconf_record_id").and_then(Value::as_str) else {
                continue;
            };
            let id = Uuid::parse_str(raw_id).map_err(|_| invalid_hop())?;
            trusted_jump_reference(attributions, index, id)?;
            let records = self.load_authorized_ssh_record_chain(id, user_id).await?;
            let (resolved, key_id, source) = resolve_loaded_jump_record(id, &records, user_id)?;
            let bindings = self
                .secret_repository
                .list_usage_bindings_for_consumer(
                    CONNECTION_CONFIG_RECORD_CONSUMER_TYPE,
                    source.record_id,
                )
                .await?;
            if !jump_key_is_bound(&bindings, key_id, &source) {
                return Err(invalid_hop());
            }
            attributions.entry(key_id).or_default().push(source);
            *hop = resolved;
        }
        let mut endpoints = HashSet::new();
        for hop in hops.iter_mut() {
            let host = hop
                .get("host")
                .and_then(Value::as_str)
                .ok_or_else(invalid_hop)?;
            let host = jump_host_for_payload(host)?;
            let port = match hop.get("port") {
                None => 22,
                Some(value) => value
                    .as_u64()
                    .filter(|p| (1..=65535).contains(p))
                    .ok_or_else(invalid_hop)?,
            };
            if !endpoints.insert((host.to_ascii_lowercase(), port)) {
                return Err(invalid_hop());
            }
            hop.as_object_mut()
                .ok_or_else(invalid_hop)?
                .insert("host".to_string(), Value::String(host));
        }
        Ok(())
    }
}

/// Translate a device/VM scope ancestry into the `(scope_type, scope_id)` pairs
/// the resolver matches on, plus the implicit `global` scope. Mirrors the same
/// mapping used by the `connection_config` REST route. Pure, no SQL.
pub(super) fn build_device_scopes(scopes: &DeviceScopeIds) -> Vec<(ConnConfScope, Option<Uuid>)> {
    scopes.to_scope_pairs()
}

/// The (record, dotted field path) that contributed a particular secret-UUID
/// leaf, per the resolver's per-leaf attribution. The connect-time fence checks
/// each candidate against the `secret_usage_binding(consumer_type=
/// 'connection_config:record', consumer_id=record_id, field_key=field_key)` of
/// the record that ACTUALLY produced it — never a blanket trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SecretLeafAttribution {
    pub record_id: Uuid,
    pub field_key: String,
}

/// Outcome of mapping a resolved connection-config param tree to launch params.
pub(super) struct MappedConnConfParams {
    /// Launch-param fields in the shape `build_proxy_payload` consumes
    /// (`host`/`port`/`credentials`/`options`/`timeout_secs`/`jump`). Secret
    /// leaves are canonical bare-UUID strings.
    pub fields: Map<String, Value>,
    /// Per-secret-UUID attribution: each bare-UUID leaf maps to the set of
    /// `(record_id, field_key)` sources that contributed it. The fence resolves
    /// a UUID only if at least one of these records has a matching binding.
    pub secret_attributions: HashMap<Uuid, Vec<SecretLeafAttribution>>,
}

/// Walk `value` at `path` (resolver dotted-path convention: object keys joined by
/// `.`, array indices as numeric segments) and, for every bare-UUID string leaf,
/// record its `(record_id, field_key)` attribution drawn from `attribution`.
/// The `field_key` stored in `secret_usage_binding` for a connection-config
/// record is the SAME dotted leaf path the resolver emits (see migration
/// `0006_init_connconf_backfill.sql`: `field_key = 'credentials.<k>'`).
fn collect_secret_attributions(
    value: &Value,
    path: &str,
    attribution: &BTreeMap<String, ConnConfSource>,
    out: &mut HashMap<Uuid, Vec<SecretLeafAttribution>>,
) {
    match value {
        Value::String(raw) => {
            let Ok(id) = Uuid::parse_str(raw.trim()) else {
                return;
            };
            // `username` is never a secret field: a UUID-shaped username is a
            // literal value, not a secret reference. Refuse to treat it as a
            // resolvable secret even if a stale binding exists (finding 4b).
            if path == "username" || path.ends_with(".username") {
                return;
            }
            // A UUID leaf is resolvable only when the resolver attributed this
            // exact path to a record; that record's binding is the authority.
            if let Some(source) = attribution.get(path) {
                let entry = SecretLeafAttribution {
                    record_id: source.record_id,
                    field_key: path.to_string(),
                };
                let bucket = out.entry(id).or_default();
                if !bucket.contains(&entry) {
                    bucket.push(entry);
                }
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                collect_secret_attributions(child, &child_path, attribution, out);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let child_path = if path.is_empty() {
                    index.to_string()
                } else {
                    format!("{path}.{index}")
                };
                collect_secret_attributions(child, &child_path, attribution, out);
            }
        }
        _ => {}
    }
}

/// Map a resolved effective-config `params` object to launch-param fields.
///
/// Returns `None` when `params` is not an object (no usable config). `ui` is
/// stripped; `jump` (if present) is preserved as a sibling field for the SSH
/// proxy; bare-UUID secret references are paired with the resolver's per-leaf
/// attribution so the connect-time fence can enforce per-record bindings.
pub(super) fn map_effective_params(
    params: &Value,
    attribution: &BTreeMap<String, ConnConfSource>,
) -> Option<MappedConnConfParams> {
    let Value::Object(source) = params else {
        return None;
    };
    let mut fields: Map<String, Value> = source.clone();
    // Presentation-only; never reaches the proxy launch payload.
    fields.remove("ui");
    fields.remove("native_host_key_endpoints");

    // Attribution is keyed on the FULL resolved-params path tree (including the
    // stripped `ui`), so walk the original `params` to preserve path alignment.
    let mut secret_attributions = HashMap::new();
    collect_secret_attributions(params, "", attribution, &mut secret_attributions);
    // The stripped `ui` subtree never reaches the proxy and must not contribute
    // resolvable secrets; drop any attributions whose only sources are ui leaves.
    secret_attributions.retain(|_, srcs| {
        srcs.retain(|s| {
            !(s.field_key == "ui"
                || s.field_key.starts_with("ui.")
                || s.field_key == "native_host_key_endpoints"
                || s.field_key.starts_with("native_host_key_endpoints."))
        });
        !srcs.is_empty()
    });

    Some(MappedConnConfParams {
        fields,
        secret_attributions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ordinary_proxy_mapping_excludes_native_trust_selector() {
        let params = json!({"host":"192.0.2.1", "native_host_key_endpoints":[{"host":"inventory.example.test"}]});
        let mapped = map_effective_params(&params, &BTreeMap::new()).unwrap();
        assert!(!mapped.fields.contains_key("native_host_key_endpoints"));
        assert_eq!(mapped.fields["host"], "192.0.2.1");
    }

    fn jump_record(params: Value) -> ConnConfRecord {
        ConnConfRecord {
            id: Uuid::new_v4(),
            name: "jump".to_string(),
            description: String::new(),
            protocol: ConnConfProtocol::Ssh,
            base_record_id: None,
            enabled: true,
            owner_user_id: None,
            params,
            custom_field_data: json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn resolve_one(
        record: &ConnConfRecord,
    ) -> Result<(Value, Uuid, SecretLeafAttribution), AppError> {
        resolve_loaded_jump_record(
            record.id,
            std::slice::from_ref(record),
            record.owner_user_id,
        )
    }

    #[test]
    fn direct_jump_uses_its_own_secret_source_and_rejects_cycles() {
        let key = Uuid::new_v4();
        let mut record = jump_record(json!({
            "host": "192.0.2.1", "port": 2222,
            "credentials": {"username": "operator", "private_key": key},
            "ui": {"label": "not transported"}
        }));
        let (hop, secret, source) = resolve_one(&record).expect("direct hop");
        assert_eq!(secret, key);
        assert_eq!(source.record_id, record.id);
        assert_eq!(source.field_key, "credentials.private_key");
        assert_eq!(
            hop,
            json!({"host": "192.0.2.1", "port": 2222,
            "credentials": {"username": "operator", "private_key": key}})
        );
        record.base_record_id = Some(record.id);
        assert!(resolve_one(&record).is_err());
    }

    #[test]
    fn inherited_jump_uses_defining_record_and_requires_access_to_base() {
        let key = Uuid::new_v4();
        let mut base = jump_record(
            json!({"host": "192.0.2.1", "credentials": {"username": "operator", "private_key": key}}),
        );
        let mut child = jump_record(json!({"port": 2222, "jump": []}));
        child.base_record_id = Some(base.id);
        let (_, actual_key, source) =
            resolve_loaded_jump_record(child.id, &[child.clone(), base.clone()], None).unwrap();
        assert_eq!(actual_key, key);
        assert_eq!(source.record_id, base.id);
        assert_eq!(source.field_key, "credentials.private_key");
        let mut binding = SecretUsageBinding {
            id: Uuid::new_v4(),
            consumer_type: CONNECTION_CONFIG_RECORD_CONSUMER_TYPE.to_string(),
            consumer_id: base.id,
            field_key: source.field_key.clone(),
            owner_scope_type: "global".to_string(),
            owner_scope_id: Uuid::nil(),
            secret_id: key,
        };
        assert!(jump_key_is_bound(
            std::slice::from_ref(&binding),
            key,
            &source
        ));
        binding.consumer_id = child.id;
        assert!(!jump_key_is_bound(
            std::slice::from_ref(&binding),
            key,
            &source
        ));
        binding.consumer_id = base.id;
        binding.field_key = "credentials.password".to_string();
        assert!(!jump_key_is_bound(
            std::slice::from_ref(&binding),
            key,
            &source
        ));
        base.owner_user_id = Some(Uuid::new_v4());
        assert!(
            resolve_loaded_jump_record(child.id, &[child.clone(), base.clone()], None).is_err()
        );
        assert!(resolve_loaded_jump_record(child.id, std::slice::from_ref(&child), None).is_err());
        base.owner_user_id = None;
        base.enabled = false;
        assert!(
            resolve_loaded_jump_record(child.id, &[child.clone(), base.clone()], None).is_err()
        );
        base.enabled = true;
        let override_key = Uuid::new_v4();
        child.params["credentials"] = json!({"private_key": override_key});
        let (_, actual_key, source) =
            resolve_loaded_jump_record(child.id, &[child.clone(), base.clone()], None).unwrap();
        assert_eq!(actual_key, override_key);
        assert_eq!(source.record_id, child.id);
        child.params.as_object_mut().unwrap().remove("credentials");
        base.base_record_id = Some(child.id);
        assert!(
            resolve_loaded_jump_record(child.id, &[child.clone(), base.clone()], None).is_err()
        );
        base.base_record_id = None;
        base.params["jump"] = json!([{"connconf_record_id": child.id}]);
        child.params.as_object_mut().unwrap().remove("jump");
        assert!(resolve_loaded_jump_record(child.id, &[child, base], None).is_err());
    }

    #[test]
    fn inherited_jump_accepts_explicitly_cleared_password_in_new_credentials() {
        let base = jump_record(json!({"host": "192.0.2.1"}));
        let key = Uuid::new_v4();
        let mut child = jump_record(json!({"credentials": {
            "username": "operator", "private_key": key, "password": {"unset": true}
        }}));
        child.base_record_id = Some(base.id);
        let (hop, actual_key, source) =
            resolve_loaded_jump_record(child.id, &[child.clone(), base], None).unwrap();
        assert_eq!(actual_key, key);
        assert_eq!(source.record_id, child.id);
        assert!(hop["credentials"].get("password").is_none());
    }

    #[test]
    fn jump_reference_must_be_attributed_to_selected_config_at_same_position() {
        let trusted = Uuid::new_v4();
        let other = Uuid::new_v4();
        let params = json!({"jump": [{"connconf_record_id": trusted}]});
        let mut attribution = BTreeMap::new();
        attribution.insert("jump.0.connconf_record_id".to_string(), source(7));
        let mapped = map_effective_params(&params, &attribution).unwrap();

        assert!(trusted_jump_reference(&mapped.secret_attributions, 0, trusted).is_ok());
        assert!(trusted_jump_reference(&mapped.secret_attributions, 0, other).is_err());
        assert!(trusted_jump_reference(&mapped.secret_attributions, 1, trusted).is_err());
        assert!(trusted_jump_reference(&HashMap::new(), 0, trusted).is_err());
    }

    #[test]
    fn direct_jump_rejects_invalid_credentials_nested_routes_and_access() {
        let key = Uuid::new_v4();
        let mut record = jump_record(json!({"host": "192.0.2.1",
            "credentials": {"username": "operator", "private_key": key}}));
        assert!(validate_jump_record_access(&record, None).is_ok());
        record.enabled = false;
        assert!(validate_jump_record_access(&record, None).is_err());
        record.enabled = true;
        record.protocol = ConnConfProtocol::Rdp;
        assert!(validate_jump_record_access(&record, None).is_err());
        record.protocol = ConnConfProtocol::Ssh;
        let owner = Uuid::new_v4();
        record.owner_user_id = Some(owner);
        assert!(validate_jump_record_access(&record, None).is_err());
        assert!(validate_jump_record_access(&record, Some(owner)).is_ok());
        record.params["jump"] = json!([]);
        assert!(resolve_one(&record).is_ok());
        record.params["jump"] = json!([{"connconf_record_id": Uuid::new_v4()}]);
        assert!(resolve_one(&record).is_err());
        record.params.as_object_mut().unwrap().remove("jump");
        record.params["credentials"]["password"] = json!("not supported");
        assert!(resolve_one(&record).is_err());
        record.params["credentials"] =
            json!({"username": "operator", "private_key": "not-a-binding"});
        assert!(resolve_one(&record).is_err());
    }

    #[test]
    fn resolved_hops_require_actual_private_keys() {
        assert!(
            validate_resolved_jump_credentials(
                &json!({"jump": [{"credentials": {"private_key": "key material"}}]})
            )
            .is_ok()
        );
        assert!(
            validate_resolved_jump_credentials(
                &json!({"jump": [{"credentials": {"private_key": Uuid::new_v4()}}]})
            )
            .is_err()
        );
        assert!(
            validate_resolved_jump_credentials(&json!({"jump": [{"credentials": {}}]})).is_err()
        );
    }

    #[test]
    fn jump_hostname_is_rejected_before_transport_and_pinning() {
        let record = jump_record(json!({"host": "  bastion.example  ",
            "credentials": {"username": "operator", "private_key": Uuid::new_v4()}}));
        let (hop, _, _) = resolve_one(&record).unwrap();
        assert!(jump_host_for_payload(hop["host"].as_str().unwrap()).is_err());
        let host = jump_host_for_payload("[2001:db8::1]").unwrap();
        assert_eq!(host, "2001:db8::1");
        assert!(jump_host_for_payload("  ").is_err());
    }

    #[test]
    fn jump_shape_rejects_invalid_and_unsupported_routes() {
        let id = Uuid::new_v4();
        for value in [
            json!({}),
            json!([null]),
            json!([{"connconf_record_id": "bad"}]),
            json!([{"connconf_record_id": id}, {"connconf_record_id": id}]),
            json!([{"jump": []}]),
            json!([{"credentials": {"password": "secret"}}]),
        ] {
            assert!(validate_jump_shape(&value).is_err(), "accepted {value}");
        }
        assert!(validate_jump_shape(&json!([])).is_ok());
        assert!(validate_jump_shape(&json!([{"connconf_record_id": id}])).is_ok());
        assert!(validate_jump_shape(&json!([{"host": "192.0.2.1", "credentials": {"username": "operator", "private_key": "key"}}])).is_ok());
    }

    fn source(record: u128) -> ConnConfSource {
        ConnConfSource {
            record_id: Uuid::from_u128(record),
            record_name: format!("rec-{record}"),
            scope_type: None,
            scope_id: None,
        }
    }

    #[test]
    fn attributes_secret_to_contributing_record_and_strips_ui() {
        let uuid = "11111111-1111-1111-1111-111111111111";
        let params = json!({
            "host": "10.0.0.1",
            "port": 2222,
            "credentials": {
                "username": "root",
                "password": uuid
            },
            "ui": { "label": "prod", "color": "#0f0" }
        });
        let mut attribution: BTreeMap<String, ConnConfSource> = BTreeMap::new();
        attribution.insert("credentials.password".to_string(), source(7));

        let mapped = map_effective_params(&params, &attribution).expect("mapped");
        assert_eq!(mapped.fields["host"], json!("10.0.0.1"));
        assert_eq!(mapped.fields["port"], json!(2222));
        assert_eq!(
            mapped.fields["credentials"]["password"],
            json!(uuid),
            "secret leaf is the canonical bare uuid string"
        );
        assert!(!mapped.fields.contains_key("ui"), "ui must be stripped");
        let attrs = mapped
            .secret_attributions
            .get(&Uuid::parse_str(uuid).unwrap())
            .expect("uuid attributed");
        assert_eq!(
            attrs,
            &vec![SecretLeafAttribution {
                record_id: Uuid::from_u128(7),
                field_key: "credentials.password".to_string(),
            }]
        );
    }

    #[test]
    fn unattributed_uuid_leaf_is_not_resolvable() {
        // A UUID-shaped leaf with no resolver attribution (e.g. an arbitrary
        // string field) must NOT be offered to the fence.
        let uuid = "22222222-2222-2222-2222-222222222222";
        let params = json!({ "host": "10.0.0.1", "credentials": { "username": uuid } });
        let attribution: BTreeMap<String, ConnConfSource> = BTreeMap::new();
        let mapped = map_effective_params(&params, &attribution).expect("mapped");
        assert!(
            mapped.secret_attributions.is_empty(),
            "no attribution -> no resolvable secret"
        );
    }

    #[test]
    fn preserves_jump_chain_field_and_attributes_hop_secret() {
        let key_uuid = "33333333-3333-3333-3333-333333333333";
        let params = json!({
            "host": "10.0.0.1",
            "jump": [ {
                "host": "10.0.0.9",
                "port": 22,
                "credentials": { "username": "jump", "private_key": key_uuid }
            } ]
        });
        let mut attribution: BTreeMap<String, ConnConfSource> = BTreeMap::new();
        attribution.insert("jump.0.credentials.private_key".to_string(), source(9));
        let mapped = map_effective_params(&params, &attribution).expect("mapped");
        assert!(
            mapped.fields.get("jump").is_some(),
            "jump must be preserved"
        );
        let attrs = mapped
            .secret_attributions
            .get(&Uuid::parse_str(key_uuid).unwrap())
            .expect("hop key attributed");
        assert_eq!(attrs[0].field_key, "jump.0.credentials.private_key");
        assert_eq!(attrs[0].record_id, Uuid::from_u128(9));
    }

    #[test]
    fn non_object_params_yield_none() {
        let empty: BTreeMap<String, ConnConfSource> = BTreeMap::new();
        assert!(map_effective_params(&json!(null), &empty).is_none());
        assert!(map_effective_params(&json!("x"), &empty).is_none());
    }
}
