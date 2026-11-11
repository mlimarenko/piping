//! Logical, directed network intent. Listener inventory does not supply ports.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::ObjectRef;
use crate::errors::AppError;

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
#[serde(rename_all = "snake_case")]
pub enum DataflowTransport {
    #[default]
    Any,
    Tcp,
    Udp,
    TcpUdp,
    Sctp,
    Icmp,
    Icmpv6,
}

impl DataflowTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::TcpUdp => "tcp_udp",
            Self::Sctp => "sctp",
            Self::Icmp => "icmp",
            Self::Icmpv6 => "icmpv6",
        }
    }
    pub fn is_icmp(self) -> bool {
        matches!(self, Self::Icmp | Self::Icmpv6)
    }
    /// `any` with explicit ports expands to the port-bearing protocols, as
    /// before; an unconstrained `any` stays unrestricted (including ICMP).
    pub fn expanded_with_ports(self, ports_constrained: bool) -> Vec<Self> {
        if self == Self::Any && ports_constrained {
            vec![Self::Tcp, Self::Udp, Self::Sctp]
        } else {
            vec![self]
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
    #[serde(rename = "ipam.iprange")]
    IpRange,
    #[serde(rename = "extras.tag")]
    Tag,
    #[serde(rename = "inventory.addressgroup")]
    AddressGroup,
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
        (Self::IpRange, "ipam.iprange", "iprange_id", "ipam_iprange"),
        (Self::Tag, "extras.tag", "tag_id", "inventory_tag"),
        (
            Self::AddressGroup,
            "inventory.addressgroup",
            "addressgroup_id",
            "inventory_address_group",
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
            && !matches!(
                self,
                Self::Service
                    | Self::IpAddress
                    | Self::Prefix
                    | Self::IpRange
                    | Self::Tag
                    | Self::AddressGroup
            )
    }
    pub fn is_collection(self) -> bool {
        self == Self::All || self.is_scope() || matches!(self, Self::Tag | Self::AddressGroup)
    }
    pub fn is_range(self) -> bool {
        self == Self::IpRange
    }
    pub fn is_tag(self) -> bool {
        self == Self::Tag
    }
    pub fn is_address_group(self) -> bool {
        self == Self::AddressGroup
    }
    /// Only devices and VMs can be named by a tag selector.
    pub fn is_tag_scope(self) -> bool {
        matches!(self, Self::Device | Self::VirtualMachine)
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
    Oob,
}
impl DataflowAddressMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Selected => "selected",
            Self::All => "all",
            Self::Oob => "oob",
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
    /// Present only for `extras.tag`: the scoped object type the tag selects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_object_type: Option<EndpointType>,
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
        if self.object_type.is_tag() {
            if !self.tag_object_type.is_some_and(EndpointType::is_tag_scope) {
                return Err(AppError::Validation(
                    "Tag selectors require tag_object_type dcim.device or virtualization.virtualmachine"
                        .into(),
                ));
            }
        } else if self.tag_object_type.is_some() {
            return Err(AppError::Validation(
                "tag_object_type is valid only for extras.tag selectors".into(),
            ));
        }
        if self.object_type.is_builtin() && self.address_mode != DataflowAddressMode::Default {
            return Err(AppError::Validation(
                "Builtin endpoints use the default address mode".into(),
            ));
        }
        if matches!(
            self.object_type,
            EndpointType::IpAddress
                | EndpointType::Prefix
                | EndpointType::IpRange
                | EndpointType::AddressGroup
        ) && self.address_mode != DataflowAddressMode::Default
        {
            return Err(AppError::Validation(
                "Explicit IP, Prefix, range and address-group endpoints have a fixed default address mode"
                    .into(),
            ));
        }
        if self.object_type.is_tag() && self.address_mode == DataflowAddressMode::Selected {
            return Err(AppError::Validation(
                "Tag selectors do not support selected addresses".into(),
            ));
        }
        if self.address_mode == DataflowAddressMode::Oob {
            let device_tags = self.object_type == EndpointType::Device
                || (self.object_type.is_tag()
                    && self.tag_object_type == Some(EndpointType::Device));
            if !device_tags {
                return Err(AppError::Validation(
                    "Out-of-band mode is valid only for devices and device tag selectors".into(),
                ));
            }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_object_type: Option<EndpointType>,
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
    pub comments: String,
    pub status: DataflowStatus,
    pub sources: Vec<DataflowEndpointInput>,
    pub destinations: Vec<DataflowEndpointInput>,
    pub transport: DataflowTransport,
    pub destination_ports: Option<Vec<i32>>,
    pub source_ports: Option<Vec<i32>>,
    pub icmp_types: Option<Vec<i32>>,
    pub allow_cross_vrf: bool,
    pub group_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_field_data: serde_json::Value,
    /// Effective state derived from the owning group ancestry, never stored.
    pub effective_status: DataflowStatus,
    pub blocked_by_group_id: Option<Uuid>,
    pub inherited_tags: Vec<Uuid>,
    pub effective_tags: Vec<Uuid>,
}

fn canonical_ports(ports: &mut Option<Vec<i32>>, label: &str) -> Result<(), AppError> {
    let Some(values) = ports else {
        return Ok(());
    };
    if values.is_empty()
        || values.len() > 256
        || values.iter().any(|port| !(1..=65535).contains(port))
    {
        return Err(AppError::Validation(format!(
            "{label} must contain 1..256 ports in 1..65535"
        )));
    }
    values.sort_unstable();
    values.dedup();
    Ok(())
}

fn canonical_tags(tags: &mut Vec<String>) -> Result<(), AppError> {
    for tag in tags.iter() {
        let parsed = Uuid::parse_str(tag)
            .map_err(|_| AppError::Validation("tags must be canonical UUID strings".into()))?;
        if parsed.to_string() != *tag {
            return Err(AppError::Validation(
                "tags must be canonical lowercase UUID strings".into(),
            ));
        }
    }
    tags.sort_unstable();
    tags.dedup();
    Ok(())
}

impl Dataflow {
    /// Validate intent, not reachability: draft flows may lack usable addresses.
    pub fn normalize(&mut self) -> Result<(), AppError> {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty()
            || self.name.len() > 200
            || self.description.len() > 4000
            || self.comments.len() > 4000
        {
            return Err(AppError::Validation(
                "name must contain 1..200 bytes; description and comments at most 4000 bytes"
                    .into(),
            ));
        }
        if self.sources.is_empty()
            || self.sources.len() > 32
            || self.destinations.is_empty()
            || self.destinations.len() > 32
        {
            return Err(AppError::Validation(
                "sources and destinations must contain 1..32 endpoints".into(),
            ));
        }
        for selector in self.sources.iter_mut().chain(self.destinations.iter_mut()) {
            selector.normalize()?;
        }
        if self.transport.is_icmp() {
            if self.destination_ports.is_some() || self.source_ports.is_some() {
                return Err(AppError::Validation(
                    "Ports are invalid for ICMP transports".into(),
                ));
            }
        } else if self.icmp_types.is_some() {
            return Err(AppError::Validation(
                "icmp_types is valid only for ICMP transports".into(),
            ));
        }
        canonical_ports(&mut self.destination_ports, "destination_ports")?;
        canonical_ports(&mut self.source_ports, "source_ports")?;
        if let Some(types) = &mut self.icmp_types {
            if types.is_empty()
                || types.len() > 256
                || types.iter().any(|value| !(0..=255).contains(value))
            {
                return Err(AppError::Validation(
                    "icmp_types must contain 1..256 unique values in 0..255".into(),
                ));
            }
            types.sort_unstable();
            types.dedup();
        }
        canonical_tags(&mut self.tags)?;
        if !self.custom_field_data.is_object() {
            return Err(AppError::Validation(
                "custom_field_data must be a JSON object".into(),
            ));
        }
        Ok(())
    }
}
