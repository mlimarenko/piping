use crate::contexts::connections::domain::USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE;
use crate::errors::AppError;

/// Consumer type for secrets referenced by a `connconf_record` credential
/// field. The binding (keyed by field path) is the canonical marker that a
/// field is a secret; the record `params` store the bare secret UUID. Reuses the
/// existing `secret_usage_binding` layer (no new secret storage).
pub const CONNECTION_CONFIG_RECORD_CONSUMER_TYPE: &str = "connection_config:record";
pub const CONNECTION_TARGET_SECRET_CONSUMER_TYPE: &str = "connection_scope:connection_target";
pub const ASSISTANT_INSTANCE_CONSUMER_TYPE: &str = "assistant_instance";
pub(crate) fn assistant_instance_id() -> Result<uuid::Uuid, AppError> {
    uuid::Uuid::parse_str("00000000-0000-7000-8000-000000000001").map_err(|error| {
        AppError::InvalidConfig(format!(
            "Invalid assistant instance secret scope id: {error}"
        ))
    })
}
pub const PLAYBOOK_SERVICE_PRINCIPAL_CREDENTIAL_CONSUMER_TYPE: &str =
    "playbook_service_principal:credential";
pub const PROXY_NODE_CREDENTIAL_CONSUMER_TYPE: &str = "proxy_node:credential";
/// Consumer type projecting a playbook version's `token_policy.runtime.secrets.env`
/// declaration into `secret_usage_binding` (consumer_id = version id, field_key =
/// ENV_NAME, owner_scope = ('playbook', playbook_id)). Unlike the other consumer
/// types this is a USAGE projection only — no secret value is written through it
/// (values live in the named secret itself), so it is intentionally NOT part of
/// `VALID_SECRET_FIELD_CONSUMER_TYPES` (the value-write intent allowlist). It
/// powers "where is this secret used" and delete-protection for named secrets.
pub const PLAYBOOK_VERSION_RUNTIME_ENV_CONSUMER_TYPE: &str = "playbook_version:runtime_env";
/// Consumer type projecting a playbook project's SCM credential into
/// `secret_usage_binding` (consumer_id = project id, field_key =
/// `scm_credential`, owner_scope = ('playbook_project', project_id)). Like the
/// playbook version runtime-env projection this is a USAGE projection only —
/// the secret value lives in the named secret itself — so it is intentionally
/// NOT part of `VALID_SECRET_FIELD_CONSUMER_TYPES`. It powers "where is this
/// secret used" and delete-protection for named secrets.
pub const PLAYBOOK_PROJECT_SCM_CREDENTIAL_CONSUMER_TYPE: &str = "playbook_project";
pub const PLAYBOOK_PROJECT_SCM_CREDENTIAL_FIELD_KEY: &str = "scm_credential";
/// Consumer type for secret-valued inventory custom fields. The binding
/// (consumer_id = the entity UUID, field_key = the custom-field name) is the
/// canonical marker that a secret custom-field value exists; the plaintext never
/// lands in the entity's `custom_field_data` JSONB.
pub const INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE: &str = "inventory_custom_field";

const VALID_SECRET_FIELD_CONSUMER_TYPES: &[&str] = &[
    CONNECTION_TARGET_SECRET_CONSUMER_TYPE,
    "oidc_settings",
    "sync_provider",
    ASSISTANT_INSTANCE_CONSUMER_TYPE,
    "user_auth_password",
    CONNECTION_CONFIG_RECORD_CONSUMER_TYPE,
    USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE,
    PLAYBOOK_SERVICE_PRINCIPAL_CREDENTIAL_CONSUMER_TYPE,
    PROXY_NODE_CREDENTIAL_CONSUMER_TYPE,
    INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE,
];

