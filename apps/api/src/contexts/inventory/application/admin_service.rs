use std::sync::Arc;

use uuid::Uuid;

use crate::contexts::inventory::domain::{
    Cluster, ClusterGroup, ClusterListView, ClusterType, Device, DeviceListView, DeviceRole,
    DeviceType, DeviceTypeListView, Manufacturer, Region, RegionListView, Site, SiteGroup,
    SiteListView, Tenant, TenantGroup, TenantListView, VirtualMachine, VirtualMachineListView,
};
use crate::contexts::inventory::infrastructure::persistence::{
    ClusterFilter, ClusterGroupRepository, ClusterRepository, ClusterTypeRepository, DeviceFilter,
    DeviceRepository, DeviceRoleRepository, DeviceTypeRepository, ManufacturerRepository,
    RegionRepository, SiteFilter, SiteGroupRepository, SiteRepository, SortSpec,
    TenantGroupRepository, TenantRepository, VirtualMachineFilter, VirtualMachineRepository,
};
use crate::errors::AppError;

#[derive(Debug, Clone, Default)]
pub struct InventoryListQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Normalised search token: already trimmed and wrapped in `%…%`, or `None`
    /// when no filter should be applied.
    pub search: Option<String>,
    /// Allowlist-validated sort directive, or `None` to keep the default order.
    pub sort: Option<SortSpec>,
    pub parent_id: Vec<Uuid>,
    pub group_id: Vec<Uuid>,
    pub manufacturer_id: Vec<Uuid>,
    pub vm_role: Vec<String>,
}

