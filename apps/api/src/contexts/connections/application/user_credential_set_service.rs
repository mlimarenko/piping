use std::sync::Arc;

use chrono::Utc;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::contexts::connections::domain::{ConnectionProtocol, UserCredentialSet};
use crate::contexts::connections::infrastructure::{ConnectionRepository, UserCredentialSetFilter};
use crate::contexts::shared::list_query::SortSpec;
use crate::errors::AppError;

const DISALLOWED_SECRET_KEYS: &[&str] = &["password", "private_key"];
const DISALLOWED_TOP_LEVEL_CREDENTIAL_KEYS: &[&str] = &["username", "domain"];
const RDP_OPTION_KEYS: &[&str] = &[
    "width",
    "height",
    "dpi",
    "color_depth",
    "security",
    "ignore_cert",
    "disable_auth",
    "disable_gfx",
];
const CREDENTIAL_SET_PARAM_KEYS: &[&str] = &[
    "host",
    "port",
    "timeout",
    "probe_only",
    "label",
    "color",
    "credentials",
    "options",
];
const SSH_CREDENTIAL_KEYS: &[&str] = &["username"];
const RDP_CREDENTIAL_KEYS: &[&str] = &["username", "domain"];
const VNC_CREDENTIAL_KEYS: &[&str] = &[];

#[derive(Debug, Clone)]
pub struct CreateUserCredentialSetInput {
    pub protocol: ConnectionProtocol,
    pub name: String,
    pub params: Value,
    pub make_default: bool,
}

#[derive(Debug, Clone)]
pub struct UpdateUserCredentialSetInput {
    pub name: String,
    pub params: Value,
}

/// Filter parameters for one page of a user's credential sets.
#[derive(Debug, Clone, Default)]
pub struct UserCredentialSetQuery {
    pub protocol: Vec<ConnectionProtocol>,
    pub is_default: Vec<String>,
    pub sort: Option<SortSpec>,
}

pub struct UserCredentialSetService<R>
where
    R: ConnectionRepository,
{
    repository: Arc<R>,
}

