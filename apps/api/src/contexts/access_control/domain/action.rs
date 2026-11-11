use serde::{Deserialize, Serialize};

/// Action enum representing allowed actions in the system
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema, strum::EnumIter,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Action {
    /// Create a resource
    Create,
    /// Read/view a resource
    Read,
    /// Update/modify a resource
    Update,
    /// Delete a resource
    Delete,
    /// Connect to a connection target
    Connect,
    /// Observe a connection target through a read-only monitoring or visual surface
    Observe,
    /// Execute a command
    Execute,
    /// Run project controller code or native inventory in an authorized execution pool.
    ExecuteNative,
    /// Export a connection credential to project-controlled execution.
    DelegateCredentials,
    /// Read CCTV live view, channels, snapshots, and device identity.
    CctvView,
    /// Read CCTV recording state and archive metadata.
    CctvRecordings,
    /// Export recorded CCTV footage.
    CctvExport,
    /// Read and manage device user accounts (video recorders and Linux hosts
    /// alike) through the neutral device-users plane.
    DeviceUsers,
    /// Read and manage CCTV device settings and health.
    CctvSettings,
    /// Control CCTV pan/tilt/zoom.
    CctvPtz,
    /// Manage roles and permissions
    ManageRoles,
    /// Manage policies
    ManagePolicies,
    /// View audit logs
    ViewAudit,
    /// Read secret values
    Secret,
}

#[cfg(test)]
mod tests {
    use super::Action;
    use crate::contexts::access_control::domain::{
        Policy, PolicyDecision, PolicyEffect, ResourceType, evaluate_policies_with_metadata,
    };
    use serde_json::json;
    use uuid::Uuid;

    fn grant(action: Action, resource_type: ResourceType) -> Policy {
        Policy {
            id: Uuid::from_u128(1),
            name: "explicit grant".to_string(),
            description: None,
            effect: PolicyEffect::Allow,
            actions: vec![action],
            resource_types: vec![resource_type],
            constraints: None,
            subjects: Vec::new(),
            resources: vec!["*".to_string()],
            enabled: true,
        }
    }

    #[test]
    fn execution_capabilities_require_separate_grants_even_for_resource_owner() {
        for (old, required, resource) in [
            (
                Action::Connect,
                Action::DelegateCredentials,
                ResourceType::ConnectionTarget,
            ),
            (
                Action::Execute,
                Action::ExecuteNative,
                ResourceType::PlaybookJobTemplate,
            ),
        ] {
            let context =
                json!({"resource": {"owner_id": "operator"}, "subject": {"id": "operator"}});
            let old_grant = grant(old, resource.clone());
            let evaluate = |policies: &[Policy]| {
                evaluate_policies_with_metadata(
                    policies,
                    required.clone(),
                    resource.clone(),
                    Some("*"),
                    &context,
                )
                .decision
            };
            assert_eq!(evaluate(&[old_grant]), PolicyDecision::Deny);
            let explicit = grant(required.clone(), resource.clone());
            assert_eq!(
                evaluate(std::slice::from_ref(&explicit)),
                PolicyDecision::Allow
            );
            let mut revoked = explicit.clone();
            revoked.id = Uuid::from_u128(2);
            revoked.effect = PolicyEffect::Deny;
            assert_eq!(evaluate(&[explicit, revoked]), PolicyDecision::Deny);
            assert_eq!(evaluate(&[]), PolicyDecision::Deny);
        }
    }
}

impl Action {
    /// Canonical lowercase wire key used by PAT scopes and MCP metadata.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Read => "read",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Connect => "connect",
            Self::Observe => "observe",
            Self::Execute => "execute",
            Self::ExecuteNative => "execute_native",
            Self::DelegateCredentials => "delegate_credentials",
            Self::CctvView => "view",
            Self::CctvRecordings => "recordings",
            Self::CctvExport => "export",
            Self::DeviceUsers => "device_users",
            Self::CctvSettings => "settings",
            Self::CctvPtz => "ptz",
            Self::ManageRoles => "manage_roles",
            Self::ManagePolicies => "manage_policies",
            Self::ViewAudit => "view_audit",
            Self::Secret => "secret",
        }
    }

    /// CCTV namespaced permissions are valid only with `ResourceType::Cctv`.
    #[must_use]
    pub const fn is_cctv_permission(&self) -> bool {
        matches!(
            self,
            Self::CctvView
                | Self::CctvRecordings
                | Self::CctvExport
                | Self::CctvSettings
                | Self::CctvPtz
        )
    }

    /// Every action variant, used to enumerate the capability grid.
    pub const ALL: [Action; 19] = [
        Action::Create,
        Action::Read,
        Action::Update,
        Action::Delete,
        Action::Connect,
        Action::Observe,
        Action::Execute,
        Action::ExecuteNative,
        Action::DelegateCredentials,
        Action::CctvView,
        Action::CctvRecordings,
        Action::CctvExport,
        Action::DeviceUsers,
        Action::CctvSettings,
        Action::CctvPtz,
        Action::ManageRoles,
        Action::ManagePolicies,
        Action::ViewAudit,
        Action::Secret,
    ];
}