impl InventoryListQuery {
    pub fn new(limit: Option<u32>, offset: Option<u32>, search: Option<String>) -> Self {
        Self {
            limit,
            offset,
            search,
            sort: None,
            parent_id: Vec::new(),
            group_id: Vec::new(),
            manufacturer_id: Vec::new(),
            vm_role: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_sort(mut self, sort: Option<SortSpec>) -> Self {
        self.sort = sort;
        self
    }

    #[must_use]
    pub fn with_parent_id(mut self, parent_id: Vec<Uuid>) -> Self {
        self.parent_id = parent_id;
        self
    }

    #[must_use]
    pub fn with_group_id(mut self, group_id: Vec<Uuid>) -> Self {
        self.group_id = group_id;
        self
    }

    #[must_use]
    pub fn with_manufacturer_id(mut self, manufacturer_id: Vec<Uuid>) -> Self {
        self.manufacturer_id = manufacturer_id;
        self
    }

    #[must_use]
    pub fn with_vm_role(mut self, vm_role: Vec<String>) -> Self {
        self.vm_role = vm_role;
        self
    }
}

#[derive(Debug, Clone)]
pub struct InventoryDeviceListQuery {
    pub filter: DeviceFilter,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Normalised search token: already trimmed and wrapped in `%…%`, or `None`
    /// when no filter should be applied.
    pub search: Option<String>,
}

impl InventoryDeviceListQuery {
    pub fn new(filter: DeviceFilter, limit: Option<u32>, offset: Option<u32>) -> Self {
        Self {
            filter,
            limit,
            offset,
            search: None,
        }
    }

    pub fn with_search(mut self, search: Option<String>) -> Self {
        self.search = search;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct InventoryVirtualMachineListQuery {
    pub filter: VirtualMachineFilter,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Normalised search token: already trimmed and wrapped in `%…%`, or `None`
    /// when no filter should be applied.
    pub search: Option<String>,
}

impl InventoryVirtualMachineListQuery {
    pub fn new(filter: VirtualMachineFilter, limit: Option<u32>, offset: Option<u32>) -> Self {
        Self {
            filter,
            limit,
            offset,
            search: None,
        }
    }

    pub fn with_search(mut self, search: Option<String>) -> Self {
        self.search = search;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct InventorySiteListQuery {
    pub filter: SiteFilter,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Normalised search token: already trimmed and wrapped in `%…%`, or `None`
    /// when no filter should be applied.
    pub search: Option<String>,
}

impl InventorySiteListQuery {
    pub fn new(filter: SiteFilter, limit: Option<u32>, offset: Option<u32>) -> Self {
        Self {
            filter,
            limit,
            offset,
            search: None,
        }
    }

    pub fn with_search(mut self, search: Option<String>) -> Self {
        self.search = search;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct InventoryClusterListQuery {
    pub filter: ClusterFilter,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Normalised search token: already trimmed and wrapped in `%…%`, or `None`
    /// when no filter should be applied.
    pub search: Option<String>,
}

impl InventoryClusterListQuery {
    pub fn new(filter: ClusterFilter, limit: Option<u32>, offset: Option<u32>) -> Self {
        Self {
            filter,
            limit,
            offset,
            search: None,
        }
    }

    pub fn with_search(mut self, search: Option<String>) -> Self {
        self.search = search;
        self
    }
}

pub struct InventoryAdminRepositories<
    TGRepo,
    TRepo,
    SRepo,
    SGRepo,
    RRepo,
    CRepo,
    CTRepo,
    CGRepo,
    DRRepo,
    DRepo,
    MRepo,
    DTRepo,
    VMRepo,
> where
    TGRepo: TenantGroupRepository,
    TRepo: TenantRepository,
    SRepo: SiteRepository,
    SGRepo: SiteGroupRepository,
    RRepo: RegionRepository,
    CRepo: ClusterRepository,
    CTRepo: ClusterTypeRepository,
    CGRepo: ClusterGroupRepository,
    DRRepo: DeviceRoleRepository,
    DRepo: DeviceRepository,
    MRepo: ManufacturerRepository,
    DTRepo: DeviceTypeRepository,
    VMRepo: VirtualMachineRepository,
{
    pub tenant_groups: Arc<TGRepo>,
    pub tenants: Arc<TRepo>,
    pub sites: Arc<SRepo>,
    pub site_groups: Arc<SGRepo>,
    pub regions: Arc<RRepo>,
    pub clusters: Arc<CRepo>,
    pub cluster_types: Arc<CTRepo>,
    pub cluster_groups: Arc<CGRepo>,
    pub device_roles: Arc<DRRepo>,
    pub devices: Arc<DRepo>,
    pub manufacturers: Arc<MRepo>,
    pub device_types: Arc<DTRepo>,
    pub virtual_machines: Arc<VMRepo>,
}

/// The thirteen inventory repositories the admin service depends on.
///
/// The service is generic over this one bundle instead of over thirteen
/// repository parameters: a thirteen-argument type in a signature scores past
/// `clippy::type_complexity`, and one bundle bound replaces the thirteen
/// repeated trait bounds. [`InventoryAdminRepositories`] is the production
/// implementation.
pub trait InventoryAdminRepositoryBundle {
    type TenantGroupRepo: TenantGroupRepository;
    type TenantRepo: TenantRepository;
    type SiteRepo: SiteRepository;
    type SiteGroupRepo: SiteGroupRepository;
    type RegionRepo: RegionRepository;
    type ClusterRepo: ClusterRepository;
    type ClusterTypeRepo: ClusterTypeRepository;
    type ClusterGroupRepo: ClusterGroupRepository;
    type DeviceRoleRepo: DeviceRoleRepository;
    type DeviceRepo: DeviceRepository;
    type ManufacturerRepo: ManufacturerRepository;
    type DeviceTypeRepo: DeviceTypeRepository;
    type VirtualMachineRepo: VirtualMachineRepository;

    fn tenant_group_repos(&self) -> Arc<Self::TenantGroupRepo>;
    fn tenant_repos(&self) -> Arc<Self::TenantRepo>;
    fn site_repos(&self) -> Arc<Self::SiteRepo>;
    fn site_group_repos(&self) -> Arc<Self::SiteGroupRepo>;
    fn region_repos(&self) -> Arc<Self::RegionRepo>;
    fn cluster_repos(&self) -> Arc<Self::ClusterRepo>;
    fn cluster_type_repos(&self) -> Arc<Self::ClusterTypeRepo>;
    fn cluster_group_repos(&self) -> Arc<Self::ClusterGroupRepo>;
    fn device_role_repos(&self) -> Arc<Self::DeviceRoleRepo>;
    fn device_repos(&self) -> Arc<Self::DeviceRepo>;
    fn manufacturer_repos(&self) -> Arc<Self::ManufacturerRepo>;
    fn device_type_repos(&self) -> Arc<Self::DeviceTypeRepo>;
    fn virtual_machine_repos(&self) -> Arc<Self::VirtualMachineRepo>;
}

impl<
    TGRepo,
    TRepo,
    SRepo,
    SGRepo,
    RRepo,
    CRepo,
    CTRepo,
    CGRepo,
    DRRepo,
    DRepo,
    MRepo,
    DTRepo,
    VMRepo,
> InventoryAdminRepositoryBundle
    for InventoryAdminRepositories<
        TGRepo,
        TRepo,
        SRepo,
        SGRepo,
        RRepo,
        CRepo,
        CTRepo,
        CGRepo,
        DRRepo,
        DRepo,
        MRepo,
        DTRepo,
        VMRepo,
    >
where
    TGRepo: TenantGroupRepository,
    TRepo: TenantRepository,
    SRepo: SiteRepository,
    SGRepo: SiteGroupRepository,
    RRepo: RegionRepository,
    CRepo: ClusterRepository,
    CTRepo: ClusterTypeRepository,
    CGRepo: ClusterGroupRepository,
    DRRepo: DeviceRoleRepository,
    DRepo: DeviceRepository,
    MRepo: ManufacturerRepository,
    DTRepo: DeviceTypeRepository,
    VMRepo: VirtualMachineRepository,
{
    type TenantGroupRepo = TGRepo;
    type TenantRepo = TRepo;
    type SiteRepo = SRepo;
    type SiteGroupRepo = SGRepo;
    type RegionRepo = RRepo;
    type ClusterRepo = CRepo;
    type ClusterTypeRepo = CTRepo;
    type ClusterGroupRepo = CGRepo;
    type DeviceRoleRepo = DRRepo;
    type DeviceRepo = DRepo;
    type ManufacturerRepo = MRepo;
    type DeviceTypeRepo = DTRepo;
    type VirtualMachineRepo = VMRepo;

    fn tenant_group_repos(&self) -> Arc<Self::TenantGroupRepo> {
        self.tenant_groups.clone()
    }

    fn tenant_repos(&self) -> Arc<Self::TenantRepo> {
        self.tenants.clone()
    }

    fn site_repos(&self) -> Arc<Self::SiteRepo> {
        self.sites.clone()
    }

    fn site_group_repos(&self) -> Arc<Self::SiteGroupRepo> {
        self.site_groups.clone()
    }

    fn region_repos(&self) -> Arc<Self::RegionRepo> {
        self.regions.clone()
    }

    fn cluster_repos(&self) -> Arc<Self::ClusterRepo> {
        self.clusters.clone()
    }

    fn cluster_type_repos(&self) -> Arc<Self::ClusterTypeRepo> {
        self.cluster_types.clone()
    }

    fn cluster_group_repos(&self) -> Arc<Self::ClusterGroupRepo> {
        self.cluster_groups.clone()
    }

    fn device_role_repos(&self) -> Arc<Self::DeviceRoleRepo> {
        self.device_roles.clone()
    }

    fn device_repos(&self) -> Arc<Self::DeviceRepo> {
        self.devices.clone()
    }

    fn manufacturer_repos(&self) -> Arc<Self::ManufacturerRepo> {
        self.manufacturers.clone()
    }

    fn device_type_repos(&self) -> Arc<Self::DeviceTypeRepo> {
        self.device_types.clone()
    }

    fn virtual_machine_repos(&self) -> Arc<Self::VirtualMachineRepo> {
        self.virtual_machines.clone()
    }
}

pub struct InventoryAdminService<R: InventoryAdminRepositoryBundle> {
    tenant_groups: Arc<R::TenantGroupRepo>,
    tenants: Arc<R::TenantRepo>,
    sites: Arc<R::SiteRepo>,
    site_groups: Arc<R::SiteGroupRepo>,
    regions: Arc<R::RegionRepo>,
    clusters: Arc<R::ClusterRepo>,
    cluster_types: Arc<R::ClusterTypeRepo>,
    cluster_groups: Arc<R::ClusterGroupRepo>,
    device_roles: Arc<R::DeviceRoleRepo>,
    devices: Arc<R::DeviceRepo>,
    manufacturers: Arc<R::ManufacturerRepo>,
    device_types: Arc<R::DeviceTypeRepo>,
    virtual_machines: Arc<R::VirtualMachineRepo>,
}

impl<R: InventoryAdminRepositoryBundle> InventoryAdminService<R> {
    pub fn new(repositories: R) -> Self {
        Self {
            tenant_groups: repositories.tenant_group_repos(),
            tenants: repositories.tenant_repos(),
            sites: repositories.site_repos(),
            site_groups: repositories.site_group_repos(),
            regions: repositories.region_repos(),
            clusters: repositories.cluster_repos(),
            cluster_types: repositories.cluster_type_repos(),
            cluster_groups: repositories.cluster_group_repos(),
            device_roles: repositories.device_role_repos(),
            devices: repositories.device_repos(),
            manufacturers: repositories.manufacturer_repos(),
            device_types: repositories.device_type_repos(),
            virtual_machines: repositories.virtual_machine_repos(),
        }
    }

    pub async fn list_tenant_groups(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<TenantGroup>, i64), AppError> {
        self.tenant_groups
            .list_paginated(
                query.limit,
                query.offset,
                query.search.as_deref(),
                &query.parent_id,
                query.sort,
            )
            .await
    }

    pub async fn get_tenant_group(
        &self,
        tenant_group_id: Uuid,
    ) -> Result<Option<TenantGroup>, AppError> {
        self.tenant_groups.get_by_id(tenant_group_id).await
    }

    pub async fn create_tenant_group(&self, tenant_group: TenantGroup) -> Result<(), AppError> {
        self.tenant_groups.create(&tenant_group).await
    }

    pub async fn update_tenant_group(&self, tenant_group: TenantGroup) -> Result<(), AppError> {
        self.tenant_groups.update(&tenant_group).await
    }

    pub async fn delete_tenant_group(&self, tenant_group_id: Uuid) -> Result<(), AppError> {
        self.tenant_groups.delete(tenant_group_id).await
    }

    pub async fn list_tenants(&self, query: InventoryListQuery) -> Result<Vec<Tenant>, AppError> {
        self.tenants.list_paginated(query.limit, query.offset).await
    }

    pub async fn list_tenants_hydrated(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<TenantListView>, i64), AppError> {
        self.tenants
            .list_hydrated(
                query.limit,
                query.offset,
                query.search.as_deref(),
                &query.group_id,
                query.sort,
            )
            .await
    }

    pub async fn get_tenant(&self, tenant_id: Uuid) -> Result<Option<Tenant>, AppError> {
        self.tenants.get_by_id(tenant_id).await
    }

    pub async fn create_tenant(&self, tenant: Tenant) -> Result<(), AppError> {
        self.tenants.create(&tenant).await
    }

    pub async fn update_tenant(&self, tenant: Tenant) -> Result<(), AppError> {
        self.tenants.update(&tenant).await
    }

    pub async fn delete_tenant(&self, tenant_id: Uuid) -> Result<(), AppError> {
        self.tenants.delete(tenant_id).await
    }

    pub async fn list_sites(&self, query: InventoryListQuery) -> Result<Vec<Site>, AppError> {
        self.sites.list_paginated(query.limit, query.offset).await
    }

    pub async fn list_sites_hydrated(
        &self,
        query: InventorySiteListQuery,
    ) -> Result<(Vec<SiteListView>, i64), AppError> {
        self.sites
            .list_hydrated(
                query.filter,
                query.limit,
                query.offset,
                query.search.as_deref(),
            )
            .await
    }

    pub async fn get_site(&self, site_id: Uuid) -> Result<Option<Site>, AppError> {
        self.sites.get_by_id(site_id).await
    }

    pub async fn create_site(&self, site: Site) -> Result<(), AppError> {
        self.sites.create(&site).await
    }

    pub async fn update_site(&self, site: Site) -> Result<(), AppError> {
        self.sites.update(&site).await
    }

    pub async fn delete_site(&self, site_id: Uuid) -> Result<(), AppError> {
        self.sites.delete(site_id).await
    }

    pub async fn list_site_groups(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<SiteGroup>, i64), AppError> {
        self.site_groups
            .list_paginated(
                query.limit,
                query.offset,
                query.search.as_deref(),
                &query.parent_id,
                query.sort,
            )
            .await
    }

    pub async fn get_site_group(&self, group_id: Uuid) -> Result<Option<SiteGroup>, AppError> {
        self.site_groups.get_by_id(group_id).await
    }

    pub async fn create_site_group(&self, group: SiteGroup) -> Result<(), AppError> {
        self.site_groups.create(&group).await
    }

    pub async fn update_site_group(&self, group: SiteGroup) -> Result<(), AppError> {
        self.site_groups.update(&group).await
    }

    pub async fn delete_site_group(&self, group_id: Uuid) -> Result<(), AppError> {
        self.site_groups.delete(group_id).await
    }

    pub async fn list_regions_hydrated(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<RegionListView>, i64), AppError> {
        self.regions
            .list_hydrated(
                query.limit,
                query.offset,
                query.search.as_deref(),
                &query.parent_id,
                query.sort,
            )
            .await
    }

    pub async fn list_regions(&self, query: InventoryListQuery) -> Result<Vec<Region>, AppError> {
        self.regions.list_paginated(query.limit, query.offset).await
    }

    pub async fn get_region(&self, region_id: Uuid) -> Result<Option<Region>, AppError> {
        self.regions.get_by_id(region_id).await
    }

    pub async fn create_region(&self, region: Region) -> Result<(), AppError> {
        self.regions.create(&region).await
    }

    pub async fn update_region(&self, region: Region) -> Result<(), AppError> {
        self.regions.update(&region).await
    }

    pub async fn delete_region(&self, region_id: Uuid) -> Result<(), AppError> {
        self.regions.delete(region_id).await
    }

    pub async fn list_clusters(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<Cluster>, i64), AppError> {
        self.clusters
            .list_paginated(query.limit, query.offset)
            .await
    }

    pub async fn list_clusters_hydrated(
        &self,
        query: InventoryClusterListQuery,
    ) -> Result<(Vec<ClusterListView>, i64), AppError> {
        self.clusters
            .list_hydrated(
                query.filter,
                query.limit,
                query.offset,
                query.search.as_deref(),
            )
            .await
    }

    pub async fn get_cluster(&self, cluster_id: Uuid) -> Result<Option<Cluster>, AppError> {
        self.clusters.get_by_id(cluster_id).await
    }

    pub async fn create_cluster(&self, cluster: Cluster) -> Result<(), AppError> {
        self.clusters.create(&cluster).await
    }

    pub async fn update_cluster(&self, cluster: Cluster) -> Result<(), AppError> {
        self.clusters.update(&cluster).await
    }

    pub async fn delete_cluster(&self, cluster_id: Uuid) -> Result<(), AppError> {
        self.clusters.delete(cluster_id).await
    }

    pub async fn list_cluster_types(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<ClusterType>, i64), AppError> {
        self.cluster_types
            .list_paginated(
                query.limit,
                query.offset,
                query.search.as_deref(),
                query.sort,
            )
            .await
    }

    pub async fn get_cluster_type(
        &self,
        cluster_type_id: Uuid,
    ) -> Result<Option<ClusterType>, AppError> {
        self.cluster_types.get_by_id(cluster_type_id).await
    }

    pub async fn create_cluster_type(&self, cluster_type: ClusterType) -> Result<(), AppError> {
        self.cluster_types.create(&cluster_type).await
    }

    pub async fn update_cluster_type(&self, cluster_type: ClusterType) -> Result<(), AppError> {
        self.cluster_types.update(&cluster_type).await
    }

    pub async fn delete_cluster_type(&self, cluster_type_id: Uuid) -> Result<(), AppError> {
        self.cluster_types.delete(cluster_type_id).await
    }

    pub async fn list_cluster_groups(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<ClusterGroup>, i64), AppError> {
        self.cluster_groups
            .list_paginated(
                query.limit,
                query.offset,
                query.search.as_deref(),
                query.sort,
            )
            .await
    }

    pub async fn get_cluster_group(
        &self,
        cluster_group_id: Uuid,
    ) -> Result<Option<ClusterGroup>, AppError> {
        self.cluster_groups.get_by_id(cluster_group_id).await
    }

    pub async fn create_cluster_group(&self, cluster_group: ClusterGroup) -> Result<(), AppError> {
        self.cluster_groups.create(&cluster_group).await
    }

    pub async fn update_cluster_group(&self, cluster_group: ClusterGroup) -> Result<(), AppError> {
        self.cluster_groups.update(&cluster_group).await
    }

    pub async fn delete_cluster_group(&self, cluster_group_id: Uuid) -> Result<(), AppError> {
        self.cluster_groups.delete(cluster_group_id).await
    }

    pub async fn list_device_roles(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<DeviceRole>, i64), AppError> {
        self.device_roles
            .list_paginated(
                query.limit,
                query.offset,
                query.search.as_deref(),
                &query.vm_role,
                query.sort,
            )
            .await
    }

    pub async fn get_device_role(&self, role_id: Uuid) -> Result<Option<DeviceRole>, AppError> {
        self.device_roles.get_by_id(role_id).await
    }

    pub async fn create_device_role(&self, role: DeviceRole) -> Result<(), AppError> {
        self.device_roles.create(&role).await
    }

    pub async fn update_device_role(&self, role: DeviceRole) -> Result<(), AppError> {
        self.device_roles.update(&role).await
    }

    pub async fn delete_device_role(&self, role_id: Uuid) -> Result<(), AppError> {
        self.device_roles.delete(role_id).await
    }

    pub async fn list_devices(
        &self,
        query: InventoryDeviceListQuery,
    ) -> Result<Vec<Device>, AppError> {
        self.devices
            .list_paginated(query.filter, query.limit, query.offset)
            .await
    }

    pub async fn list_devices_hydrated(
        &self,
        query: InventoryDeviceListQuery,
    ) -> Result<(Vec<DeviceListView>, i64), AppError> {
        self.devices
            .list_hydrated(
                query.filter,
                query.limit,
                query.offset,
                query.search.as_deref(),
            )
            .await
    }

    pub async fn get_device(&self, device_id: Uuid) -> Result<Option<Device>, AppError> {
        self.devices.get_by_id(device_id).await
    }

    pub async fn create_device(&self, device: Device) -> Result<(), AppError> {
        self.devices.create(&device).await
    }

    pub async fn update_device(&self, device: Device) -> Result<(), AppError> {
        self.devices.update(&device).await
    }

    pub async fn delete_device(&self, device_id: Uuid) -> Result<(), AppError> {
        self.devices.delete(device_id).await
    }

    pub async fn list_manufacturers(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<Manufacturer>, i64), AppError> {
        self.manufacturers
            .list_paginated(
                query.limit,
                query.offset,
                query.search.as_deref(),
                query.sort,
            )
            .await
    }

    pub async fn get_manufacturer(
        &self,
        manufacturer_id: Uuid,
    ) -> Result<Option<Manufacturer>, AppError> {
        self.manufacturers.get_by_id(manufacturer_id).await
    }

    pub async fn create_manufacturer(&self, manufacturer: Manufacturer) -> Result<(), AppError> {
        self.manufacturers.create(&manufacturer).await
    }

    pub async fn update_manufacturer(&self, manufacturer: Manufacturer) -> Result<(), AppError> {
        self.manufacturers.update(&manufacturer).await
    }

    pub async fn delete_manufacturer(&self, manufacturer_id: Uuid) -> Result<(), AppError> {
        self.manufacturers.delete(manufacturer_id).await
    }

    pub async fn list_device_types(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<DeviceType>, i64), AppError> {
        self.device_types
            .list_paginated(query.limit, query.offset)
            .await
    }

    pub async fn list_device_types_hydrated(
        &self,
        query: InventoryListQuery,
    ) -> Result<(Vec<DeviceTypeListView>, i64), AppError> {
        self.device_types
            .list_hydrated(
                query.limit,
                query.offset,
                query.search.as_deref(),
                &query.manufacturer_id,
                query.sort,
            )
            .await
    }

    pub async fn get_device_type(
        &self,
        device_type_id: Uuid,
    ) -> Result<Option<DeviceType>, AppError> {
        self.device_types.get_by_id(device_type_id).await
    }

    pub async fn create_device_type(&self, device_type: DeviceType) -> Result<(), AppError> {
        self.device_types.create(&device_type).await
    }

    pub async fn update_device_type(&self, device_type: DeviceType) -> Result<(), AppError> {
        self.device_types.update(&device_type).await
    }

    pub async fn delete_device_type(&self, device_type_id: Uuid) -> Result<(), AppError> {
        self.device_types.delete(device_type_id).await
    }

    pub async fn list_virtual_machines(
        &self,
        query: InventoryListQuery,
    ) -> Result<Vec<VirtualMachine>, AppError> {
        self.virtual_machines
            .list_paginated(query.limit, query.offset)
            .await
    }

    pub async fn list_virtual_machines_hydrated(
        &self,
        query: InventoryVirtualMachineListQuery,
    ) -> Result<(Vec<VirtualMachineListView>, i64), AppError> {
        self.virtual_machines
            .list_hydrated(
                query.filter,
                query.limit,
                query.offset,
                query.search.as_deref(),
            )
            .await
    }

    pub async fn get_virtual_machine(
        &self,
        vm_id: Uuid,
    ) -> Result<Option<VirtualMachine>, AppError> {
        self.virtual_machines.get_by_id(vm_id).await
    }

    pub async fn create_virtual_machine(&self, vm: VirtualMachine) -> Result<(), AppError> {
        self.virtual_machines.create(&vm).await
    }

    pub async fn update_virtual_machine(&self, vm: VirtualMachine) -> Result<(), AppError> {
        self.virtual_machines.update(&vm).await
    }

    pub async fn delete_virtual_machine(&self, vm_id: Uuid) -> Result<(), AppError> {
        self.virtual_machines.delete(vm_id).await
    }
}