impl<R> UserCredentialSetService<R>
where
    R: ConnectionRepository,
{
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    /// One page of the user's credential sets plus the TOTAL number of
    /// matches, so the REST envelope can report a page count.
    pub async fn list_for_user(
        &self,
        user_id: Uuid,
        query: UserCredentialSetQuery,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<UserCredentialSet>, i64), AppError> {
        self.repository
            .list_user_credential_sets(
                UserCredentialSetFilter {
                    user_id: vec![user_id],
                    protocol: query.protocol,
                    is_default: query.is_default,
                    sort: query.sort,
                },
                Some(i64::from(limit)),
                Some(i64::from(offset)),
            )
            .await
    }

    pub async fn get_for_user(
        &self,
        user_id: Uuid,
        set_id: Uuid,
    ) -> Result<UserCredentialSet, AppError> {
        self.require_owned_set(user_id, set_id).await
    }

    pub async fn create_for_user(
        &self,
        user_id: Uuid,
        input: CreateUserCredentialSetInput,
    ) -> Result<UserCredentialSet, AppError> {
        validate_protocol(&input.protocol)?;
        let name = normalize_name(&input.name)?;
        let params = normalize_params(&input.protocol, &input.params)?;
        let now = Utc::now();
        let mut set = UserCredentialSet {
            id: Uuid::now_v7(),
            user_id,
            protocol: input.protocol.clone(),
            name,
            params,
            is_default: false,
            created_at: now,
            updated_at: now,
        };
        self.repository.create_user_credential_set(&set).await?;

        // Only the unpaged match count matters here (is the new set the only
        // one for this protocol?), so fetch the smallest possible page.
        let (_, total) = self
            .repository
            .list_user_credential_sets(
                UserCredentialSetFilter {
                    user_id: vec![user_id],
                    protocol: vec![input.protocol.clone()],
                    is_default: Vec::new(),
                    sort: None,
                },
                Some(1),
                None,
            )
            .await?;
        let should_make_default = input.make_default || total == 1;
        if should_make_default {
            self.repository
                .set_default_user_credential_set(user_id, input.protocol, set.id, Utc::now())
                .await?;
            set.is_default = true;
            set.updated_at = Utc::now();
        }
        Ok(set)
    }

    pub async fn update_for_user(
        &self,
        user_id: Uuid,
        set_id: Uuid,
        input: UpdateUserCredentialSetInput,
    ) -> Result<UserCredentialSet, AppError> {
        let mut set = self.require_owned_set(user_id, set_id).await?;
        set.name = normalize_name(&input.name)?;
        set.params = normalize_params(&set.protocol, &input.params)?;
        set.updated_at = Utc::now();
        self.repository.update_user_credential_set(&set).await?;
        Ok(set)
    }

    pub async fn set_default_for_user(&self, user_id: Uuid, set_id: Uuid) -> Result<(), AppError> {
        let set = self.require_owned_set(user_id, set_id).await?;
        self.repository
            .set_default_user_credential_set(user_id, set.protocol, set.id, Utc::now())
            .await
    }

    pub async fn delete_for_user(&self, user_id: Uuid, set_id: Uuid) -> Result<(), AppError> {
        let set = self.require_owned_set(user_id, set_id).await?;
        if set.is_default {
            return Err(AppError::Conflict(
                "Default credential set cannot be deleted".to_string(),
            ));
        }
        self.repository.delete_user_credential_set(set.id).await
    }

    async fn require_owned_set(
        &self,
        user_id: Uuid,
        set_id: Uuid,
    ) -> Result<UserCredentialSet, AppError> {
        let set = self
            .repository
            .get_user_credential_set(set_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Credential set not found".to_string()))?;
        if set.user_id != user_id {
            return Err(AppError::Forbidden(
                "Cannot modify credential set owned by another user".to_string(),
            ));
        }
        Ok(set)
    }
}

fn normalize_name(value: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(
            "Credential set name is required".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_protocol(protocol: &ConnectionProtocol) -> Result<(), AppError> {
    match protocol {
        ConnectionProtocol::Ssh | ConnectionProtocol::Rdp | ConnectionProtocol::Vnc => Ok(()),
        _ => Err(AppError::Validation(
            "Credential sets are supported only for ssh/rdp/vnc".to_string(),
        )),
    }
}

fn normalize_params(protocol: &ConnectionProtocol, value: &Value) -> Result<Value, AppError> {
    let mut normalized = value.clone();
    let Value::Object(map) = &mut normalized else {
        return Err(AppError::Validation(
            "Credential set params must be a JSON object".to_string(),
        ));
    };
    assert_no_disallowed_secret_keys(value)?;
    if map
        .get("credentials")
        .is_some_and(|value| !value.is_object())
    {
        return Err(AppError::Validation(
            "Credential set params.credentials must be a JSON object".to_string(),
        ));
    }
    if map.get("options").is_some_and(|value| !value.is_object()) {
        return Err(AppError::Validation(
            "Credential set params.options must be a JSON object".to_string(),
        ));
    }

    for &key in DISALLOWED_TOP_LEVEL_CREDENTIAL_KEYS {
        if map.contains_key(key) {
            return Err(AppError::Validation(format!(
                "Credential set param '{}' must be nested under credentials",
                key
            )));
        }
    }
    for key in map.keys() {
        if !CREDENTIAL_SET_PARAM_KEYS.contains(&key.as_str()) {
            return Err(AppError::Validation(format!(
                "Unsupported credential set param '{key}' for protocol '{}'",
                protocol.as_str()
            )));
        }
    }
    if let Some(credentials) = map.get("credentials").and_then(Value::as_object) {
        let allowed_credentials = credential_keys_for_protocol(protocol);
        for key in credentials.keys() {
            if !allowed_credentials.contains(&key.as_str()) {
                return Err(AppError::Validation(format!(
                    "Unsupported credential set credential '{key}' for protocol '{}'",
                    protocol.as_str()
                )));
            }
        }
    }
    if let Some(options) = map.get("options").and_then(Value::as_object) {
        if *protocol != ConnectionProtocol::Rdp && !options.is_empty() {
            return Err(AppError::Validation(
                "Credential set params.options is supported only for rdp".to_string(),
            ));
        }
        for key in options.keys() {
            if !RDP_OPTION_KEYS.contains(&key.as_str()) {
                return Err(AppError::Validation(format!(
                    "Unsupported credential set option '{key}' for protocol '{}'",
                    protocol.as_str()
                )));
            }
        }
    }
    if *protocol == ConnectionProtocol::Rdp
        && let Some(credentials) = map.get_mut("credentials").and_then(Value::as_object_mut)
    {
        normalize_rdp_credential_params(credentials)?;
    }

    Ok(normalized)
}

fn normalize_rdp_credential_params(credentials: &mut Map<String, Value>) -> Result<(), AppError> {
    let username = take_optional_credential_string(credentials, "username")?;
    let domain = take_optional_credential_string(credentials, "domain")?;
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

fn take_optional_credential_string(
    credentials: &mut Map<String, Value>,
    key: &str,
) -> Result<Option<String>, AppError> {
    match credentials.remove(key) {
        None => Ok(None),
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(value) => {
            credentials.insert(key.to_string(), value);
            Err(AppError::Validation(format!(
                "Credential set params.credentials.{key} must be a string"
            )))
        }
    }
}

fn credential_keys_for_protocol(protocol: &ConnectionProtocol) -> &'static [&'static str] {
    match protocol {
        ConnectionProtocol::Ssh => SSH_CREDENTIAL_KEYS,
        ConnectionProtocol::Rdp => RDP_CREDENTIAL_KEYS,
        ConnectionProtocol::Vnc => VNC_CREDENTIAL_KEYS,
        ConnectionProtocol::App | ConnectionProtocol::Cctv => &[],
    }
}

fn assert_no_disallowed_secret_keys(value: &Value) -> Result<(), AppError> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let normalized_key = key.trim().to_ascii_lowercase();
                if DISALLOWED_SECRET_KEYS.contains(&normalized_key.as_str()) {
                    return Err(AppError::Validation(
                        "Credential set params must not include password/private_key. Use secret fields instead."
                            .to_string(),
                    ));
                }
                assert_no_disallowed_secret_keys(child)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for item in items {
                assert_no_disallowed_secret_keys(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::normalize_params;
    use crate::contexts::connections::domain::ConnectionProtocol;
    use crate::errors::AppError;

    fn assert_validation_error_contains<T: std::fmt::Debug>(
        result: Result<T, AppError>,
        needle: &str,
    ) {
        assert!(
            matches!(
                &result,
                Err(AppError::Validation(message)) if message.contains(needle)
            ),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn normalize_params_accepts_canonical_shape() {
        let params = json!({
            "port": 3389,
            "credentials": {
                "username": "operator",
                "domain": "GRAD"
            },
            "options": {
                "width": 1920,
                "height": 1080,
                "color_depth": 32
            }
        });
        let result = normalize_params(&ConnectionProtocol::Rdp, &params);
        assert!(result.is_ok(), "canonical params should pass: {result:?}");
        let normalized = result.unwrap_or_default();
        assert_eq!(normalized, params);
    }

    #[test]
    fn normalize_params_keeps_rdp_upn_username_opaque() {
        let result = normalize_params(
            &ConnectionProtocol::Rdp,
            &json!({
                "credentials": {
                    "username": "operator@example.test"
                }
            }),
        )
        .expect("the UPN username should normalize");

        assert_eq!(
            result
                .pointer("/credentials/username")
                .and_then(Value::as_str),
            Some("operator@example.test")
        );
        assert_eq!(
            result
                .pointer("/credentials/domain")
                .and_then(Value::as_str),
            None
        );
    }

    #[test]
    fn normalize_params_splits_rdp_backslash_username() {
        let result = normalize_params(
            &ConnectionProtocol::Rdp,
            &json!({
                "credentials": {
                    "username": "GRAD\\operator"
                }
            }),
        )
        .expect("domain-qualified username should normalize");

        assert_eq!(
            result
                .pointer("/credentials/username")
                .and_then(Value::as_str),
            Some("operator")
        );
        assert_eq!(
            result
                .pointer("/credentials/domain")
                .and_then(Value::as_str),
            Some("GRAD")
        );
    }

    #[test]
    fn normalize_params_rejects_rdp_conflicting_explicit_domain() {
        let result = normalize_params(
            &ConnectionProtocol::Rdp,
            &json!({
                "credentials": {
                    "username": "GRAD\\operator",
                    "domain": "OTHER"
                }
            }),
        );

        assert_validation_error_contains(result, "conflicts");
    }

    #[test]
    fn normalize_params_rejects_non_object_payload() {
        let result = normalize_params(&ConnectionProtocol::Rdp, &json!(["bad"]));
        assert_validation_error_contains(result, "JSON object");
    }

    #[test]
    fn normalize_params_rejects_top_level_username() {
        let result = normalize_params(&ConnectionProtocol::Rdp, &json!({ "username": "operator" }));
        assert_validation_error_contains(result, "must be nested under credentials");
    }

    #[test]
    fn normalize_params_rejects_top_level_domain() {
        let result = normalize_params(&ConnectionProtocol::Rdp, &json!({ "domain": "GRAD" }));
        assert_validation_error_contains(result, "must be nested under credentials");
    }

    #[test]
    fn normalize_params_rejects_unknown_top_level_key() {
        let result = normalize_params(&ConnectionProtocol::Ssh, &json!({ "retired_mode": true }));
        assert_validation_error_contains(result, "Unsupported credential set param");
    }

    #[test]
    fn normalize_params_rejects_non_object_credentials() {
        let result = normalize_params(&ConnectionProtocol::Rdp, &json!({ "credentials": "bad" }));
        assert_validation_error_contains(result, "credentials must be a JSON object");
    }

    #[test]
    fn normalize_params_rejects_protocol_invalid_credential_key() {
        let result = normalize_params(
            &ConnectionProtocol::Vnc,
            &json!({ "credentials": { "username": "operator" } }),
        );
        assert_validation_error_contains(result, "Unsupported credential set credential");
    }

    #[test]
    fn normalize_params_rejects_option_fields_inside_credentials() {
        let result = normalize_params(
            &ConnectionProtocol::Rdp,
            &json!({
                "credentials": {
                    "username": "operator",
                    "screen_resolution": "1920x1080"
                }
            }),
        );
        assert_validation_error_contains(result, "Unsupported credential set credential");
    }

    #[test]
    fn normalize_params_rejects_non_object_options() {
        let result = normalize_params(&ConnectionProtocol::Rdp, &json!({ "options": "bad" }));
        assert_validation_error_contains(result, "options must be a JSON object");
    }

    #[test]
    fn normalize_params_rejects_screen_resolution_option() {
        let result = normalize_params(
            &ConnectionProtocol::Rdp,
            &json!({
                "options": {
                    "screen_resolution": "1920x1080"
                }
            }),
        );
        assert_validation_error_contains(result, "Unsupported credential set option");
    }

    #[test]
    fn normalize_params_rejects_secret_fields_in_params() {
        let result = normalize_params(
            &ConnectionProtocol::Rdp,
            &json!({
                "credentials": {
                    "password": "plaintext"
                }
            }),
        );
        assert_validation_error_contains(result, "must not include password/private_key");
    }
}
