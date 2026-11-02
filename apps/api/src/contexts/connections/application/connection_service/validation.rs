use super::*;

pub(super) const ENDPOINT_REASON_UNRESOLVED: &str = "endpoint_unresolved";
pub(super) const ENDPOINT_REASON_NOT_BOUND_TO_TARGET: &str = "endpoint_not_bound_to_target";
pub(super) const ENDPOINT_REASON_UNSUPPORTED_PLACEHOLDER: &str = "endpoint_unsupported_placeholder";

pub(super) fn endpoint_validation_error(reason_code: &str, message: impl Into<String>) -> AppError {
    AppError::Validation(format!("{reason_code}: {}", message.into()))
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
    pub(super) fn ensure_transition(
        current: &ConnectionStatus,
        next: &ConnectionStatus,
    ) -> Result<(), AppError> {
        if current.can_transition_to(next) {
            Ok(())
        } else {
            Err(AppError::Validation(format!(
                "Invalid session status transition: {:?} -> {:?}",
                current, next
            )))
        }
    }

    pub(super) fn validate_entry_driven_overrides(
        protocol: &ConnectionProtocol,
        protocol_params: Option<ConnectionLaunchParams>,
    ) -> Result<Option<ConnectionLaunchParams>, AppError> {
        // Constitution 3.2: when launching via a selected host connection entry, the server-side
        // materialized config is the canonical source for all non-credential parameters.
        //
        // Client-provided overrides are allowed only for credentials (e.g. prompted username/
        // password when the selected entry has no creds). RDP may additionally override
        // deterministic runtime display controls through the canonical `options` object.
        let Some(params) = protocol_params else {
            return Ok(None);
        };
        let mut map = params;
        if map.is_empty() {
            return Ok(None);
        }

        let credentials_value = map.remove("credentials");
        let options_value = map.remove("options");
        if !map.is_empty() {
            return Err(AppError::Validation(
                "Only credentials and RDP options overrides are allowed when launching via entryId"
                    .to_string(),
            ));
        }

        let mut normalized = map;
        if let Some(credentials_value) = credentials_value {
            let Value::Object(credentials) = credentials_value else {
                return Err(AppError::Validation(
                    "Connection credentials must be a JSON object".to_string(),
                ));
            };

            let allowed: &[&str] = match protocol {
                ConnectionProtocol::Rdp => &["username", "password", "domain"],
                ConnectionProtocol::Ssh => &["username", "password", "private_key"],
                ConnectionProtocol::Vnc => &["password"],
                ConnectionProtocol::App | ConnectionProtocol::Cctv => &[],
            };
            let normalized_credentials = Self::normalize_credentials_map(protocol, credentials)?;
            for key in normalized_credentials.keys() {
                if !allowed.contains(&key.as_str()) {
                    return Err(AppError::Validation(format!(
                        "Unsupported credential override key '{}'",
                        key
                    )));
                }
            }

            normalized.insert(
                "credentials".to_string(),
                Value::Object(normalized_credentials),
            );
        }

        if let Some(options_value) = options_value {
            if *protocol != ConnectionProtocol::Rdp {
                return Err(AppError::Validation(
                    "Connection options overrides are supported only for RDP".to_string(),
                ));
            }
            let Value::Object(options) = options_value else {
                return Err(AppError::Validation(
                    "Connection options must be a JSON object".to_string(),
                ));
            };
            let normalized_options = Self::normalize_options_map(protocol, options)?;
            if !normalized_options.is_empty() {
                normalized.insert("options".to_string(), Value::Object(normalized_options));
            }
        }

        if normalized.is_empty() {
            return Ok(None);
        }
        Ok(Some(normalized))
    }
}
