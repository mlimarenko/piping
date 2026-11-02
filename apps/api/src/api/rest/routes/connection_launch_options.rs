//! Read-only REST endpoint that drives the connect-panel buttons.
//!
//! `GET /api/v1/connections/targets/{id}/launch-options` returns, per
//! protocol, an ordered list of launchable ACCOUNTS for a device/VM:
//! predefined/shared accounts (resolved by the non-merging
//! `connection_config::resolve_launch_accounts`, "Option A") followed by the
//! caller's personal credential sets only when a connection-config account or
//! fallback makes that protocol available on this target.
//!
//! ## Secret handling
//!
//! Secret values are NEVER returned. The resolved params carry bare-UUID secret
//! references in-process; this module surfaces only:
//!   * `username` — display only, and ONLY when it is a literal (a UUID-shaped
//!     username is treated as a secret_ref and omitted, matching the
//!     "username is never a secret" rule in
//!     `connection_service::connconf_resolve`);
//!   * `requires_input` — a boolean presence heuristic (no resolvable stored
//!     password/private_key for a protocol that needs auth).
//!
//! ## entry_id ordering contract
//!
//! The position of each shared account in the resolver output is the `index` in
//! `config:connconf:<proto>:<index>`; personal sets are appended as
//! `user:connconf:<proto>:<index>:<uuid>`. The later connect-path phase MUST
//! rebuild the same shared ordering to select by index (documented in
//! `connection_config::launch_accounts_resolver`).

use std::collections::{HashMap, HashSet};

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::connection_authz_attributes::connection_target_authz_attributes_for;
use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::Path;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::connection_config::LaunchAccount;
use crate::contexts::connections::application::ConnectionLaunchOptions;
use crate::contexts::connections::domain::{
    ConnectionProtocol, USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_PASSWORD,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PASSWORD,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PRIVATE_KEY,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_VNC_PASSWORD,
    USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE, UserCredentialSet,
};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

/// The protocols surfaced as connect buttons. `app` is a launcher target
/// (web URI, protocol handler, or trusted local desktop command) instead of a
/// proxied session; `cctv` is a managed cctv-device (recorder/camera) plane.
const LAUNCH_PROTOCOLS: [ConnectionProtocol; 5] = [
    ConnectionProtocol::Ssh,
    ConnectionProtocol::Rdp,
    ConnectionProtocol::Vnc,
    ConnectionProtocol::App,
    ConnectionProtocol::Cctv,
];

