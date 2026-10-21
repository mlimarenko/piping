//! Logical, directed network intent. Listener inventory does not supply ports.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::ObjectRef;
use crate::errors::AppError;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DataflowStatus {
    #[default]
    Draft,
    Active,
    Disabled,
}

impl DataflowStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum DataflowTransport {
    #[default]
    Any,
    Tcp,
    Udp,
    Sctp,
}

impl DataflowTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Sctp => "sctp",
        }
    }
}

/// Typed endpoint identity, independent of its current address resolution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DataflowEndpoint {
    pub object_type: EndpointType,
    pub object_id: Uuid,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema,
)]
pub enum EndpointType {
    #[serde(rename = "dcim.device")]
    Device,
    #[serde(rename = "virtualization.virtualmachine")]
    VirtualMachine,
    #[serde(rename = "ipam.service")]
    Service,
    #[serde(rename = "ipam.ipaddress")]
    IpAddress,
    #[serde(rename = "ipam.prefix")]
    Prefix,
    #[serde(rename = "dcim.region")]
    Region,
    #[serde(rename = "dcim.site")]
    Site,
    #[serde(rename = "dcim.sitegroup")]
    SiteGroup,
    #[serde(rename = "dcim.devicerole")]
    DeviceRole,
    #[serde(rename = "tenancy.tenant")]
    Tenant,
    #[serde(rename = "tenancy.tenantgroup")]
    TenantGroup,
    #[serde(rename = "dcim.platform")]
    Platform,
    #[serde(rename = "dcim.manufacturer")]
    Manufacturer,
    #[serde(rename = "dcim.devicetype")]
    DeviceType,
    #[serde(rename = "dcim.rack")]
    Rack,
    #[serde(rename = "dcim.location")]
    Location,
    #[serde(rename = "virtualization.cluster")]
    Cluster,
    #[serde(rename = "virtualization.clustergroup")]
    ClusterGroup,
    #[serde(rename = "virtualization.clustertype")]
    ClusterType,
    #[serde(rename = "all")]
    All,
    #[serde(rename = "internet")]
    Internet,
}

impl EndpointType {
    /// Canonical registry for every strong endpoint reference, including scopes.
    pub const REFERENCES: &'static [(Self, &'static str, &'static str, &'static str)] = &[
        (Self::Device, "dcim.device", "device_id", "dcim_device"),
        (
            Self::VirtualMachine,
            "virtualization.virtualmachine",
            "vm_id",
            "virtualization_virtualmachine",
        ),
        (Self::Service, "ipam.service", "service_id", "ipam_service"),
        (
            Self::IpAddress,
            "ipam.ipaddress",
            "ipaddress_id",
            "ipam_ipaddress",
        ),
        (Self::Prefix, "ipam.prefix", "prefix_id", "ipam_prefix"),
        (Self::Region, "dcim.region", "region_id", "dcim_region"),
        (Self::Site, "dcim.site", "site_id", "dcim_site"),
        (
            Self::SiteGroup,
            "dcim.sitegroup",
            "sitegroup_id",
            "dcim_sitegroup",
        ),
        (
            Self::DeviceRole,
            "dcim.devicerole",
            "devicerole_id",
            "dcim_devicerole",
        ),
        (
            Self::Tenant,
            "tenancy.tenant",
            "tenant_id",
            "tenancy_tenant",
        ),
        (
            Self::TenantGroup,
            "tenancy.tenantgroup",
            "tenantgroup_id",
            "tenancy_tenantgroup",
        ),
        (
            Self::Platform,
            "dcim.platform",
            "platform_id",
            "dcim_platform",
        ),
        (
            Self::Manufacturer,
            "dcim.manufacturer",
            "manufacturer_id",
            "dcim_manufacturer",
        ),
        (
            Self::DeviceType,
            "dcim.devicetype",
            "devicetype_id",
            "dcim_devicetype",
        ),
        (Self::Rack, "dcim.rack", "rack_id", "dcim_rack"),
        (
            Self::Location,
            "dcim.location",
            "location_id",
            "dcim_location",
        ),
        (
            Self::Cluster,
            "virtualization.cluster",
            "cluster_id",
            "virtualization_cluster",
        ),
        (
            Self::ClusterGroup,
            "virtualization.clustergroup",
            "clustergroup_id",
            "virtualization_clustergroup",
        ),
        (
            Self::ClusterType,
            "virtualization.clustertype",
            "clustertype_id",
            "virtualization_clustertype",
        ),
    ];
    pub fn parse(value: &str) -> Result<Self, AppError> {
        if value == "all" {
            return Ok(Self::All);
        }
        if value == "internet" {
            return Ok(Self::Internet);
        }
        Self::REFERENCES
            .iter()
            .find(|entry| entry.1 == value)
            .map(|entry| entry.0)
            .ok_or_else(|| AppError::Validation("Unsupported dataflow endpoint object_type".into()))
    }
    pub fn as_str(self) -> &'static str {
        if self == Self::All {
            return "all";
        }
        if self == Self::Internet {
            return "internet";
        }
        Self::REFERENCES
            .iter()
            .find(|entry| entry.0 == self)
            .map_or("", |entry| entry.1)
    }
    pub fn column(self) -> &'static str {
        Self::REFERENCES
            .iter()
            .find(|entry| entry.0 == self)
            .map_or("", |entry| entry.2)
    }
    pub fn is_host(self) -> bool {
        matches!(self, Self::Device | Self::VirtualMachine)
    }
    pub fn is_builtin(self) -> bool {
        matches!(self, Self::All | Self::Internet)
    }
    pub fn is_scope(self) -> bool {
        !self.is_host()
            && !self.is_builtin()
            && !matches!(self, Self::Service | Self::IpAddress | Self::Prefix)
    }
    pub fn is_collection(self) -> bool {
        self == Self::All || self.is_scope()
    }
    pub fn is_hierarchical(self) -> bool {
        matches!(
            self,
            Self::Region | Self::SiteGroup | Self::DeviceRole | Self::TenantGroup | Self::Location
        )
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum DataflowAddressMode {
    #[default]
    Default,
    Selected,
    All,
}
impl DataflowAddressMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Selected => "selected",
            Self::All => "all",
        }
    }
}