pub fn normalize_secret_field_consumer_type(raw: &str) -> Result<String, AppError> {
    let normalized = raw.trim();
    if normalized.is_empty() {
        return Err(AppError::Validation(
            "consumer_type is required".to_string(),
        ));
    }
    if !VALID_SECRET_FIELD_CONSUMER_TYPES.contains(&normalized) {
        // Name the accepted values: a bare "unsupported" leaves the caller with
        // no way to correct the request, and the common case is a near-miss on
        // a renamed type (`user_connection_credential` for
        // `user_connection_credential_set`), which the list resolves directly.
        return Err(AppError::Validation(format!(
            "Unsupported consumer_type '{normalized}'; expected one of: {}",
            VALID_SECRET_FIELD_CONSUMER_TYPES.join(", ")
        )));
    }
    Ok(normalized.to_string())
}

pub fn is_user_connection_credential_set_consumer_type(consumer_type: &str) -> bool {
    consumer_type == USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE
}

#[cfg(test)]
mod tests {
    use super::is_user_connection_credential_set_consumer_type;
    use super::normalize_secret_field_consumer_type;
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
    fn normalize_secret_field_consumer_type_rejects_empty_value() {
        let result = normalize_secret_field_consumer_type(" ");
        assert_validation_error_contains(result, "required");
    }

    #[test]
    fn normalize_secret_field_consumer_type_rejects_unknown_user_consumer() {
        let result = normalize_secret_field_consumer_type("unknown_user_consumer");
        assert_validation_error_contains(result, "Unsupported consumer_type");
    }

    #[test]
    fn normalize_secret_field_consumer_type_keeps_supported_value() {
        let result = normalize_secret_field_consumer_type("connection_scope:connection_target");
        assert!(
            result.is_ok(),
            "supported consumer type should pass: {result:?}"
        );
        let normalized = result.unwrap_or_default();
        assert_eq!(normalized, "connection_scope:connection_target");
        assert!(!is_user_connection_credential_set_consumer_type(
            normalized.as_str()
        ));
    }

    #[test]
    fn normalize_secret_field_consumer_type_keeps_connection_config_record() {
        let result = normalize_secret_field_consumer_type("connection_config:record");
        assert!(
            result.is_ok(),
            "connection_config:record should be supported: {result:?}"
        );
        assert_eq!(result.unwrap_or_default(), "connection_config:record");
    }

    #[test]
    fn normalize_secret_field_consumer_type_keeps_assistant_instance() {
        let result = normalize_secret_field_consumer_type("assistant_instance");
        assert!(
            result.is_ok(),
            "assistant_instance should be supported: {result:?}"
        );
        assert_eq!(result.unwrap_or_default(), "assistant_instance");
    }

    #[test]
    fn normalize_secret_field_consumer_type_keeps_playbook_service_principal_credential() {
        let result = normalize_secret_field_consumer_type("playbook_service_principal:credential");
        assert!(
            result.is_ok(),
            "playbook_service_principal:credential should be supported: {result:?}"
        );
        assert_eq!(
            result.unwrap_or_default(),
            "playbook_service_principal:credential"
        );
    }

    #[test]
    fn normalize_secret_field_consumer_type_keeps_proxy_node_credential() {
        let result = normalize_secret_field_consumer_type("proxy_node:credential");
        assert!(
            result.is_ok(),
            "proxy_node:credential should be supported: {result:?}"
        );
        assert_eq!(result.unwrap_or_default(), "proxy_node:credential");
    }

    #[test]
    fn normalize_secret_field_consumer_type_keeps_inventory_custom_field() {
        let result = normalize_secret_field_consumer_type("inventory_custom_field");
        assert!(
            result.is_ok(),
            "inventory_custom_field should be supported: {result:?}"
        );
        assert_eq!(result.unwrap_or_default(), "inventory_custom_field");
    }

    #[test]
    fn normalize_secret_field_consumer_type_rejects_unknown_value() {
        let result = normalize_secret_field_consumer_type("custom_consumer");
        assert_validation_error_contains(result, "Unsupported consumer_type");
    }

    #[test]
    fn identifies_user_connection_credential_set_consumer_type() {
        assert!(is_user_connection_credential_set_consumer_type(
            "user_connection_credential_set"
        ));
    }
}
