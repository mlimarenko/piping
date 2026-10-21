//! Production [`DeviceResolver`]: turn a connection-target id into a concrete,
//! reachable device (family, endpoint, credentials).
//!
//! # Preferred path — the typed `cctv` connection profile
//!
//! When the target has a resolvable `cctv` launch account (the SAME typed
//! connection-config profile the connect flow resolves per protocol, via
//! [`resolve_launch_accounts`]), the resolver drives everything off it:
//! `params.host` / `params.mgmt_port` / `params.family` /
//! `params.credentials.{username,password}`. Inheritance is the profile's native
//! scope + template merge (most-specific wins, first account is the winner), and
//! `params.family` overrides the platform-derived family. The `password` is
//! EITHER a bare secret-UUID (dereferenced ONLY through the same secret fence the
//! connect path uses: allowed if the UUID is bound to the target via
//! `connection_scope:connection_target`, OR bound to the record that produced the
//! `credentials.password` leaf via `connection_config:record` at field_key
//! `credentials.password`; a UUID that is not fence-bound is never dereferenced)
//! OR a literal value used verbatim — exactly as the connect path treats a
//! non-UUID `credentials.password` for ssh/rdp/vnc, so the UI's plain password
//! field works without a separate secret-vaulting step.
//!
//! # Fallback path (backward compatible)
//!
//! With no cctv launch account (or one without fence-verified credentials) the
//! resolver keeps the prior behavior: the address comes from the connection-target
//! read model, the family from the operator-set `napalm_driver` slug on the
//! target's platform (a typed, structural signal — never a keyword match), the
//! port from the family default, and credentials from the target's effective
//! config-context tree behind the `connection_scope:connection_target` fence. See
//! `ConfiguredDeviceResolver::resolve_device_credentials`.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

