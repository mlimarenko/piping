//! Canonical identifiers for user-owned connection credentials stored in the unified secrets engine.
//!
//! These are *not* transport URLs. They are used as stable keys for `secret_usage_binding` and
//! server-side resolution at connection launch.

/// Secret usage binding consumer type for user-owned credential sets.
pub const USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE: &str = "user_connection_credential_set";

/// Owner scope type for user-owned connection credentials.
pub const USER_CONNECTION_CREDENTIAL_OWNER_SCOPE_TYPE: &str = "user";

/// Field key for a user's SSH password (if used).
pub const USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PASSWORD: &str = "ssh_password";

/// Field key for a user's SSH private key (PEM/OpenSSH).
pub const USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PRIVATE_KEY: &str = "ssh_private_key";

/// Field key for a user's RDP username.
pub const USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_USERNAME: &str = "rdp_username";

/// Field key for a user's RDP password.
pub const USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_PASSWORD: &str = "rdp_password";

/// Field key for a user's VNC password.
pub const USER_CONNECTION_CREDENTIAL_FIELD_KEY_VNC_PASSWORD: &str = "vnc_password";

pub fn is_user_connection_credential_field_key(field_key: &str) -> bool {
    matches!(
        field_key,
        USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PASSWORD
            | USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PRIVATE_KEY
            | USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_USERNAME
            | USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_PASSWORD
            | USER_CONNECTION_CREDENTIAL_FIELD_KEY_VNC_PASSWORD
    )
}
