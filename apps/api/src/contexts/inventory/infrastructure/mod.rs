//! Inventory caching and persistence infrastructure modules with their public re-exports.
pub mod caching;
pub mod persistence;

pub use caching::HostConfigCache;
pub use persistence::{
    ClusterRepository, ConfigContextBindingListFilters, ConfigContextRepository, DeviceRepository,
    DeviceRoleRepository, SiteRepository, SqlxClusterRepository, SqlxConfigContextRepository,
    SqlxDeviceRepository, SqlxDeviceRoleRepository, SqlxSiteRepository, SqlxTenantGroupRepository,
    SqlxTenantRepository, SqlxVirtualMachineRepository, TenantGroupRepository, TenantRepository,
    VirtualMachineRepository,
};
