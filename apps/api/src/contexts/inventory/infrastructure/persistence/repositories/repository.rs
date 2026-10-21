use uuid::Uuid;

use crate::contexts::access_control::domain::VisibilityPredicate;

use super::super::list_query::{CustomFieldListFilter, SortSpec};
use crate::contexts::inventory::domain::{
    Cluster, ClusterGroup, ClusterListView, ClusterType, Device, DeviceListView, DeviceRole,
    DeviceType, DeviceTypeListView, Manufacturer, Region, RegionListView, Site, SiteGroup,
    SiteListView, Tenant, TenantGroup, TenantListView, VirtualMachine, VirtualMachineListView,
};
use crate::errors::AppError;

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait TenantRepository: Send + Sync {
    async fn get_by_id(&self, tenant_id: Uuid) -> Result<Option<Tenant>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Tenant>, AppError>;
    /// Canonical hydrated list for the REST envelope. Returns `(rows, total)`
    /// where `total` is the filtered row count from a single `COUNT(*) OVER()`
    /// window. LEFT JOINs resolve FK names so the UI does not need a
    /// secondary lookup pass. `search` is a pre-formatted `%term%` pattern or
    /// `None` for no filter.
    async fn list_hydrated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        group_id: &[Uuid],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<TenantListView>, i64), AppError>;
    async fn create(&self, tenant: &Tenant) -> Result<(), AppError>;
    async fn update(&self, tenant: &Tenant) -> Result<(), AppError>;
    async fn delete(&self, tenant_id: Uuid) -> Result<(), AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait TenantGroupRepository: Send + Sync {
    async fn get_by_id(&self, tenant_group_id: Uuid) -> Result<Option<TenantGroup>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        parent_id: &[Uuid],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<TenantGroup>, i64), AppError>;
    async fn create(&self, tenant_group: &TenantGroup) -> Result<(), AppError>;
    async fn update(&self, tenant_group: &TenantGroup) -> Result<(), AppError>;
    async fn delete(&self, tenant_group_id: Uuid) -> Result<(), AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait SiteRepository: Send + Sync {
    async fn get_by_id(&self, site_id: Uuid) -> Result<Option<Site>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Site>, AppError>;
    async fn list_hydrated(
        &self,
        filter: SiteFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
    ) -> Result<(Vec<SiteListView>, i64), AppError>;
    async fn create(&self, site: &Site) -> Result<(), AppError>;
    async fn update(&self, site: &Site) -> Result<(), AppError>;
    async fn delete(&self, site_id: Uuid) -> Result<(), AppError>;
}

/// Site list filter. Multi-value FK fields are `Vec<Uuid>` where an empty vector
/// means "no filter" and a non-empty vector emits `col IN (…)`. `status` is a
/// multi-value string filter. `sort` carries an already-allowlist-validated
/// [`SortSpec`].
#[derive(Debug, Clone, Default)]
pub struct SiteFilter {
    pub ids: Option<Vec<Uuid>>,
    pub region_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub site_group_id: Vec<Uuid>,
    pub status: Vec<String>,
    pub custom_fields: CustomFieldListFilter,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait SiteGroupRepository: Send + Sync {
    async fn get_by_id(&self, group_id: Uuid) -> Result<Option<SiteGroup>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        parent_id: &[Uuid],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<SiteGroup>, i64), AppError>;
    async fn create(&self, group: &SiteGroup) -> Result<(), AppError>;
    async fn update(&self, group: &SiteGroup) -> Result<(), AppError>;
    async fn delete(&self, group_id: Uuid) -> Result<(), AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ClusterRepository: Send + Sync {
    async fn get_by_id(&self, cluster_id: Uuid) -> Result<Option<Cluster>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<Cluster>, i64), AppError>;
    /// Canonical hydrated list for the REST envelope. LEFT JOINs resolve the
    /// cluster type / group / site / tenant names so the UI does not need a
    /// secondary lookup pass.
    async fn list_hydrated(
        &self,
        filter: ClusterFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
    ) -> Result<(Vec<ClusterListView>, i64), AppError>;
    async fn create(&self, cluster: &Cluster) -> Result<(), AppError>;
    async fn update(&self, cluster: &Cluster) -> Result<(), AppError>;
    async fn delete(&self, cluster_id: Uuid) -> Result<(), AppError>;
}

