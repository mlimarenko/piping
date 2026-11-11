//! Connection sessions, templates, proxy nodes and credential-set domain models and their public re-exports.
pub mod connection_session;
pub mod connection_status;
pub mod connection_template;
pub mod proxy_node;
pub mod proxy_node_assignment;
pub mod rdp_credentials;
pub mod routing_source;
pub mod user_connection_credentials;
pub mod user_credential_set;

pub use connection_session::ConnectionSession;
pub use connection_status::{
    ConnectionStatus, ConnectionStatusParseError, parse_connection_status,
    parse_optional_connection_status,
};
pub use connection_template::{
    ConnectionLaunchParams, ConnectionProtocol, ConnectionProtocolParseError, ConnectionTemplate,
    parse_connection_protocol, parse_optional_connection_protocol,
};
pub use proxy_node::{ProxyNode, ProxyNodeValidationError, validate_proxy_base_url};
pub use proxy_node_assignment::{
    NewProxyNodeAssignment, ProxyNodeAssignment, TargetProxyAssignment, UpdateProxyNodeAssignment,
};
pub use rdp_credentials::normalize_rdp_username_domain;
pub use routing_source::{RoutingSource, RoutingSourceParseError, parse_routing_source};
pub use user_connection_credentials::{
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_PASSWORD,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_RDP_USERNAME,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PASSWORD,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_SSH_PRIVATE_KEY,
    USER_CONNECTION_CREDENTIAL_FIELD_KEY_VNC_PASSWORD, USER_CONNECTION_CREDENTIAL_OWNER_SCOPE_TYPE,
    USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE,
};
pub use user_credential_set::UserCredentialSet;