use crate::contexts::connection_config::{
    ConnConfAssignmentRepository, ConnConfProtocol, ConnConfRecordRepository, ConnConfSource,
    LaunchAccount, ResolveInput, SqlxConnConfAssignmentRepository, resolve_launch_accounts,
};
use crate::contexts::device_management::application::resolver::{
    DeviceResolveError, DeviceResolver, ResolvedDevice,
};
use crate::contexts::device_management::domain::{DeviceFamily, Secret};
use crate::contexts::device_management::infrastructure::transport::DeviceEndpoint;
use crate::contexts::inventory::application::resolve_config_contexts_ordered;
use crate::contexts::inventory::infrastructure::{ConfigContextRepository, SqlxConfigContextRepository};
use crate::contexts::inventory::infrastructure::persistence::repositories::native::connection_target_repository::{
    ConnectionTargetFilters, ConnectionTargetRepository,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::platform_repository::{
    PlatformRepository, SqlxPlatformRepository,
};
use crate::contexts::secrets::application::{
    CONNECTION_CONFIG_RECORD_CONSUMER_TYPE, CONNECTION_TARGET_SECRET_CONSUMER_TYPE, SecretStore,
};
use crate::contexts::secrets::infrastructure::{SecretRepository, SqlxSecretRepository};
use crate::errors::AppError;

/// The dotted leaf path of the cctv profile's password field. Both the launch
/// account's per-leaf attribution and the `connection_config:record`
/// usage-binding key the password by this exact path.
const VIDEO_PASSWORD_FIELD_KEY: &str = "credentials.password";

/// Dotted leaf path of the optional privilege-escalation password a Linux
/// launch profile may carry; fenced exactly like the password.
const BECOME_PASSWORD_FIELD_KEY: &str = "credentials.become_password";

/// Launch-account protocol for a device family: video devices keep their
/// `cctv` profile, a Linux host uses its `ssh` profile. Without a known
/// platform family the video profile stays the default.
fn launch_protocol(family: Option<DeviceFamily>) -> ConnConfProtocol {
    match family {
        Some(family) if !family.is_video() => ConnConfProtocol::Ssh,
        _ => ConnConfProtocol::Cctv,
    }
}

/// Object-safe read of a platform's operator-set driver slug.
///
/// `PlatformRepository` uses async-fn-in-trait and is therefore not
/// dyn-compatible, so the resolver depends on this narrow adapter instead. The
/// returned slug is a typed, operator-configured field, kept structural.
#[async_trait]
pub trait PlatformDriverLookup: Send + Sync {
    /// The platform's `napalm_driver` slug, or `None` when unset or absent.
    ///
    /// # Errors
    ///
    /// Fails only on an underlying storage error.
    async fn napalm_driver(&self, platform_id: Uuid) -> Result<Option<String>, AppError>;
}

#[async_trait]
impl PlatformDriverLookup for SqlxPlatformRepository {
    async fn napalm_driver(&self, platform_id: Uuid) -> Result<Option<String>, AppError> {
        Ok(self
            .get_by_id(platform_id)
            .await?
            .and_then(|platform| platform.napalm_driver))
    }
}

/// Object-safe read of a connection target's platform id via its inventory
/// scope ancestry.
///
/// Wraps [`ConnConfAssignmentRepository::get_device_scopes`] so the resolver can
/// hold a narrow, easily-faked dependency instead of the full assignment CRUD
/// trait.
#[async_trait]
pub trait DevicePlatformScopeLookup: Send + Sync {
    /// The platform id bound to `target_id`, or `None` when the target has no
    /// platform (or does not exist).
    ///
    /// # Errors
    ///
    /// Fails only on an underlying storage error.
    async fn platform_id(&self, target_id: Uuid) -> Result<Option<Uuid>, AppError>;
}

#[async_trait]
impl DevicePlatformScopeLookup for SqlxConnConfAssignmentRepository {
    async fn platform_id(&self, target_id: Uuid) -> Result<Option<Uuid>, AppError> {
        Ok(self
            .get_device_scopes(target_id)
            .await?
            .and_then(|scopes| scopes.platform_id))
    }
}

/// Object-safe read of a connection target's effective connection-config.
///
/// Wraps [`ConfigContextRepository::list_for_scope_hierarchy`] plus
/// [`resolve_config_contexts_ordered`] so the resolver holds a narrow, faked
/// dependency instead of the full config-context CRUD trait (which uses
/// async-fn-in-trait and is not dyn-compatible). Secret leaves are left as bare
/// UUID strings — this method never resolves a secret value.
#[async_trait]
pub trait DeviceConnectionConfigLookup: Send + Sync {
    /// The target's merged config-context tree (the same structure the connect
    /// path materializes), or an empty object when the target has no bound
    /// contexts.
    ///
    /// # Errors
    ///
    /// Fails only on an underlying storage error.
    async fn effective_config(&self, target_id: Uuid) -> Result<Value, AppError>;
}

#[async_trait]
impl DeviceConnectionConfigLookup for SqlxConfigContextRepository {
    async fn effective_config(&self, target_id: Uuid) -> Result<Value, AppError> {
        let contexts = self
            .list_for_scope_hierarchy("connection_target", target_id)
            .await?;
        Ok(resolve_config_contexts_ordered(&contexts))
    }
}

/// Object-safe read of a connection target's own (target-scope) secret ids.
///
/// Wraps [`SecretRepository::list_usage_bindings_for_consumer`] for the
/// `connection_scope:connection_target` consumer, projecting the bindings to the
/// set of secret ids the target owns. This set is the secret fence: a candidate
/// password is resolvable ONLY if it is a member, which is exactly how the
/// connect-path proxy fence gates target-scope secrets.
#[async_trait]
pub trait TargetSecretBindings: Send + Sync {
    /// The secret ids bound to `target_id` under the `connection_scope:
    /// connection_target` consumer.
    ///
    /// # Errors
    ///
    /// Fails only on an underlying storage error.
    async fn target_scope_secret_ids(&self, target_id: Uuid) -> Result<HashSet<Uuid>, AppError>;
}

#[async_trait]
impl TargetSecretBindings for SqlxSecretRepository {
    async fn target_scope_secret_ids(&self, target_id: Uuid) -> Result<HashSet<Uuid>, AppError> {
        let bindings = self
            .list_usage_bindings_for_consumer(CONNECTION_TARGET_SECRET_CONSUMER_TYPE, target_id)
            .await?;
        Ok(bindings
            .into_iter()
            .map(|binding| binding.secret_id)
            .collect())
    }
}

/// Object-safe resolution of a target's ordered `cctv` launch accounts.
///
/// Mirrors the connect path's shared-account resolution
/// (`ConnectionService::resolve_shared_launch_accounts`) for
/// [`ConnConfProtocol::Cctv`]: it does the DB round-trips the pure resolver
/// needs (`get_device_scopes` → `list_for_scopes` + `list_all`) and runs
/// [`resolve_launch_accounts`], so the resolver holds a narrow, easily-faked
/// dependency instead of the two full CRUD traits.
#[async_trait]
pub trait DeviceLaunchAccounts: Send + Sync {
    /// The ordered cctv launch accounts for `target_id`, most-specific first
    /// (index 0 is the winning account). Empty when the target has no matching
    /// `cctv` connection-config records.
    ///
    /// # Errors
    ///
    /// Fails only on an underlying storage error (or a resolver validation
    /// error, surfaced as [`AppError::Validation`]).
    async fn resolve(
        &self,
        target_id: Uuid,
        protocol: ConnConfProtocol,
    ) -> Result<Vec<LaunchAccount>, AppError>;
}

/// Production [`DeviceLaunchAccounts`] over the connection-config repositories.
pub struct ConnConfDeviceLaunchAccounts {
    records: Arc<dyn ConnConfRecordRepository>,
    assignments: Arc<dyn ConnConfAssignmentRepository>,
}

impl ConnConfDeviceLaunchAccounts {
    /// Construct from the connection-config record + assignment repositories.
    #[must_use]
    pub fn new(
        records: Arc<dyn ConnConfRecordRepository>,
        assignments: Arc<dyn ConnConfAssignmentRepository>,
    ) -> Self {
        Self {
            records,
            assignments,
        }
    }
}

#[async_trait]
impl DeviceLaunchAccounts for ConnConfDeviceLaunchAccounts {
    async fn resolve(
        &self,
        target_id: Uuid,
        protocol: ConnConfProtocol,
    ) -> Result<Vec<LaunchAccount>, AppError> {
        // Same round-trips as the connect path: resolve the device's scope
        // ancestry, gather the matching enabled assignments, and load the record
        // pool the pure resolver needs.
        let Some(scope_ids) = self.assignments.get_device_scopes(target_id).await? else {
            return Ok(Vec::new());
        };
        let device_scopes = scope_ids.to_scope_pairs();
        let assignments = self.assignments.list_for_scopes(&device_scopes).await?;
        let records = self.records.list_all().await?;

        let input = ResolveInput {
            protocol,
            device_scopes: &device_scopes,
            assignments: &assignments,
            records: &records,
        };
        resolve_launch_accounts(&input).map_err(|error| AppError::Validation(error.to_string()))
    }
}

/// Object-safe read of a connection-config record's own secret bindings.
///
/// Wraps [`SecretRepository::list_usage_bindings_for_consumer`] for the
/// `connection_config:record` consumer, projecting the bindings to the
/// `(field_key, secret_id)` pairs the record owns. This is the SECOND half of
/// the cctv-password fence (the target-scope half is [`TargetSecretBindings`]):
/// the record that produced the `credentials.password` leaf must itself bind the
/// candidate secret at that field_key, exactly as the connect-path proxy fence
/// requires.
#[async_trait]
pub trait RecordSecretBindings: Send + Sync {
    /// The `(field_key, secret_id)` bindings for connection-config `record_id`
    /// under the `connection_config:record` consumer.
    ///
    /// # Errors
    ///
    /// Fails only on an underlying storage error.
    async fn record_scope_bindings(&self, record_id: Uuid)
    -> Result<Vec<(String, Uuid)>, AppError>;
}

#[async_trait]
impl RecordSecretBindings for SqlxSecretRepository {
    async fn record_scope_bindings(
        &self,
        record_id: Uuid,
    ) -> Result<Vec<(String, Uuid)>, AppError> {
        let bindings = self
            .list_usage_bindings_for_consumer(CONNECTION_CONFIG_RECORD_CONSUMER_TYPE, record_id)
            .await?;
        Ok(bindings
            .into_iter()
            .map(|binding| (binding.field_key, binding.secret_id))
            .collect())
    }
}

/// Resolves a connection target to a reachable device.
///
/// PREFERS the typed `cctv` connection profile (via [`DeviceLaunchAccounts`] +
/// the record/target secret fence), and FALLS BACK to the inventory read-model,
/// the platform driver registry, and the target-scope config-context fence when
/// no fence-verified cctv credential is available.
pub struct ConfiguredDeviceResolver {
    connection_targets: Arc<dyn ConnectionTargetRepository>,
    scopes: Arc<dyn DevicePlatformScopeLookup>,
    platforms: Arc<dyn PlatformDriverLookup>,
    config: Arc<dyn DeviceConnectionConfigLookup>,
    secret_bindings: Arc<dyn TargetSecretBindings>,
    launch_accounts: Arc<dyn DeviceLaunchAccounts>,
    record_bindings: Arc<dyn RecordSecretBindings>,
    secret_store: Arc<dyn SecretStore>,
}

/// The narrow adapters [`ConfiguredDeviceResolver`] is assembled from.
///
/// Each field is a distinct adapter trait that tests substitute individually;
/// grouping them into one struct keeps the constructor signature small without
/// bundling the traits themselves.
pub struct ResolverDependencies {
    /// Connection-target read model (existence + management address).
    pub connection_targets: Arc<dyn ConnectionTargetRepository>,
    /// Platform-scope lookup feeding the family derivation.
    pub scopes: Arc<dyn DevicePlatformScopeLookup>,
    /// Platform driver-slug lookup.
    pub platforms: Arc<dyn PlatformDriverLookup>,
    /// Effective connection-config lookup (fallback credential path).
    pub config: Arc<dyn DeviceConnectionConfigLookup>,
    /// Target-scope secret-id fence.
    pub secret_bindings: Arc<dyn TargetSecretBindings>,
    /// Typed cctv launch-account resolver.
    pub launch_accounts: Arc<dyn DeviceLaunchAccounts>,
    /// Record-scope secret-id fence.
    pub record_bindings: Arc<dyn RecordSecretBindings>,
    /// Secret store behind both fences.
    pub secret_store: Arc<dyn SecretStore>,
}

impl ConfiguredDeviceResolver {
    /// Construct the resolver from the read-model repositories, the typed cctv
    /// launch-account resolver, the config-context lookup, and the target- and
    /// record-scope secret fences it needs.
    #[must_use]
    pub fn new(dependencies: ResolverDependencies) -> Self {
        Self {
            connection_targets: dependencies.connection_targets,
            scopes: dependencies.scopes,
            platforms: dependencies.platforms,
            config: dependencies.config,
            secret_bindings: dependencies.secret_bindings,
            launch_accounts: dependencies.launch_accounts,
            record_bindings: dependencies.record_bindings,
            secret_store: dependencies.secret_store,
        }
    }

    /// Read the target's management address from the connection-target read
    /// model.
    async fn resolve_address(&self, target_id: Uuid) -> Result<String, DeviceResolveError> {
        let filters = ConnectionTargetFilters {
            id: Some(target_id),
            ..ConnectionTargetFilters::default()
        };
        let (targets, _total) = self
            .connection_targets
            .list(1, 0, filters)
            .await
            .map_err(|error| DeviceResolveError::Internal(error.to_string()))?;
        let target = targets
            .into_iter()
            .next()
            .ok_or(DeviceResolveError::NotFound(target_id))?;
        let address = target
            .address
            .map(|address| address.trim().to_owned())
            .filter(|address| !address.is_empty());
        address.ok_or(DeviceResolveError::NoAddress(target_id))
    }

    /// Derive the device family from the target's platform `napalm_driver`.
    async fn resolve_family(&self, target_id: Uuid) -> Result<DeviceFamily, DeviceResolveError> {
        let platform_id = self
            .scopes
            .platform_id(target_id)
            .await
            .map_err(|error| DeviceResolveError::Internal(error.to_string()))?
            .ok_or(DeviceResolveError::UnknownFamily(target_id))?;
        let driver = self
            .platforms
            .napalm_driver(platform_id)
            .await
            .map_err(|error| DeviceResolveError::Internal(error.to_string()))?
            .ok_or(DeviceResolveError::UnknownFamily(target_id))?;
        DeviceFamily::parse(&driver).ok_or(DeviceResolveError::UnknownFamily(target_id))
    }

    /// Resolve device-admin credentials for `target_id` at TARGET scope.
    ///
    /// Reuses the same secret fence as the connect path: the target's own
    /// secrets (bound under `connection_scope:connection_target`) are the gate.
    /// The username + candidate password-UUID come from the target's effective
    /// connection-config; the password is dereferenced ONLY after the UUID is
    /// verified to be a target-owned secret, so a UUID appearing in config is
    /// never trusted on its own.
    ///
    /// # Errors
    ///
    /// [`DeviceResolveError::NoCredentials`] when the target owns no secrets, its
    /// config carries no fence-verified `username`+`password` credential, or the
    /// bound secret value is missing; [`DeviceResolveError::Internal`] on a
    /// storage failure.
    async fn resolve_device_credentials(
        &self,
        target_id: Uuid,
    ) -> Result<(String, Secret), DeviceResolveError> {
        // The target's own secrets. A candidate password is dereferenced ONLY if
        // it is in this set — never because it is UUID-shaped or merely present
        // in the config tree.
        let allowed = self
            .secret_bindings
            .target_scope_secret_ids(target_id)
            .await
            .map_err(|error| DeviceResolveError::Internal(error.to_string()))?;
        if allowed.is_empty() {
            return Err(DeviceResolveError::NoCredentials(target_id));
        }

        let config = self
            .config
            .effective_config(target_id)
            .await
            .map_err(|error| DeviceResolveError::Internal(error.to_string()))?;

        let Some((username, secret_id)) = select_target_credential(&config, &allowed) else {
            return Err(DeviceResolveError::NoCredentials(target_id));
        };

        match self.secret_store.get_secret(secret_id).await {
            Ok(password) => Ok((username, Secret::new(password))),
            // A bound-but-empty secret is a configured-yet-unusable credential,
            // reported as an actionable gap rather than an internal failure.
            Err(AppError::MissingSecret(_)) => Err(DeviceResolveError::NoCredentials(target_id)),
            Err(error) => Err(DeviceResolveError::Internal(error.to_string())),
        }
    }

    /// Resolve `target_id` from its typed `cctv` connection profile.
    ///
    /// Returns `Some(device)` when the winning (most-specific) launch account
    /// carries a literal `credentials.username` and, for a video family, a
    /// `credentials.password` that passes the fence and dereferences to a value.
    /// A Linux host needs no password (its driver authenticates through the SSH
    /// proxy's own launch material), so a password is optional there. When a
    /// video family has no usable password the method returns `Ok(None)` so the
    /// caller falls back to the platform + config-context path; a Linux account
    /// without a username is a credentials gap and returns
    /// [`DeviceResolveError::NoCredentials`]. `params.host` / `params.mgmt_port` /
    /// `params.family` override the read-model address / family default /
    /// platform-derived family respectively.
    async fn resolve_from_launch_profile(
        &self,
        target_id: Uuid,
        platform_family: Option<DeviceFamily>,
    ) -> Result<Option<ResolvedDevice>, DeviceResolveError> {
        let accounts = self
            .launch_accounts
            .resolve(target_id, launch_protocol(platform_family))
            .await
            .map_err(|error| DeviceResolveError::Internal(error.to_string()))?;
        // The winning account (index 0 = most-specific, native profile inheritance).
        let Some(account) = accounts.into_iter().next() else {
            return Ok(None);
        };

        // Family: `params.family` (typed enum parse) else the platform driver.
        // Resolved FIRST because whether a password is required depends on it.
        let family = match account
            .params
            .get("family")
            .and_then(Value::as_str)
            .and_then(DeviceFamily::parse)
            .or(platform_family)
        {
            Some(family) => family,
            None => self.resolve_family(target_id).await?,
        };

        // Username and password come from the typed profile. The password is
        // EITHER a bare secret-UUID reference (dereferenced only through the same
        // fence the connect path uses) OR a literal value used verbatim, exactly
        // as the connect path treats a non-UUID `credentials.password` for
        // ssh/rdp/vnc — see `connection_service::proxy_payload::parse_secret_link_id`.
        // This keeps the vaulted path more secure while letting the UI's plain
        // password field work without a separate secret-vaulting step.
        let credentials = account.params.get("credentials");
        let username = credentials
            .and_then(|creds| creds.get("username"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|username| !username.is_empty());
        let Some(username) = username else {
            // A video target may still have usable credentials in its
            // config-context tree, so fall back. A Linux host has no
            // password-based fallback that could authenticate it, so the missing
            // login is reported as the actionable credentials gap.
            if family.is_video() {
                return Ok(None);
            }
            return Err(DeviceResolveError::NoCredentials(target_id));
        };

        // A password is required for a video device but optional for a Linux host
        // (its driver runs through the SSH proxy's launch material and uses only
        // the login name locally). The fence is unchanged for a present value.
        let password = self
            .fenced_secret(
                target_id,
                credentials
                    .and_then(|creds| creds.get("password"))
                    .and_then(Value::as_str),
                VIDEO_PASSWORD_FIELD_KEY,
                &account.attribution,
            )
            .await?;
        if family.is_video() && password.is_none() {
            // A video device cannot authenticate without a password: fall back to
            // the config-context credential path rather than build a driver that
            // cannot connect.
            return Ok(None);
        }

        // Endpoint host: a literal `params.host` else the read-model address.
        // Connection-config records routinely carry the reserved endpoint
        // placeholder (`{{primary_ip4}}`, per-device VNC/CCTV seeds do) — any
        // `{{...}}` template means "use the target's own address", which is
        // exactly what the read-model fallback returns.
        let host = match account
            .params
            .get("host")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|host| !host.is_empty() && !host.contains("{{"))
        {
            Some(host) => host.to_owned(),
            None => self.resolve_address(target_id).await?,
        };

        let become_password = self
            .fenced_secret(
                target_id,
                credentials
                    .and_then(|creds| creds.get("become_password"))
                    .and_then(Value::as_str),
                BECOME_PASSWORD_FIELD_KEY,
                &account.attribution,
            )
            .await?;

        // Port: `params.mgmt_port` else the family default.
        let port = account
            .params
            .get("mgmt_port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .unwrap_or_else(|| family.default_management_port());

        Ok(Some(ResolvedDevice {
            family,
            endpoint: DeviceEndpoint::new(host, port),
            username: username.to_owned(),
            password,
            become_password,
        }))
    }

    /// Dereference an optional secret leaf of the launch profile through the
    /// same fence as `credentials.password`: a UUID is read only when bound to
    /// the target or to the record that produced the leaf; a literal is used
    /// verbatim; an absent, unfenced or empty value yields `None`.
    async fn fenced_secret(
        &self,
        target_id: Uuid,
        raw: Option<&str>,
        field_key: &str,
        attribution: &BTreeMap<String, ConnConfSource>,
    ) -> Result<Option<Secret>, DeviceResolveError> {
        let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let Ok(secret_id) = Uuid::parse_str(raw) else {
            return Ok(Some(Secret::new(raw)));
        };
        if !self
            .secret_allowed(target_id, secret_id, field_key, attribution)
            .await?
        {
            return Ok(None);
        }
        match self.secret_store.get_secret(secret_id).await {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(AppError::MissingSecret(_)) => Ok(None),
            Err(error) => Err(DeviceResolveError::Internal(error.to_string())),
        }
    }

    /// Whether the cctv profile's `credentials.password` secret UUID may be
    /// dereferenced, mirroring the connect-path proxy fence EXACTLY.
    ///
    /// Allowed iff the UUID is bound to the target under
    /// `connection_scope:connection_target`, OR the record the resolver
    /// attributed the `credentials.password` leaf to has a
    /// `connection_config:record` binding for field_key `credentials.password`
    /// whose secret id equals the candidate. Never a blanket trust of a
    /// UUID-shaped value.
    async fn secret_allowed(
        &self,
        target_id: Uuid,
        secret_id: Uuid,
        field_key: &str,
        attribution: &BTreeMap<String, ConnConfSource>,
    ) -> Result<bool, DeviceResolveError> {
        // (a) Target-scope fence.
        let target_owned = self
            .secret_bindings
            .target_scope_secret_ids(target_id)
            .await
            .map_err(|error| DeviceResolveError::Internal(error.to_string()))?;
        if target_owned.contains(&secret_id) {
            return Ok(true);
        }

        // (b) Record-scope fence: only the record that ACTUALLY produced the
        // password leaf can vouch for the UUID, and only via a matching binding.
        let Some(source) = attribution.get(field_key) else {
            return Ok(false);
        };
        let bindings = self
            .record_bindings
            .record_scope_bindings(source.record_id)
            .await
            .map_err(|error| DeviceResolveError::Internal(error.to_string()))?;
        Ok(bindings.iter().any(|(bound_key, bound_secret_id)| {
            bound_key == field_key && *bound_secret_id == secret_id
        }))
    }
}

/// Pick a fence-verified admin credential from a target's effective config.
///
/// Walks `piping.connections.<proto>[*].credentials` for a block carrying a
/// non-empty literal `username` and a `password` that parses to a UUID present
/// in `allowed` (the target's own secrets). Protocol keys are visited in stable
/// (sorted) order so a target that configures credentials under several protocol
/// slots resolves the SAME one deterministically. Selection is driven by the
/// secret fence (`password ∈ allowed`), never by the protocol name — a
/// structural, domain-neutral signal.
///
/// Returns `(username, password_secret_id)` for the first qualifying block.
fn select_target_credential(config: &Value, allowed: &HashSet<Uuid>) -> Option<(String, Uuid)> {
    let connections = config
        .get("piping")
        .and_then(|piping| piping.get("connections"))
        .and_then(Value::as_object)?;

    let mut protocols: Vec<&String> = connections.keys().collect();
    protocols.sort_unstable();

    for protocol in protocols {
        let Some(value) = connections.get(protocol) else {
            continue;
        };
        for entry in entry_objects(value) {
            if let Some(found) = credential_from_entry(entry, allowed) {
                return Some(found);
            }
        }
    }
    None
}

/// Normalize a protocol value into its entry objects. A protocol slot is either
/// an array of entries or a single entry object (both shapes are accepted by the
/// connection-config schema); anything else yields no entries.
fn entry_objects(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![value],
        _ => Vec::new(),
    }
}

/// Extract a fence-verified `(username, password_secret_id)` from one entry's
/// `credentials` block, or `None` when it lacks a literal username or a
/// target-owned password reference.
fn credential_from_entry(entry: &Value, allowed: &HashSet<Uuid>) -> Option<(String, Uuid)> {
    let credentials = entry.get("credentials").and_then(Value::as_object)?;
    let username = credentials
        .get("username")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|username| !username.is_empty())?;
    let secret_id = credentials
        .get("password")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw.trim()).ok())?;
    if !allowed.contains(&secret_id) {
        return None;
    }
    Some((username.to_owned(), secret_id))
}

