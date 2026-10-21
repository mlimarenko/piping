//! Access-control domain models, value objects and policy evaluation.
pub mod action;
pub mod auth_session;
pub mod conditions;
pub mod credential_provisioning;
pub mod group;
pub mod oidc_settings;
pub mod policy;
pub mod policy_evaluation;
pub mod principal;
pub mod resource;
pub mod resource_id;
pub mod role;
pub mod role_assignment;
pub mod user;
pub mod user_api_token;
pub mod user_credential;
pub mod visibility;
pub mod vocabulary;

pub use action::Action;
pub use auth_session::AuthSession;
pub use conditions::{
    ConditionExpression, constraint_attribute_keys_used, evaluate_constraints, parse_constraints,
    validate_constraints,
};
pub use credential_provisioning::{CredentialProvisioningCommand, CredentialProvisioningMode};
pub use group::Group;
pub use oidc_settings::{
    OIDC_CLIENT_SECRET_CONSUMER_TYPE, OIDC_CLIENT_SECRET_FIELD_KEY,
    OIDC_CLIENT_SECRET_OWNER_SCOPE_TYPE, OidcClientSecretState, OidcProvisioningMode, OidcSettings,
    oidc_client_secret_consumer_id, oidc_client_secret_owner_scope_id,
};
pub use policy::{Policy, PolicyEffect};
pub use policy_evaluation::{
    PolicyDecision, PolicyDecisionMetadata, evaluate_policies, evaluate_policies_with_metadata,
};
pub use principal::{Principal, ServicePrincipalId};
pub use resource::{
    ResourceType, matches_resource_id, matches_resource_pattern, resource_pattern_is_valid,
};
pub use resource_id::{ResourceId, ResourceIdError};
pub use role::Role;
pub use role_assignment::RoleAssignment;
pub use user::User;
pub use user_api_token::{UserApiToken, UserApiTokenScope, UserApiTokenStatus};
pub use user_credential::UserCredential;
pub use visibility::{Facet, RowFacets, VisibilityPredicate, compile_connection_target_visibility};
pub use vocabulary::{is_valid_permission_pair, pat_scope_wire_value};