/// Cluster list filter. Multi-value FK fields are `Vec<Uuid>` where an empty
/// vector means "no filter" and a non-empty vector emits `col IN (…)`.
/// `status` is a multi-value string filter. `sort` carries an
/// already-allowlist-validated [`SortSpec`].
#[derive(Debug, Clone, Default)]
pub struct ClusterFilter {
    pub ids: Option<Vec<Uuid>>,
    pub cluster_type_id: Vec<Uuid>,
    pub cluster_group_id: Vec<Uuid>,
    pub site_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub status: Vec<String>,
    pub custom_fields: CustomFieldListFilter,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait DeviceRoleRepository: Send + Sync {
    async fn get_by_id(&self, role_id: Uuid) -> Result<Option<DeviceRole>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        vm_role: &[String],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<DeviceRole>, i64), AppError>;
    async fn create(&self, role: &DeviceRole) -> Result<(), AppError>;
    async fn update(&self, role: &DeviceRole) -> Result<(), AppError>;
    async fn delete(&self, role_id: Uuid) -> Result<(), AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait DeviceRepository: Send + Sync {
    async fn get_by_id(&self, device_id: Uuid) -> Result<Option<Device>, AppError>;
    async fn list_paginated(
        &self,
        filter: DeviceFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Device>, AppError>;
    async fn list_paginated_projected(
        &self,
        filter: DeviceFilter,
        projection: DeviceListProjection,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Device>, AppError> {
        let _ = projection;
        self.list_paginated(filter, limit, offset).await
    }
    async fn list_hydrated(
        &self,
        filter: DeviceFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
    ) -> Result<(Vec<DeviceListView>, i64), AppError>;
    async fn create(&self, device: &Device) -> Result<(), AppError>;
    async fn update(&self, device: &Device) -> Result<(), AppError>;
    async fn delete(&self, device_id: Uuid) -> Result<(), AppError>;
}

/// Device list filter. Multi-value FK fields are `Vec<Uuid>` where an empty
/// vector means "no filter" and a non-empty vector emits `col IN (…)`.
/// `region_id` stays single-valued (subquery filter). `sort` carries an
/// already-allowlist-validated [`SortSpec`].
#[derive(Debug, Clone, Default)]
pub struct DeviceFilter {
    pub ids: Option<Vec<Uuid>>,
    pub tenant_id: Vec<Uuid>,
    pub site_id: Vec<Uuid>,
    pub region_id: Option<Uuid>,
    pub role_id: Vec<Uuid>,
    pub device_type_id: Vec<Uuid>,
    pub platform_id: Vec<Uuid>,
    pub cluster_id: Vec<Uuid>,
    pub rack_id: Vec<Uuid>,
    pub location_id: Vec<Uuid>,
    pub status: Vec<String>,
    pub custom_fields: CustomFieldListFilter,
    pub sort: Option<SortSpec>,
    /// Row-level connection-target visibility predicate, set by the
    /// REST list handler from the caller's `Read × CONNECTION_TARGET` policies.
    /// `None` = no row filtering. Applied by `list_hydrated` only (the human-
    /// facing REST list); internal/system list paths are unfiltered.
    pub visibility: Option<VisibilityPredicate>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceListProjection {
    pub include_status: bool,
    pub include_primary_ip4_id: bool,
    pub include_primary_ip6_id: bool,
}

impl DeviceListProjection {
    pub fn include_primary_ip(self) -> bool {
        self.include_primary_ip4_id || self.include_primary_ip6_id
    }
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait VirtualMachineRepository: Send + Sync {
    async fn get_by_id(&self, vm_id: Uuid) -> Result<Option<VirtualMachine>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<VirtualMachine>, AppError>;
    async fn list_paginated_projected(
        &self,
        projection: VirtualMachineListProjection,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<VirtualMachine>, AppError> {
        let _ = projection;
        self.list_paginated(limit, offset).await
    }
    async fn list_hydrated(
        &self,
        filter: VirtualMachineFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
    ) -> Result<(Vec<VirtualMachineListView>, i64), AppError>;
    async fn create(&self, vm: &VirtualMachine) -> Result<(), AppError>;
    async fn update(&self, vm: &VirtualMachine) -> Result<(), AppError>;
    async fn delete(&self, vm_id: Uuid) -> Result<(), AppError>;
}

/// Virtual-machine list filter. Multi-value FK fields are `Vec<Uuid>` where an
/// empty vector means "no filter" and a non-empty vector emits `col IN (…)`.
/// `status` is a multi-value string filter. `sort` carries an
/// already-allowlist-validated [`SortSpec`].
#[derive(Debug, Clone, Default)]
pub struct VirtualMachineFilter {
    pub ids: Option<Vec<Uuid>>,
    pub cluster_id: Vec<Uuid>,
    pub site_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub role_id: Vec<Uuid>,
    pub platform_id: Vec<Uuid>,
    pub virtual_machine_type_id: Vec<Uuid>,
    pub status: Vec<String>,
    pub sort: Option<SortSpec>,
    /// Row-level connection-target visibility predicate; see
    /// [`DeviceFilter::visibility`].
    pub visibility: Option<VisibilityPredicate>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VirtualMachineListProjection {
    pub include_status: bool,
    pub include_primary_ip4_id: bool,
    pub include_primary_ip6_id: bool,
}

impl VirtualMachineListProjection {
    pub fn include_primary_ip(self) -> bool {
        self.include_primary_ip4_id || self.include_primary_ip6_id
    }
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait RegionRepository: Send + Sync {
    async fn get_by_id(&self, region_id: Uuid) -> Result<Option<Region>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Region>, AppError>;
    async fn list_hydrated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        parent_id: &[Uuid],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<RegionListView>, i64), AppError>;
    async fn create(&self, region: &Region) -> Result<(), AppError>;
    async fn update(&self, region: &Region) -> Result<(), AppError>;
    async fn delete(&self, region_id: Uuid) -> Result<(), AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ManufacturerRepository: Send + Sync {
    async fn get_by_id(&self, manufacturer_id: Uuid) -> Result<Option<Manufacturer>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<Manufacturer>, i64), AppError>;
    async fn create(&self, manufacturer: &Manufacturer) -> Result<(), AppError>;
    async fn update(&self, manufacturer: &Manufacturer) -> Result<(), AppError>;
    async fn delete(&self, manufacturer_id: Uuid) -> Result<(), AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait DeviceTypeRepository: Send + Sync {
    async fn get_by_id(&self, device_type_id: Uuid) -> Result<Option<DeviceType>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<DeviceType>, i64), AppError>;
    /// Canonical hydrated list for the REST envelope. A LEFT JOIN resolves the
    /// manufacturer name so the UI does not need a secondary lookup pass.
    async fn list_hydrated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        manufacturer_id: &[Uuid],
        sort: Option<SortSpec>,
    ) -> Result<(Vec<DeviceTypeListView>, i64), AppError>;
    async fn create(&self, device_type: &DeviceType) -> Result<(), AppError>;
    async fn update(&self, device_type: &DeviceType) -> Result<(), AppError>;
    async fn delete(&self, device_type_id: Uuid) -> Result<(), AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ClusterTypeRepository: Send + Sync {
    async fn get_by_id(&self, cluster_type_id: Uuid) -> Result<Option<ClusterType>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ClusterType>, i64), AppError>;
    async fn create(&self, cluster_type: &ClusterType) -> Result<(), AppError>;
    async fn update(&self, cluster_type: &ClusterType) -> Result<(), AppError>;
    async fn delete(&self, cluster_type_id: Uuid) -> Result<(), AppError>;
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ClusterGroupRepository: Send + Sync {
    async fn get_by_id(&self, cluster_group_id: Uuid) -> Result<Option<ClusterGroup>, AppError>;
    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
        sort: Option<SortSpec>,
    ) -> Result<(Vec<ClusterGroup>, i64), AppError>;
    async fn create(&self, cluster_group: &ClusterGroup) -> Result<(), AppError>;
    async fn update(&self, cluster_group: &ClusterGroup) -> Result<(), AppError>;
    async fn delete(&self, cluster_group_id: Uuid) -> Result<(), AppError>;
}