#[async_trait]
impl DeviceResolver for ConfiguredDeviceResolver {
    async fn resolve(&self, target_id: Uuid) -> Result<ResolvedDevice, DeviceResolveError> {
        // Prefer the typed cctv connection profile with its native inheritance
        // and secret fence.
        let platform_family = self.resolve_family(target_id).await.ok();
        if let Some(device) = self
            .resolve_from_launch_profile(target_id, platform_family)
            .await?
        {
            return Ok(device);
        }

        // Backward-compat fallback: read-model address, platform-derived family,
        // and the any-protocol config-context credential walk behind the
        // target-scope fence.
        let host = self.resolve_address(target_id).await?;
        let family = match platform_family {
            Some(family) => family,
            None => self.resolve_family(target_id).await?,
        };
        let endpoint = DeviceEndpoint::new(host, family.default_management_port());
        let (username, password) = self.resolve_device_credentials(target_id).await?;
        Ok(ResolvedDevice {
            family,
            endpoint,
            username,
            password: Some(password),
            become_password: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use serde_json::json;

    use crate::contexts::inventory::infrastructure::persistence::repositories::native::connection_target_repository::{
        ConnectionTarget, ConnectionTargetKind,
    };

    /// Connection-target repository double returning a fixed row set.
    struct FakeTargets {
        rows: Vec<ConnectionTarget>,
    }

    #[async_trait]
    impl ConnectionTargetRepository for FakeTargets {
        async fn list(
            &self,
            _limit: i64,
            _offset: i64,
            filters: ConnectionTargetFilters,
        ) -> Result<(Vec<ConnectionTarget>, i64), AppError> {
            let rows: Vec<ConnectionTarget> = self
                .rows
                .iter()
                .filter(|row| filters.id.is_none_or(|id| row.id == id))
                .cloned()
                .collect();
            let total = i64::try_from(rows.len()).unwrap_or(i64::MAX);
            Ok((rows, total))
        }
    }

    /// Platform-scope lookup double returning a fixed platform id.
    struct FakeScopes {
        platform_id: Option<Uuid>,
    }

    #[async_trait]
    impl DevicePlatformScopeLookup for FakeScopes {
        async fn platform_id(&self, _target_id: Uuid) -> Result<Option<Uuid>, AppError> {
            Ok(self.platform_id)
        }
    }

    /// Driver-slug lookup double returning a fixed slug.
    struct FakeDriver {
        driver: Option<String>,
    }

    #[async_trait]
    impl PlatformDriverLookup for FakeDriver {
        async fn napalm_driver(&self, _platform_id: Uuid) -> Result<Option<String>, AppError> {
            Ok(self.driver.clone())
        }
    }

    /// Effective-config lookup double returning a fixed merged config tree.
    struct FakeConfig {
        config: Value,
    }

    #[async_trait]
    impl DeviceConnectionConfigLookup for FakeConfig {
        async fn effective_config(&self, _target_id: Uuid) -> Result<Value, AppError> {
            Ok(self.config.clone())
        }
    }

    /// Target-scope binding double returning a fixed owned-secret set.
    struct FakeBindings {
        ids: HashSet<Uuid>,
    }

    #[async_trait]
    impl TargetSecretBindings for FakeBindings {
        async fn target_scope_secret_ids(
            &self,
            _target_id: Uuid,
        ) -> Result<HashSet<Uuid>, AppError> {
            Ok(self.ids.clone())
        }
    }

    /// Cctv launch-account double returning a fixed ordered account list. The
    /// default is empty, so the backward-compat tests fall through to the
    /// platform + config-context path unchanged.
    struct FakeCctvAccounts {
        accounts: Vec<LaunchAccount>,
    }

    #[async_trait]
    impl DeviceLaunchAccounts for FakeCctvAccounts {
        async fn resolve(
            &self,
            _target_id: Uuid,
            _protocol: ConnConfProtocol,
        ) -> Result<Vec<LaunchAccount>, AppError> {
            Ok(self.accounts.clone())
        }
    }

    /// Record-scope binding double returning fixed `(field_key, secret_id)` pairs
    /// per connection-config record id.
    struct FakeRecordBindings {
        by_record: HashMap<Uuid, Vec<(String, Uuid)>>,
    }

    #[async_trait]
    impl RecordSecretBindings for FakeRecordBindings {
        async fn record_scope_bindings(
            &self,
            record_id: Uuid,
        ) -> Result<Vec<(String, Uuid)>, AppError> {
            Ok(self.by_record.get(&record_id).cloned().unwrap_or_default())
        }
    }

    /// Secret store double that records every read and enforces the fence: it is
    /// a test failure to read a secret id the target does not own, so a resolver
    /// that dereferenced an unverified UUID would fail loudly here.
    struct RecordingStore {
        values: HashMap<Uuid, String>,
        owned: HashSet<Uuid>,
        reads: Mutex<Vec<Uuid>>,
    }

    impl RecordingStore {
        fn new(
            values: impl IntoIterator<Item = (Uuid, String)>,
            owned: impl IntoIterator<Item = Uuid>,
        ) -> Self {
            Self {
                values: values.into_iter().collect(),
                owned: owned.into_iter().collect(),
                reads: Mutex::new(Vec::new()),
            }
        }

        fn empty() -> Self {
            Self::new(Vec::new(), Vec::new())
        }

        fn reads(&self) -> Vec<Uuid> {
            self.reads.lock().expect("reads lock").clone()
        }
    }

    #[async_trait]
    impl SecretStore for RecordingStore {
        async fn get_secret(&self, secret_id: Uuid) -> Result<String, AppError> {
            self.reads.lock().expect("reads lock").push(secret_id);
            assert!(
                self.owned.contains(&secret_id),
                "fence violated: get_secret dereferenced a non-target-owned secret"
            );
            self.values
                .get(&secret_id)
                .cloned()
                .ok_or_else(|| AppError::MissingSecret(secret_id.to_string()))
        }

        async fn set_secret(&self, _secret_id: Uuid, _value: &str) -> Result<(), AppError> {
            Err(AppError::Internal("set_secret unused in tests".to_owned()))
        }

        async fn delete_secret(&self, _secret_id: Uuid) -> Result<(), AppError> {
            Err(AppError::Internal(
                "delete_secret unused in tests".to_owned(),
            ))
        }

        async fn ensure_unique_key(
            &self,
            _scope_type: &str,
            _scope_id: Uuid,
            _key: &str,
        ) -> Result<(), AppError> {
            Err(AppError::Internal(
                "ensure_unique_key unused in tests".to_owned(),
            ))
        }
    }

    fn target(id: Uuid, address: Option<&str>) -> ConnectionTarget {
        ConnectionTarget {
            id,
            name: "test-target".to_owned(),
            address: address.map(str::to_owned),
            address_bound: address.is_some(),
            target_kind: ConnectionTargetKind::Device,
            status: None,
            device_id: Some(id),
            device_name: None,
            vm_id: None,
            vm_name: None,
            tenant_id: None,
            tenant_name: None,
            site_id: None,
            site_name: None,
            region_id: None,
            region_name: None,
            role_id: None,
            role_name: None,
            role_custom_fields: None,
            platform_id: None,
            platform_name: None,
            platform_driver: None,
            family: None,
            is_cctv: false,
            device_type_id: None,
            device_type_name: None,
            cluster_id: None,
            cluster_name: None,
        }
    }

    /// Build a resolver over the platform + config-context path with NO cctv
    /// launch account (the fallback path). Cctv accounts and record bindings are
    /// empty, so resolution flows through the backward-compat legacy path.
    fn resolver_with(
        rows: Vec<ConnectionTarget>,
        platform_id: Option<Uuid>,
        driver: Option<&str>,
        config: Value,
        owned: HashSet<Uuid>,
        store: Arc<RecordingStore>,
    ) -> ConfiguredDeviceResolver {
        ConfiguredDeviceResolver::new(ResolverDependencies {
            connection_targets: Arc::new(FakeTargets { rows }),
            scopes: Arc::new(FakeScopes { platform_id }),
            platforms: Arc::new(FakeDriver {
                driver: driver.map(str::to_owned),
            }),
            config: Arc::new(FakeConfig { config }),
            secret_bindings: Arc::new(FakeBindings { ids: owned }),
            launch_accounts: Arc::new(FakeCctvAccounts {
                accounts: Vec::new(),
            }),
            record_bindings: Arc::new(FakeRecordBindings {
                by_record: HashMap::new(),
            }),
            secret_store: store,
        })
    }

    /// The cctv launch accounts plus the two secret fences that vouch for the
    /// password they carry: target-owned ids and per-record bindings.
    struct CctvFences {
        accounts: Vec<LaunchAccount>,
        owned: HashSet<Uuid>,
        record_bindings: HashMap<Uuid, Vec<(String, Uuid)>>,
    }

    /// Build a resolver driven by a typed cctv launch-account list plus the two
    /// fences (target-owned secret ids and per-record `connection_config:record`
    /// bindings). The config-context tree is empty: the cctv path never reads it.
    fn video_resolver(
        rows: Vec<ConnectionTarget>,
        platform_id: Option<Uuid>,
        driver: Option<&str>,
        cctv: CctvFences,
        store: Arc<RecordingStore>,
    ) -> ConfiguredDeviceResolver {
        ConfiguredDeviceResolver::new(ResolverDependencies {
            connection_targets: Arc::new(FakeTargets { rows }),
            scopes: Arc::new(FakeScopes { platform_id }),
            platforms: Arc::new(FakeDriver {
                driver: driver.map(str::to_owned),
            }),
            config: Arc::new(FakeConfig {
                config: Value::Object(serde_json::Map::new()),
            }),
            secret_bindings: Arc::new(FakeBindings { ids: cctv.owned }),
            launch_accounts: Arc::new(FakeCctvAccounts {
                accounts: cctv.accounts,
            }),
            record_bindings: Arc::new(FakeRecordBindings {
                by_record: cctv.record_bindings,
            }),
            secret_store: store,
        })
    }

    /// Build one cctv [`LaunchAccount`] with the given params. When
    /// `password_source_record` is `Some`, the `credentials.password` leaf is
    /// attributed to that record id (as the launch resolver would), so the
    /// record-scope fence can be exercised.
    fn video_account(
        record_id: Uuid,
        params: Value,
        password_source_record: Option<Uuid>,
    ) -> LaunchAccount {
        let mut attribution: BTreeMap<String, ConnConfSource> = BTreeMap::new();
        if let Some(source_record) = password_source_record {
            attribution.insert(
                VIDEO_PASSWORD_FIELD_KEY.to_owned(),
                ConnConfSource {
                    record_id: source_record,
                    record_name: "cctv-record".to_owned(),
                    scope_type: None,
                    scope_id: None,
                },
            );
        }
        LaunchAccount {
            record_id,
            record_name: "cctv".to_owned(),
            scope_type: None,
            scope_id: None,
            params,
            attribution,
        }
    }

    /// Build a resolver whose credential dependencies are empty. The address /
    /// family tests fail before the credential step, so the store is never read.
    fn resolver(
        rows: Vec<ConnectionTarget>,
        platform_id: Option<Uuid>,
        driver: Option<&str>,
    ) -> ConfiguredDeviceResolver {
        resolver_with(
            rows,
            platform_id,
            driver,
            Value::Object(serde_json::Map::new()),
            HashSet::new(),
            Arc::new(RecordingStore::empty()),
        )
    }

    /// A resolvable device: valid address, a recognized driver slug, and a
    /// config whose credentials reference a target-owned secret.
    fn config_with_ssh_credentials(username: &str, password_secret: Uuid) -> Value {
        json!({
            "piping": {
                "connections": {
                    "ssh": [
                        {
                            "host": "10.0.0.5",
                            "credentials": {
                                "username": username,
                                "password": password_secret.to_string(),
                            },
                        }
                    ]
                }
            }
        })
    }

    #[tokio::test]
    async fn missing_target_maps_to_not_found() {
        let id = Uuid::from_u128(1);
        let error = resolver(Vec::new(), None, None)
            .resolve(id)
            .await
            .expect_err("no such target");
        assert!(matches!(error, DeviceResolveError::NotFound(got) if got == id));
    }

    #[tokio::test]
    async fn target_without_address_maps_to_no_address() {
        let id = Uuid::from_u128(2);
        let error = resolver(vec![target(id, None)], None, None)
            .resolve(id)
            .await
            .expect_err("no address");
        assert!(matches!(error, DeviceResolveError::NoAddress(got) if got == id));
    }

    #[tokio::test]
    async fn blank_address_maps_to_no_address() {
        let id = Uuid::from_u128(3);
        let error = resolver(vec![target(id, Some("   "))], None, None)
            .resolve(id)
            .await
            .expect_err("blank address");
        assert!(matches!(error, DeviceResolveError::NoAddress(got) if got == id));
    }

    #[tokio::test]
    async fn no_platform_maps_to_unknown_family() {
        let id = Uuid::from_u128(4);
        let error = resolver(vec![target(id, Some("10.0.0.5"))], None, None)
            .resolve(id)
            .await
            .expect_err("no platform");
        assert!(matches!(error, DeviceResolveError::UnknownFamily(got) if got == id));
    }

    #[tokio::test]
    async fn unrecognized_driver_slug_maps_to_unknown_family() {
        let id = Uuid::from_u128(5);
        let error = resolver(
            vec![target(id, Some("10.0.0.5"))],
            Some(Uuid::from_u128(99)),
            Some("acme-not-a-family"),
        )
        .resolve(id)
        .await
        .expect_err("unknown driver");
        assert!(matches!(error, DeviceResolveError::UnknownFamily(got) if got == id));
    }

    // (a) A target that owns the password secret referenced by its config
    // resolves to the configured username and the dereferenced password.
    #[tokio::test]
    async fn resolves_target_owned_username_and_password() {
        let id = Uuid::from_u128(6);
        let secret_id = Uuid::from_u128(0xABC);
        let store = Arc::new(RecordingStore::new(
            [(secret_id, "s3cr3t".to_owned())],
            [secret_id],
        ));
        let device = resolver_with(
            vec![target(id, Some("10.0.0.5"))],
            Some(Uuid::from_u128(99)),
            Some("dvrip_sofia"),
            config_with_ssh_credentials("admin", secret_id),
            [secret_id].into_iter().collect(),
            store.clone(),
        )
        .resolve(id)
        .await
        .expect("device resolves");

        assert_eq!(device.username, "admin");
        assert_eq!(
            device
                .password
                .as_ref()
                .expect("password resolved")
                .expose(),
            "s3cr3t"
        );
        assert_eq!(
            store.reads(),
            vec![secret_id],
            "exactly the verified secret is dereferenced"
        );
    }

    // (b) The fence holds: a config-referenced password UUID that is NOT one of
    // the target's own secrets is never dereferenced, and the outcome is
    // NoCredentials.
    #[tokio::test]
    async fn unbound_candidate_secret_is_not_dereferenced() {
        let id = Uuid::from_u128(7);
        let candidate = Uuid::from_u128(0xABC);
        let owned_other = Uuid::from_u128(0xDEF);
        // The store owns only `owned_other`; a read of `candidate` would trip the
        // fence assertion inside the double.
        let store = Arc::new(RecordingStore::new(
            [(candidate, "leaked".to_owned())],
            [owned_other],
        ));
        let error = resolver_with(
            vec![target(id, Some("10.0.0.5"))],
            Some(Uuid::from_u128(99)),
            Some("dvrip_sofia"),
            config_with_ssh_credentials("admin", candidate),
            // Target owns a DIFFERENT secret; the candidate is not fence-verified.
            [owned_other].into_iter().collect(),
            store.clone(),
        )
        .resolve(id)
        .await
        .expect_err("candidate secret is not target-owned");

        assert!(matches!(error, DeviceResolveError::NoCredentials(got) if got == id));
        assert!(
            store.reads().is_empty(),
            "an unverified secret must never be dereferenced"
        );
    }

    // (c) A fully resolvable target that owns no secrets (no target-scope
    // binding) yields NoCredentials without touching the secret store.
    #[tokio::test]
    async fn no_target_scope_binding_yields_no_credentials() {
        let id = Uuid::from_u128(8);
        let secret_id = Uuid::from_u128(0xABC);
        let store = Arc::new(RecordingStore::new(
            [(secret_id, "s3cr3t".to_owned())],
            [secret_id],
        ));
        let error = resolver_with(
            vec![target(id, Some("10.0.0.5"))],
            Some(Uuid::from_u128(99)),
            Some("dvrip_sofia"),
            config_with_ssh_credentials("admin", secret_id),
            // No target-scope bindings: the target owns nothing.
            HashSet::new(),
            store.clone(),
        )
        .resolve(id)
        .await
        .expect_err("target owns no secrets");

        assert!(matches!(error, DeviceResolveError::NoCredentials(got) if got == id));
        assert!(
            store.reads().is_empty(),
            "with no owned secrets the store is never read"
        );
    }

    // (cctv 1) A target WITH a cctv launch account resolves host / port /
    // family / credentials from the typed profile, and `params.family` overrides
    // the platform-derived family.
    #[tokio::test]
    async fn video_profile_resolves_host_port_family_and_credentials() {
        let id = Uuid::from_u128(0x100);
        let secret_id = Uuid::from_u128(0x1abc);
        let record_id = Uuid::from_u128(0x1);
        let store = Arc::new(RecordingStore::new(
            [(secret_id, "vidpw".to_owned())],
            [secret_id],
        ));
        let params = json!({
            "host": "192.168.1.50",
            "mgmt_port": 8000,
            "family": "hikvision",
            "credentials": { "username": "admin", "password": secret_id.to_string() },
        });
        let device = video_resolver(
            // The read-model address and the platform family both DIFFER from the
            // cctv profile, proving the profile wins.
            vec![target(id, Some("10.9.9.9"))],
            Some(Uuid::from_u128(99)),
            Some("dvrip_sofia"),
            CctvFences {
                accounts: vec![video_account(record_id, params, Some(record_id))],
                owned: [secret_id].into_iter().collect(),
                record_bindings: HashMap::new(),
            },
            store.clone(),
        )
        .resolve(id)
        .await
        .expect("cctv device resolves");

        assert_eq!(
            device.family,
            DeviceFamily::Hikvision,
            "params.family overrides platform"
        );
        assert_eq!(
            device.endpoint.host, "192.168.1.50",
            "params.host overrides read model"
        );
        assert_eq!(
            device.endpoint.port, 8000,
            "params.mgmt_port overrides family default"
        );
        assert_eq!(device.username, "admin");
        assert_eq!(
            device
                .password
                .as_ref()
                .expect("password resolved")
                .expose(),
            "vidpw"
        );
        assert_eq!(
            store.reads(),
            vec![secret_id],
            "exactly the fenced secret is read"
        );
    }

    // (cctv 1b) The reserved endpoint placeholder in `params.host` is NOT a
    // literal endpoint: `{{primary_ip4}}` (the form per-device CCTV/VNC seeds
    // write) must fall back to the read-model address instead of reaching the
    // driver as a literal template string.
    #[tokio::test]
    async fn video_profile_placeholder_host_falls_back_to_read_model_address() {
        let id = Uuid::from_u128(0x101);
        let secret_id = Uuid::from_u128(0x1abd);
        let record_id = Uuid::from_u128(0x2);
        let store = Arc::new(RecordingStore::new(
            [(secret_id, "vidpw".to_owned())],
            [secret_id],
        ));
        let params = json!({
            "host": "{{primary_ip4}}",
            "family": "dvrip_sofia",
            "credentials": { "username": "admin", "password": secret_id.to_string() },
        });
        let device = video_resolver(
            vec![target(id, Some("10.9.9.9"))],
            Some(Uuid::from_u128(99)),
            Some("dvrip_sofia"),
            CctvFences {
                accounts: vec![video_account(record_id, params, Some(record_id))],
                owned: [secret_id].into_iter().collect(),
                record_bindings: HashMap::new(),
            },
            store,
        )
        .resolve(id)
        .await
        .expect("cctv device resolves");

        assert_eq!(
            device.endpoint.host, "10.9.9.9",
            "placeholder host resolves to the read-model address"
        );
    }

    // (cctv 2) The record-scope half of the fence: a password UUID that is NOT
    // target-owned but IS bound to the record that produced the
    // `credentials.password` leaf (`connection_config:record`, matching
    // field_key) resolves. Host falls back to the read model, port to the family
    // default, both absent from the profile.
    #[tokio::test]
    async fn video_password_bound_via_record_resolves() {
        let id = Uuid::from_u128(0x200);
        let secret_id = Uuid::from_u128(0x2abc);
        let record_id = Uuid::from_u128(0x2);
        let store = Arc::new(RecordingStore::new(
            [(secret_id, "recpw".to_owned())],
            [secret_id],
        ));
        let params = json!({
            "family": "dahua",
            "credentials": { "username": "root", "password": secret_id.to_string() },
        });
        let record_bindings = HashMap::from([(
            record_id,
            vec![("credentials.password".to_owned(), secret_id)],
        )]);
        let device = video_resolver(
            vec![target(id, Some("10.0.0.7"))],
            Some(Uuid::from_u128(99)),
            Some("dvrip_sofia"),
            CctvFences {
                accounts: vec![video_account(record_id, params, Some(record_id))],
                // NOT target-owned; only the record binding vouches for it.
                owned: HashSet::new(),
                record_bindings,
            },
            store.clone(),
        )
        .resolve(id)
        .await
        .expect("record-bound cctv credential resolves");

        assert_eq!(device.family, DeviceFamily::Dahua);
        assert_eq!(
            device.endpoint.host, "10.0.0.7",
            "no params.host -> read-model address"
        );
        assert_eq!(
            device.endpoint.port, 80,
            "no params.mgmt_port -> dahua default"
        );
        assert_eq!(device.username, "root");
        assert_eq!(
            device
                .password
                .as_ref()
                .expect("password resolved")
                .expose(),
            "recpw"
        );
        assert_eq!(store.reads(), vec![secret_id]);
    }

    // (cctv 3) The fence holds: a `credentials.password` UUID that is neither
    // target-owned NOR record-bound is NEVER dereferenced. The resolver falls
    // back, and here the fallback finds no config-context credential, so the
    // outcome is NoCredentials with the cctv secret untouched.
    #[tokio::test]
    async fn video_unbound_password_is_never_dereferenced_and_falls_back() {
        let id = Uuid::from_u128(0x300);
        let secret_id = Uuid::from_u128(0x3abc);
        let record_id = Uuid::from_u128(0x3);
        // The store does NOT own the secret: a read would trip the fence assertion.
        let store = Arc::new(RecordingStore::new(
            [(secret_id, "leaked".to_owned())],
            Vec::new(),
        ));
        let params = json!({
            "family": "hikvision",
            "credentials": { "username": "admin", "password": secret_id.to_string() },
        });
        let error = video_resolver(
            vec![target(id, Some("10.0.0.8"))],
            Some(Uuid::from_u128(99)),
            Some("dvrip_sofia"),
            CctvFences {
                accounts: vec![video_account(record_id, params, Some(record_id))],
                // Neither target-owned nor record-bound.
                owned: HashSet::new(),
                record_bindings: HashMap::new(),
            },
            store.clone(),
        )
        .resolve(id)
        .await
        .expect_err("unbound cctv password -> fall back -> no creds");

        assert!(matches!(error, DeviceResolveError::NoCredentials(got) if got == id));
        assert!(
            store.reads().is_empty(),
            "an unfenced cctv password must never be dereferenced"
        );
    }

    // (cctv literal) A `credentials.password` that is a plain literal (not a
    // UUID) is used verbatim — the path the UI's plain password field produces —
    // WITHOUT touching the secret store or the fence. This mirrors how the connect
    // path treats a non-UUID ssh/rdp/vnc password.
    #[tokio::test]
    async fn video_literal_password_is_used_verbatim() {
        let id = Uuid::from_u128(0x500);
        let record_id = Uuid::from_u128(0x5);
        // The store owns nothing and would panic on any read — proving the literal
        // path never dereferences a secret.
        let store = Arc::new(RecordingStore::empty());
        let params = json!({
            "host": "192.168.1.77",
            "family": "hikvision",
            "credentials": { "username": "admin", "password": "plain-literal-pw" },
        });
        let device = video_resolver(
            vec![target(id, Some("10.0.0.9"))],
            Some(Uuid::from_u128(99)),
            Some("dvrip_sofia"),
            CctvFences {
                accounts: vec![video_account(record_id, params, None)],
                owned: HashSet::new(),
                record_bindings: HashMap::new(),
            },
            store.clone(),
        )
        .resolve(id)
        .await
        .expect("literal cctv password resolves");

        assert_eq!(device.family, DeviceFamily::Hikvision);
        assert_eq!(device.endpoint.host, "192.168.1.77");
        assert_eq!(device.username, "admin");
        assert_eq!(
            device
                .password
                .as_ref()
                .expect("password resolved")
                .expose(),
            "plain-literal-pw"
        );
        assert!(
            store.reads().is_empty(),
            "a literal password must never touch the secret store"
        );
    }

    // (cctv 4) A target WITHOUT a cctv launch account falls back to the
    // platform + any-protocol config-context path, producing a complete device
    // (family + port from the platform driver, host from the read model,
    // credentials from the config-context walk) — the pre-cctv behavior.
    #[tokio::test]
    async fn without_video_account_falls_back_to_platform_path() {
        let id = Uuid::from_u128(0x400);
        let secret_id = Uuid::from_u128(0x4abc);
        let store = Arc::new(RecordingStore::new(
            [(secret_id, "storedpw".to_owned())],
            [secret_id],
        ));
        let device = resolver_with(
            vec![target(id, Some("10.0.0.5"))],
            Some(Uuid::from_u128(99)),
            Some("dahua"),
            config_with_ssh_credentials("operator", secret_id),
            [secret_id].into_iter().collect(),
            store.clone(),
        )
        .resolve(id)
        .await
        .expect("fallback device resolves");

        assert_eq!(
            device.family,
            DeviceFamily::Dahua,
            "family from platform driver"
        );
        assert_eq!(device.endpoint.host, "10.0.0.5", "host from read model");
        assert_eq!(device.endpoint.port, 80, "port from dahua family default");
        assert_eq!(device.username, "operator");
        assert_eq!(
            device
                .password
                .as_ref()
                .expect("password resolved")
                .expose(),
            "storedpw"
        );
        assert_eq!(store.reads(), vec![secret_id]);
    }

    // (linux 1) A Linux host's ssh launch account carries `username` and a
    // private key but NO password: it must resolve, with no local password.
    #[tokio::test]
    async fn linux_profile_without_a_password_resolves_with_no_password() {
        let id = Uuid::from_u128(0x600);
        let record_id = Uuid::from_u128(0x6);
        let store = Arc::new(RecordingStore::empty());
        let params = json!({
            "host": "10.20.30.40",
            "family": "linux",
            "credentials": { "username": "root", "private_key": "PRIVATE-KEY" },
        });
        let device = video_resolver(
            vec![target(id, Some("10.9.9.9"))],
            Some(Uuid::from_u128(99)),
            Some("linux"),
            CctvFences {
                accounts: vec![video_account(record_id, params, None)],
                owned: HashSet::new(),
                record_bindings: HashMap::new(),
            },
            store.clone(),
        )
        .resolve(id)
        .await
        .expect("linux profile resolves without a password");

        assert_eq!(device.family, DeviceFamily::Linux);
        assert_eq!(device.endpoint.host, "10.20.30.40");
        assert_eq!(device.endpoint.port, 22, "linux family default port");
        assert_eq!(device.username, "root");
        assert!(
            device.password.is_none(),
            "a Linux host needs no local password"
        );
        assert!(
            store.reads().is_empty(),
            "the profile carries no password, so no secret is read"
        );
    }

    // (linux 2) A Linux account with NO username cannot fall back to the
    // password-based config path, so it reports the existing credentials gap.
    #[tokio::test]
    async fn linux_profile_without_a_username_reports_no_credentials() {
        let id = Uuid::from_u128(0x601);
        let record_id = Uuid::from_u128(0x7);
        let store = Arc::new(RecordingStore::empty());
        let params = json!({
            "host": "10.20.30.41",
            "family": "linux",
            "credentials": { "private_key": "PRIVATE-KEY" },
        });
        let error = video_resolver(
            vec![target(id, Some("10.0.0.11"))],
            Some(Uuid::from_u128(99)),
            Some("linux"),
            CctvFences {
                accounts: vec![video_account(record_id, params, None)],
                owned: HashSet::new(),
                record_bindings: HashMap::new(),
            },
            store,
        )
        .resolve(id)
        .await
        .expect_err("linux without a username has no credentials");

        assert!(matches!(error, DeviceResolveError::NoCredentials(got) if got == id));
    }

    // (video no password) A video launch account with a username but no
    // `credentials.password` still falls back to the config-context path, and
    // with no credential there yields the same NoCredentials as before.
    #[tokio::test]
    async fn video_profile_without_a_password_falls_back() {
        let id = Uuid::from_u128(0x602);
        let record_id = Uuid::from_u128(0x8);
        let store = Arc::new(RecordingStore::empty());
        let params = json!({
            "host": "192.168.1.88",
            "family": "hikvision",
            "credentials": { "username": "admin" },
        });
        let error = video_resolver(
            vec![target(id, Some("10.0.0.12"))],
            Some(Uuid::from_u128(99)),
            Some("hikvision"),
            CctvFences {
                accounts: vec![video_account(record_id, params, None)],
                owned: HashSet::new(),
                record_bindings: HashMap::new(),
            },
            store,
        )
        .resolve(id)
        .await
        .expect_err("video without a password falls back and finds none");

        assert!(matches!(error, DeviceResolveError::NoCredentials(got) if got == id));
    }

    // Determinism: with fence-verified credentials under several protocol slots,
    // selection is stable (sorted protocol order), independent of the store.
    #[test]
    fn launch_protocol_follows_the_device_family() {
        assert_eq!(
            launch_protocol(Some(DeviceFamily::Linux)),
            ConnConfProtocol::Ssh
        );
        assert_eq!(
            launch_protocol(Some(DeviceFamily::Dahua)),
            ConnConfProtocol::Cctv
        );
        assert_eq!(launch_protocol(None), ConnConfProtocol::Cctv);
    }

    #[test]
    fn credential_selection_is_deterministic_across_protocols() {
        let ssh_secret = Uuid::from_u128(0x11);
        let rdp_secret = Uuid::from_u128(0x22);
        let config = json!({
            "piping": {
                "connections": {
                    "ssh": [ { "credentials": { "username": "ssh-user", "password": ssh_secret.to_string() } } ],
                    "rdp": [ { "credentials": { "username": "rdp-user", "password": rdp_secret.to_string() } } ],
                }
            }
        });
        let allowed: HashSet<Uuid> = [ssh_secret, rdp_secret].into_iter().collect();

        let (username, secret_id) =
            select_target_credential(&config, &allowed).expect("a credential is selected");
        // "rdp" sorts before "ssh": the lower protocol key wins deterministically.
        assert_eq!(username, "rdp-user");
        assert_eq!(secret_id, rdp_secret);
    }

    // A single-object entry shape (not wrapped in an array) is also accepted.
    #[test]
    fn single_object_entry_shape_is_supported() {
        let secret_id = Uuid::from_u128(0x33);
        let config = json!({
            "piping": {
                "connections": {
                    "ssh": { "credentials": { "username": "root", "password": secret_id.to_string() } }
                }
            }
        });
        let allowed: HashSet<Uuid> = [secret_id].into_iter().collect();

        let (username, got) =
            select_target_credential(&config, &allowed).expect("credential from object entry");
        assert_eq!(username, "root");
        assert_eq!(got, secret_id);
    }
}