/// Flow-owned selector; the global inventory ObjectRef contract stays unchanged.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, utoipa::ToSchema,
)]
#[serde(deny_unknown_fields)]
pub struct DataflowEndpointInput {
    pub object_type: EndpointType,
    pub object_id: Option<Uuid>,
    #[serde(default)]
    #[schema(default = DataflowAddressMode::default)]
    pub address_mode: DataflowAddressMode,
    #[serde(default)]
    pub ipaddresses: Vec<Uuid>,
}
impl DataflowEndpointInput {
    pub fn identity(&self) -> Option<DataflowEndpoint> {
        self.object_id.map(|object_id| DataflowEndpoint {
            object_type: self.object_type,
            object_id,
        })
    }
    pub fn normalize(&mut self) -> Result<(), AppError> {
        if self.object_type.is_builtin() != self.object_id.is_none() {
            return Err(AppError::Validation(
                "Builtin endpoints have no object_id; inventory endpoints require one".into(),
            ));
        }
        if self.object_type.is_builtin() && self.address_mode != DataflowAddressMode::Default {
            return Err(AppError::Validation(
                "Builtin endpoints use the default address mode".into(),
            ));
        }
        if matches!(
            self.object_type,
            EndpointType::IpAddress | EndpointType::Prefix
        ) && self.address_mode != DataflowAddressMode::Default
        {
            return Err(AppError::Validation(
                "Explicit IP and Prefix endpoints have a fixed default address mode".into(),
            ));
        }
        if (self.address_mode == DataflowAddressMode::Selected) != !self.ipaddresses.is_empty()
            || self.ipaddresses.len() > 32
        {
            return Err(AppError::Validation(
                "selected requires 1..32 ipaddresses; other modes require an empty list".into(),
            ));
        }
        self.ipaddresses.sort_unstable();
        self.ipaddresses.dedup();
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DataflowEndpointResponse {
    pub object_type: EndpointType,
    pub object_id: Option<Uuid>,
    pub address_mode: DataflowAddressMode,
    pub ipaddresses: Vec<Uuid>,
    pub label: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataflowEndpointAddressResponse {
    pub id: Uuid,
    pub address: String,
    pub vrf_id: Option<Uuid>,
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataflowEndpointResolutionResponse {
    pub endpoint: DataflowEndpointResponse,
    pub members: Vec<DataflowEndpointMemberResponse>,
    pub ipaddresses: Vec<DataflowEndpointAddressResponse>,
    pub default_ipaddresses: Vec<Uuid>,
    pub addresses: Vec<DataflowResolvedAddressResponse>,
    pub diagnostics: Vec<DataflowResolutionDiagnostic>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
pub struct DataflowResolvedAddressResponse {
    pub cidr: String,
    pub vrf_id: Option<Uuid>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
pub struct DataflowResolutionDiagnostic {
    pub code: String,
    pub message: String,
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataflowEndpointMemberResponse {
    pub host: ObjectRef,
    pub addresses: Vec<DataflowResolvedAddressResponse>,
    pub diagnostics: Vec<DataflowResolutionDiagnostic>,
}

impl DataflowEndpoint {
    pub fn projection(&self) -> ObjectRef {
        ObjectRef {
            object_type: self.object_type.as_str().into(),
            object_id: self.object_id,
            label: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Dataflow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub status: DataflowStatus,
    pub source: DataflowEndpointInput,
    pub destination: DataflowEndpointInput,
    pub transport: DataflowTransport,
    pub destination_ports: Option<Vec<i32>>,
    pub allow_cross_vrf: bool,
}

impl Dataflow {
    /// Validate intent, not reachability: draft flows may lack usable addresses.
    pub fn normalize(&mut self) -> Result<(), AppError> {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() || self.name.len() > 200 || self.description.len() > 4000 {
            return Err(AppError::Validation(
                "name must contain 1..200 bytes; description at most 4000 bytes".into(),
            ));
        }
        self.source.normalize()?;
        self.destination.normalize()?;
        let Some(ports) = &mut self.destination_ports else {
            return Ok(());
        };
        if ports.is_empty()
            || ports.len() > 256
            || ports.iter().any(|port| !(1..=65535).contains(port))
        {
            return Err(AppError::Validation(
                "destination_ports must contain 1..256 ports in 1..65535".into(),
            ));
        }
        ports.sort_unstable();
        ports.dedup();
        Ok(())
    }
}
