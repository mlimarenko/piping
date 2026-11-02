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
    pub const ALL: [Action; 17] = [
        Action::Create,
        Action::Read,
        Action::Update,
        Action::Delete,
        Action::Connect,
        Action::Observe,
        Action::Execute,
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
