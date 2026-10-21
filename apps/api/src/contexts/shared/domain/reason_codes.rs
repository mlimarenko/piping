pub const AUTH_INVALID_CREDENTIALS: &str = "AUTH_INVALID_CREDENTIALS";
pub const AUTH_SESSION_EXPIRED: &str = "AUTH_SESSION_EXPIRED";
pub const AUTH_SESSION_REVOKED: &str = "AUTH_SESSION_REVOKED";
pub const AUTH_ACCOUNT_DISABLED: &str = "AUTH_ACCOUNT_DISABLED";

pub const SUBSCRIPTION_NOT_ACTIVE: &str = "SUBSCRIPTION_NOT_ACTIVE";
pub const SUBSCRIPTION_PAST_DUE: &str = "SUBSCRIPTION_PAST_DUE";
pub const SUBSCRIPTION_CANCELED: &str = "SUBSCRIPTION_CANCELED";
pub const SUBSCRIPTION_MUTATION_BLOCKED_BY_POLICY: &str = "SUBSCRIPTION_MUTATION_BLOCKED_BY_POLICY";

pub const SEAT_VERSION_CONFLICT: &str = "SEAT_VERSION_CONFLICT";
pub const SEAT_UPDATE_OUT_OF_RANGE: &str = "SEAT_UPDATE_OUT_OF_RANGE";
pub const SEAT_UPDATE_NOT_ALLOWED_FOR_ROLE: &str = "SEAT_UPDATE_NOT_ALLOWED_FOR_ROLE";

pub const TOKEN_NOT_FOUND: &str = "TOKEN_NOT_FOUND";
pub const TOKEN_REVOKED: &str = "TOKEN_REVOKED";
pub const TOKEN_EXPIRED: &str = "TOKEN_EXPIRED";
pub const TOKEN_ALREADY_CONSUMED: &str = "TOKEN_ALREADY_CONSUMED";

pub const ENTITLEMENT_EXPIRED: &str = "ENTITLEMENT_EXPIRED";
pub const ENTITLEMENT_SUSPENDED: &str = "ENTITLEMENT_SUSPENDED";
pub const ENTITLEMENT_SEAT_LIMIT_REACHED: &str = "ENTITLEMENT_SEAT_LIMIT_REACHED";
pub const ENTITLEMENT_BINDING_INVALID: &str = "ENTITLEMENT_BINDING_INVALID";

/// Control-plane-issued: one activation token was observed forking entitlement
/// versions across two live instances (single license reused on two copies).
/// The control plane owns detection; the app only mirrors the code so the
/// shared reason-code vocabulary stays consistent (this app never raises it).
pub const LICENSE_INSTANCE_CONFLICT: &str = "LICENSE_INSTANCE_CONFLICT";

pub const CONTENT_BLOCK_NOT_FOUND: &str = "CONTENT_BLOCK_NOT_FOUND";
pub const CONTENT_BLOCK_NOT_PUBLISHED: &str = "CONTENT_BLOCK_NOT_PUBLISHED";
pub const DELETE_BLOCKED: &str = "DELETE_BLOCKED";

pub const CONNECTION_CREDENTIALS_REQUIRED: &str = "CONNECTION_CREDENTIALS_REQUIRED";
pub const CONNECTION_AUTHENTICATION_FAILED: &str = "CONNECTION_AUTHENTICATION_FAILED";
pub const CONNECTION_TIMEOUT: &str = "CONNECTION_TIMEOUT";
pub const CONNECTION_UNREACHABLE: &str = "CONNECTION_UNREACHABLE";
pub const CONNECTION_BUSY: &str = "CONNECTION_BUSY";
pub const CONNECTION_UNSUPPORTED: &str = "CONNECTION_UNSUPPORTED";
pub const CONNECTION_FAILED: &str = "CONNECTION_FAILED";

/// Protocol-neutral reasons a connection attempt can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConnectionReasonCode {
    /// No credentials are configured for the target.
    ConnectionCredentialsRequired,
    /// The remote endpoint rejected the configured credentials.
    ConnectionAuthenticationFailed,
    /// The remote operation exceeded its typed deadline.
    ConnectionTimeout,
    /// The remote endpoint could not be reached.
    ConnectionUnreachable,
    /// A bounded connection or media resource is saturated.
    ConnectionBusy,
    /// The requested operation or target configuration is unsupported.
    ConnectionUnsupported,
    /// The remote endpoint failed in another way.
    ConnectionFailed,
}