/// `<src>` token used in entry selectors for connection-config accounts. A free
/// token by the entry-selector grammar; `connconf` identifies this source.
const ENTRY_SOURCE: &str = "connconf";

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionLaunchAccountSourceResponse {
    /// Scope layer the shared account's record was matched at (shared only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_type: Option<String>,
    /// Scope id for `scope_type` (shared only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    /// The contributing connection-config record name (shared only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_name: Option<String>,
    /// The personal credential set name (personal only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_set_name: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionLaunchAccountResponse {
    /// Launch selector: `config:connconf:<proto>:<index>` (shared) or
    /// `user:connconf:<proto>:<index>:<uuid>` (personal).
    pub entry_id: String,
    /// `"shared"` or `"personal"`.
    pub kind: String,
    /// Display label: `ui.label` (shared) or the credential set name (personal),
    /// falling back to the record name / protocol.
    pub label: String,
    /// Optional `ui.color` for the button.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Optional `ui.icon` for the button.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Display-only username (NEVER a secret). Omitted when it is a UUID-shaped
    /// secret reference or absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Launch target (app protocol only). Absolute URI/custom scheme for a browser,
    /// or a trusted local command for Piping Desktop. Absent for session
    /// protocols (ssh/rdp/vnc) and for personal accounts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Where the account came from (shared scope/record or personal set name).
    pub source: ConnectionLaunchAccountSourceResponse,
    /// Whether this is the default account for the protocol (personal accounts
    /// only; always `false` for shared accounts).
    pub is_default: bool,
    /// Best-effort: true when the account has no resolvable stored credential
    /// (password / private_key) and the protocol needs auth, so the UI should
    /// prompt for credentials before launch.
    pub requires_input: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionLaunchProtocolResponse {
    /// Protocol slug (`ssh` / `rdp` / `vnc` / `app`).
    pub protocol: String,
    /// Model render hint: `"auto"` (default), `"buttons"`, or `"split"`.
    pub render: String,
    /// Ordered accounts: shared first (resolver order), then personal. Empty
    /// means the protocol is not configured for this target.
    pub accounts: Vec<ConnectionLaunchAccountResponse>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionLaunchOptionsResponse {
    pub host_id: String,
    pub protocols: Vec<ConnectionLaunchProtocolResponse>,
}

/// Resolve launch options for the canonical connection-target route wrapper.
pub async fn get_connection_launch_options(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);

    let host_id = match parse_required_uuid(&id, "id") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    // Authorize a Read against the specific connection target, like the other
    // target-scoped connect surfaces.
    if let Err(error) = ensure_host_read(&state, &request_context, host_id).await {
        return handle_app_error(error);
    }

    let user_id = match resolve_user_id(&request_context) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    // The connconf scope ancestry, its assignments and the record set do not
    // depend on the protocol: load them ONCE for all LAUNCH_PROTOCOLS instead of
    // three round-trips per protocol.
    let shared_inputs = match state
        .services
        .connection
        .load_shared_launch_inputs(host_id)
        .await
    {
        Ok(inputs) => inputs,
        Err(error) => return handle_app_error(error),
    };

    let mut protocols = Vec::with_capacity(LAUNCH_PROTOCOLS.len());
    for protocol in &LAUNCH_PROTOCOLS {
        let options = match state
            .services
            .connection
            .resolve_launch_options_with_inputs(shared_inputs.as_ref(), host_id, user_id, protocol)
            .await
        {
            Ok(options) => options,
            Err(error) => return handle_app_error(error),
        };
        let response = match build_protocol_response(&state, protocol, &options).await {
            Ok(response) => response,
            Err(error) => return handle_app_error(error),
        };
        protocols.push(response);
    }

    (
        StatusCode::OK,
        Json(ConnectionLaunchOptionsResponse {
            host_id: host_id.to_string(),
            protocols,
        }),
    )
        .into_response()
}

/// Build the per-protocol response: shared accounts (resolver order) then any
/// eligible personal credential sets, with the model-driven `render` hint.
async fn build_protocol_response(
    state: &AppState,
    protocol: &ConnectionProtocol,
    options: &ConnectionLaunchOptions,
) -> Result<ConnectionLaunchProtocolResponse, AppError> {
    let proto = protocol.as_str();
    let mut accounts = Vec::new();

    let is_app = matches!(protocol, ConnectionProtocol::App);
    let personal_secret_fields = personal_secret_fields_by_set_id(state, options).await?;

    for (index, account) in options.shared_accounts.iter().enumerate() {
        accounts.push(shared_account_response(
            protocol, proto, index, account, is_app,
        ));
    }
    for (index, set) in options.personal_credential_sets.iter().enumerate() {
        let secret_fields = personal_secret_fields.get(&set.id);
        accounts.push(personal_account_response(
            protocol,
            proto,
            index,
            set,
            is_app,
            secret_fields,
        ));
    }

    let render = resolve_render_hint(&options.shared_accounts);

    Ok(ConnectionLaunchProtocolResponse {
        protocol: proto.to_string(),
        render,
        accounts,
    })
}

async fn personal_secret_fields_by_set_id(
    state: &AppState,
    options: &ConnectionLaunchOptions,
) -> Result<HashMap<Uuid, HashSet<String>>, AppError> {
    let mut by_set_id = HashMap::with_capacity(options.personal_credential_sets.len());
    for set in &options.personal_credential_sets {
        let states = state
            .services
            .secret_field_states
            .list_for_consumer(USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE, set.id)
            .await?;
        let fields = states
            .into_iter()
            .filter(|state| state.enabled && state.has_value)
            .map(|state| state.field_key)
            .collect::<HashSet<_>>();
        by_set_id.insert(set.id, fields);
    }
    Ok(by_set_id)
}

/// Resolve the `render` hint from the shared accounts' `ui.render`. The
/// most-specific account (last in resolver order) wins; defaults to `"auto"` and
/// only `"buttons"` / `"split"` are honored.
fn resolve_render_hint(shared_accounts: &[LaunchAccount]) -> String {
    let hint = shared_accounts
        .iter()
        .rev()
        .find_map(|account| {
            account
                .params
                .get("ui")
                .and_then(Value::as_object)
                .and_then(|ui| ui.get("render"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    match hint.as_str() {
        "buttons" => "buttons".to_string(),
        "split" => "split".to_string(),
        _ => "auto".to_string(),
    }
}

fn shared_account_response(
    protocol: &ConnectionProtocol,
    proto: &str,
    index: usize,
    account: &LaunchAccount,
    is_app: bool,
) -> ConnectionLaunchAccountResponse {
    let ui = account.params.get("ui").and_then(Value::as_object);
    let label = ui
        .and_then(|ui| ui.get("label"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if account.record_name.trim().is_empty() {
                proto.to_uppercase()
            } else {
                account.record_name.clone()
            }
        });
    let color = ui
        .and_then(|ui| ui.get("color"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let icon = ui
        .and_then(|ui| ui.get("icon"))
        .and_then(Value::as_str)
        .map(str::to_string);

    ConnectionLaunchAccountResponse {
        entry_id: format!("config:{ENTRY_SOURCE}:{proto}:{index}"),
        kind: "shared".to_string(),
        label,
        color,
        icon,
        username: if is_app {
            None
        } else {
            display_username(&account.params)
        },
        uri: if is_app {
            display_uri(&account.params)
        } else {
            None
        },
        source: ConnectionLaunchAccountSourceResponse {
            scope_type: account.scope_type.map(|s| s.as_str().to_string()),
            scope_id: account.scope_id.map(|id| id.to_string()),
            record_name: Some(account.record_name.clone()),
            credential_set_name: None,
        },
        // Shared accounts are never marked default (there is no per-host default
        // flag); the UI defaults to the personal default or the first account.
        is_default: false,
        // App is a URI launch; it needs no credentials, so it never prompts.
        requires_input: !is_app && requires_input_from_params(protocol, &account.params),
    }
}

fn personal_account_response(
    protocol: &ConnectionProtocol,
    proto: &str,
    index: usize,
    set: &UserCredentialSet,
    is_app: bool,
    secret_fields: Option<&HashSet<String>>,
) -> ConnectionLaunchAccountResponse {
    let label = if set.name.trim().is_empty() {
        proto.to_uppercase()
    } else {
        set.name.clone()
    };
    ConnectionLaunchAccountResponse {
        entry_id: format!("user:{ENTRY_SOURCE}:{proto}:{index}:{}", set.id),
        kind: "personal".to_string(),
        label,
        color: None,
        icon: None,
        username: if is_app {
            None
        } else {
            display_username(&set.params)
        },
        // Personal credential sets never carry a launch URI; the app target lives
        // on the shared connection-config record's `params.uri`.
        uri: None,
        source: ConnectionLaunchAccountSourceResponse {
            scope_type: None,
            scope_id: None,
            record_name: None,
            credential_set_name: Some(set.name.clone()),
        },
        is_default: set.is_default,
        requires_input: !is_app
            && requires_input_from_personal_credentials(protocol, &set.params, secret_fields),
    }
}

/// Extract a display-only `credentials.username`. Returns `None` when the
/// username is absent, blank, or a UUID-shaped secret reference (the
/// "username is never a secret" rule: such a value is a secret_ref, not a
/// human-facing login, so it must not be displayed).
fn display_username(params: &Value) -> Option<String> {
    let username = params
        .get("credentials")
        .and_then(Value::as_object)
        .and_then(|creds| creds.get("username"))
        .and_then(Value::as_str)?
        .trim();
    if username.is_empty() {
        return None;
    }
    if Uuid::parse_str(username).is_ok() {
        return None;
    }
    Some(username.to_string())
}

/// Extract the app launch target from `params.url`. Returns `None` when absent
/// or blank. The URI is a plain string (e.g. `https://host:631`, `mailto:…`,
/// `app://…`); it is not a secret, so it is surfaced verbatim.
fn display_uri(params: &Value) -> Option<String> {
    let uri = params.get("url").and_then(Value::as_str)?.trim();
    if uri.is_empty() {
        return None;
    }
    Some(uri.to_string())
}

/// Best-effort shared-account `requires_input` heuristic. Presence — not value —
/// is all we expose for a read-only listing. SSH may authenticate via key OR
/// password; RDP/VNC via password. RDP `disable_auth=true` intentionally does
/// not require a password.
fn requires_input_from_params(protocol: &ConnectionProtocol, params: &Value) -> bool {
    if matches!(protocol, ConnectionProtocol::App) {
        return false;
    }
    match protocol {
        ConnectionProtocol::Ssh => {
            !has_param_secret(params, "password") && !has_param_secret(params, "private_key")
        }
        ConnectionProtocol::Rdp => {
            !rdp_disable_auth_enabled(params) && !has_param_secret(params, "password")
        }
        ConnectionProtocol::Vnc => !has_param_secret(params, "password"),
        ConnectionProtocol::App | ConnectionProtocol::Cctv => false,
    }
}

/// Personal credential sets keep password/key material in secret-fields, not in
/// `params`, so launch options must use secret-field presence for the auth
/// readiness bit while still never returning the secret value.
fn requires_input_from_personal_credentials(
    protocol: &ConnectionProtocol,
    params: &Value,
    secret_fields: Option<&HashSet<String>>,
) -> bool {
    if matches!(protocol, ConnectionProtocol::App) {
        return false;
    }
    let has_secret_field =
        |field_key: &str| secret_fields.is_some_and(|fields| fields.contains(field_key));
    match protocol {
        ConnectionProtocol::Ssh => {
            !has_secret_field(USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PASSWORD)
                && !has_secret_field(USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PRIVATE_KEY)
                && requires_input_from_params(protocol, params)
        }
        ConnectionProtocol::Rdp => {
            !rdp_disable_auth_enabled(params)
                && !has_secret_field(USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_PASSWORD)
                && !has_param_secret(params, "password")
        }
        ConnectionProtocol::Vnc => {
            !has_secret_field(USER_CONNECTION_CREDENTIAL_FIELD_KEY_VNC_PASSWORD)
                && !has_param_secret(params, "password")
        }
        ConnectionProtocol::App | ConnectionProtocol::Cctv => false,
    }
}

fn has_param_secret(params: &Value, key: &str) -> bool {
    params
        .get("credentials")
        .and_then(Value::as_object)
        .is_some_and(|creds| non_empty_leaf(creds.get(key)))
}

fn rdp_disable_auth_enabled(params: &Value) -> bool {
    params
        .get("options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("disable_auth"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// True when a JSON leaf is present and non-empty (a stored credential
/// reference). A `{"unset": true}` sentinel or blank string counts as absent.
fn non_empty_leaf(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Object(map)) => {
            // A non-empty object is a value UNLESS it is the `{"unset": true}`
            // deletion sentinel.
            let is_unset_sentinel = map.len() == 1 && map.get("unset") == Some(&Value::Bool(true));
            !map.is_empty() && !is_unset_sentinel
        }
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

fn parse_required_uuid(value: &str, field: &str) -> Result<Uuid, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!("{field} cannot be empty")));
    }
    Uuid::parse_str(trimmed)
        .map_err(|error| AppError::Validation(format!("Invalid {field}: {error}")))
}

fn resolve_user_id(request_context: &RequestContext) -> Result<Uuid, AppError> {
    request_context
        .user_id
        .ok_or_else(|| AppError::Auth("authentication required".to_string()))
}

/// Reads of a device's launch options expose its accounts; gate the SAME way the
/// connect path does — `Action::Read` + `ResourceType::ConnectionTarget` against the target.
async fn ensure_host_read(
    state: &AppState,
    request_context: &RequestContext,
    host_id: Uuid,
) -> Result<(), AppError> {
    // Canonical `connection_target:{id}` form so resource-scoped DENY policies
    // (authored in the prefixed form) actually match; a bare id silently skips
    // prefixed patterns. Attributes let site/role/tenant-scoped Read policies
    // apply here the same way they do on the monitoring read path;
    // no protocol — launch options are read before a protocol is chosen.
    let resource_id = format!("connection_target:{host_id}");
    let attributes = connection_target_authz_attributes_for(state, host_id, None).await?;
    build_policy_evaluation_service(state)
        .ensure_access_with_attributes(
            request_context,
            Action::Read,
            ResourceType::ConnectionTarget,
            Some(resource_id.as_str()),
            Some(attributes),
        )
        .await
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::{
        USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_PASSWORD,
        USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PRIVATE_KEY, requires_input_from_params,
        requires_input_from_personal_credentials,
    };
    use crate::contexts::connections::domain::ConnectionProtocol;

    #[test]
    fn personal_rdp_password_secret_clears_requires_input() {
        let params = json!({
            "credentials": {
                "username": "operator"
            }
        });
        let secret_fields =
            HashSet::from([USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_PASSWORD.to_string()]);

        assert!(!requires_input_from_personal_credentials(
            &ConnectionProtocol::Rdp,
            &params,
            Some(&secret_fields),
        ));
    }

    #[test]
    fn personal_rdp_without_password_secret_requires_input() {
        let params = json!({
            "credentials": {
                "username": "operator"
            }
        });

        assert!(requires_input_from_personal_credentials(
            &ConnectionProtocol::Rdp,
            &params,
            Some(&HashSet::new()),
        ));
    }

    #[test]
    fn rdp_disable_auth_clears_requires_input_without_password() {
        let params = json!({
            "credentials": {
                "username": "operator"
            },
            "options": {
                "disable_auth": true
            }
        });

        assert!(!requires_input_from_params(
            &ConnectionProtocol::Rdp,
            &params,
        ));
        assert!(!requires_input_from_personal_credentials(
            &ConnectionProtocol::Rdp,
            &params,
            Some(&HashSet::new()),
        ));
    }

    #[test]
    fn ssh_private_key_secret_clears_requires_input() {
        let params = json!({
            "credentials": {
                "username": "operator"
            }
        });
        let secret_fields =
            HashSet::from([USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PRIVATE_KEY.to_string()]);

        assert!(!requires_input_from_personal_credentials(
            &ConnectionProtocol::Ssh,
            &params,
            Some(&secret_fields),
        ));
    }
}
