use serde::{Deserialize, Serialize};

/// Protocol kind a connection-config record is typed for.
///
/// Mirrors the `connconf_record_protocol_check` constraint in the consolidated
/// `0001_init.sql` baseline (which includes the `cctv` value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnConfProtocol {
    Ssh,
    Rdp,
    Vnc,
    App,
    Cctv,
}

impl ConnConfProtocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
            Self::App => "app",
            Self::Cctv => "cctv",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "ssh" => Some(Self::Ssh),
            "rdp" => Some(Self::Rdp),
            "vnc" => Some(Self::Vnc),
            "app" => Some(Self::App),
            "cctv" => Some(Self::Cctv),
            _ => None,
        }
    }
}

/// The 12 ordered scope layers (plus `Global`) an assignment can bind to.
///
/// Order is least-specific → most-specific and supersets both the legacy
/// profile model (6 layers) and the config-context model (8 layers). The
/// numeric [`ConnConfScope::specificity`] index drives deepest-wins merge in
/// the resolver, exactly like `config_resolver::scope_precedence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnConfScope {
    Global,
    Region,
    SiteGroup,
    Site,
    Location,
    Rack,
    Tenant,
    Cluster,
    Role,
    DeviceType,
    Platform,
    Device,
    Vm,
}

impl ConnConfScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Region => "region",
            Self::SiteGroup => "site_group",
            Self::Site => "site",
            Self::Location => "location",
            Self::Rack => "rack",
            Self::Tenant => "tenant",
            Self::Cluster => "cluster",
            Self::Role => "role",
            Self::DeviceType => "device_type",
            Self::Platform => "platform",
            Self::Device => "device",
            Self::Vm => "vm",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "global" => Some(Self::Global),
            "region" => Some(Self::Region),
            "site_group" => Some(Self::SiteGroup),
            "site" => Some(Self::Site),
            "location" => Some(Self::Location),
            "rack" => Some(Self::Rack),
            "tenant" => Some(Self::Tenant),
            "cluster" => Some(Self::Cluster),
            "role" => Some(Self::Role),
            "device_type" => Some(Self::DeviceType),
            "platform" => Some(Self::Platform),
            "device" => Some(Self::Device),
            "vm" => Some(Self::Vm),
            _ => None,
        }
    }

    /// Ordering from least specific (0) to most specific.
    /// Used by the resolver to merge deepest-wins.
    pub const fn specificity(self) -> u8 {
        match self {
            Self::Global => 0,
            Self::Region => 1,
            Self::SiteGroup => 2,
            Self::Site => 3,
            Self::Location => 4,
            Self::Rack => 5,
            Self::Tenant => 6,
            Self::Cluster => 7,
            Self::Role => 8,
            Self::DeviceType => 9,
            Self::Platform => 10,
            Self::Device => 11,
            Self::Vm => 12,
        }
    }
}