impl ConnectionReasonCode {
    /// Return the canonical wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConnectionCredentialsRequired => CONNECTION_CREDENTIALS_REQUIRED,
            Self::ConnectionAuthenticationFailed => CONNECTION_AUTHENTICATION_FAILED,
            Self::ConnectionTimeout => CONNECTION_TIMEOUT,
            Self::ConnectionUnreachable => CONNECTION_UNREACHABLE,
            Self::ConnectionBusy => CONNECTION_BUSY,
            Self::ConnectionUnsupported => CONNECTION_UNSUPPORTED,
            Self::ConnectionFailed => CONNECTION_FAILED,
        }
    }
}

pub const CANONICAL_REASON_CODES: [&str; 30] = [
    AUTH_INVALID_CREDENTIALS,
    AUTH_SESSION_EXPIRED,
    AUTH_SESSION_REVOKED,
    AUTH_ACCOUNT_DISABLED,
    SUBSCRIPTION_NOT_ACTIVE,
    SUBSCRIPTION_PAST_DUE,
    SUBSCRIPTION_CANCELED,
    SUBSCRIPTION_MUTATION_BLOCKED_BY_POLICY,
    SEAT_VERSION_CONFLICT,
    SEAT_UPDATE_OUT_OF_RANGE,
    SEAT_UPDATE_NOT_ALLOWED_FOR_ROLE,
    TOKEN_NOT_FOUND,
    TOKEN_REVOKED,
    TOKEN_EXPIRED,
    TOKEN_ALREADY_CONSUMED,
    ENTITLEMENT_EXPIRED,
    ENTITLEMENT_SUSPENDED,
    ENTITLEMENT_SEAT_LIMIT_REACHED,
    ENTITLEMENT_BINDING_INVALID,
    LICENSE_INSTANCE_CONFLICT,
    CONTENT_BLOCK_NOT_FOUND,
    CONTENT_BLOCK_NOT_PUBLISHED,
    DELETE_BLOCKED,
    CONNECTION_CREDENTIALS_REQUIRED,
    CONNECTION_AUTHENTICATION_FAILED,
    CONNECTION_TIMEOUT,
    CONNECTION_UNREACHABLE,
    CONNECTION_BUSY,
    CONNECTION_UNSUPPORTED,
    CONNECTION_FAILED,
];

pub fn is_canonical_reason_code(value: &str) -> bool {
    CANONICAL_REASON_CODES.contains(&value)
}

#[cfg(test)]
mod tests {
    use super::{
        CONNECTION_AUTHENTICATION_FAILED, CONNECTION_BUSY, CONNECTION_CREDENTIALS_REQUIRED,
        CONNECTION_FAILED, CONNECTION_TIMEOUT, CONNECTION_UNREACHABLE, CONNECTION_UNSUPPORTED,
        ConnectionReasonCode, is_canonical_reason_code,
    };

    #[test]
    fn connection_reason_codes_have_exact_canonical_wire_values() {
        let cases = [
            (
                ConnectionReasonCode::ConnectionCredentialsRequired,
                CONNECTION_CREDENTIALS_REQUIRED,
            ),
            (
                ConnectionReasonCode::ConnectionAuthenticationFailed,
                CONNECTION_AUTHENTICATION_FAILED,
            ),
            (ConnectionReasonCode::ConnectionTimeout, CONNECTION_TIMEOUT),
            (
                ConnectionReasonCode::ConnectionUnreachable,
                CONNECTION_UNREACHABLE,
            ),
            (ConnectionReasonCode::ConnectionBusy, CONNECTION_BUSY),
            (
                ConnectionReasonCode::ConnectionUnsupported,
                CONNECTION_UNSUPPORTED,
            ),
            (ConnectionReasonCode::ConnectionFailed, CONNECTION_FAILED),
        ];

        for (reason, expected) in cases {
            assert_eq!(reason.as_str(), expected);
            assert_eq!(
                serde_json::to_value(reason).expect("reason code should serialize"),
                expected
            );
            assert!(is_canonical_reason_code(expected));
        }
    }
}
