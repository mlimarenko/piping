//! Bounded selector demand and non-recursive ownership/collision witnesses.
use super::*;

// All dynamic values are bound. Identifiers are selected by EndpointType only.
const HOST_ROWS: &str = "SELECT 'dcim.device' AS kind, id, name, primary_ip4_id, primary_ip6_id,site_id,role_id,tenant_id,platform_id,device_type_id,rack_id,location_id,cluster_id FROM dcim_device UNION ALL SELECT 'virtualization.virtualmachine', id, name, primary_ip4_id, primary_ip6_id,site_id,role_id,tenant_id,platform_id,NULL::uuid,NULL::uuid,NULL::uuid,cluster_id FROM virtualization_virtualmachine";
const SERVICE_ROWS: &str = "SELECT s.id, s.name, ct.app_label || '.' || ct.model AS parent_kind, s.parent_object_id AS parent_id, ARRAY(SELECT ipaddress_id FROM ipam_service_ipaddresses WHERE service_id = s.id ORDER BY ipaddress_id) AS bindings FROM ipam_service s JOIN inventory_content_type ct ON ct.id = s.parent_object_type_id";
const IP_ROWS: &str = "SELECT ip.id, ip.address::text, ip.status, ip.vrf_id, ip.assigned_object_id, ct.app_label || '.' || ct.model AS assigned_kind, COALESCE(di.device_id, vi.virtual_machine_id) AS assigned_host_id FROM ipam_ipaddress ip LEFT JOIN inventory_content_type ct ON ct.id = ip.assigned_object_type_id LEFT JOIN dcim_interface di ON di.id = ip.assigned_object_id AND ct.app_label = 'dcim' AND ct.model = 'interface' LEFT JOIN virtualization_vminterface vi ON vi.id = ip.assigned_object_id AND ct.app_label = 'virtualization' AND ct.model = 'vminterface'";

#[derive(PartialEq, Eq)]
pub(super) enum CatalogPurpose {
    Mutation,
    Preview,
    Policy {
        host: DataflowEndpoint,
        cross_vrf: bool,
    },
}
impl CatalogPurpose {
    fn expands(&self, selector: &DataflowEndpointInput) -> bool {
        *self != Self::Mutation
            && (*self == Self::Preview || selector.address_mode != DataflowAddressMode::Selected)
    }
}
enum HostDemand<'a> {
    All,
    Identities(&'a [DataflowEndpoint]),
    PrimaryOwners(&'a [Uuid]),
    Scope(&'a DataflowEndpointInput),
}

#[derive(Default)]
struct CatalogDemand {
    ip_ids: BTreeSet<Uuid>,
    full_hosts: BTreeSet<DataflowEndpoint>,
    primary_hosts: BTreeSet<DataflowEndpoint>,
    assigned_hosts: BTreeSet<DataflowEndpoint>,
    prefix_ids: BTreeSet<Uuid>,
    all: bool,
}
impl CatalogDemand {
    fn add_service(&mut self, service: &ServiceRow, purpose: &CatalogPurpose) {
        if *purpose == CatalogPurpose::Preview && service.bindings.is_empty() {
            self.full_hosts.extend(service_parent(service));
        } else if service.bindings.is_empty() {
            self.assigned_hosts.extend(service_parent(service));
        }
        self.ip_ids.extend(service.bindings.iter().copied());
    }
    async fn load_ips(
        mut self,
        connection: &mut PgConnection,
        catalog: &Catalog,
    ) -> Result<Vec<IpRow>, AppError> {
        for host in &self.primary_hosts {
            if let Some(row) = catalog.hosts.get(host) {
                self.ip_ids.extend(
                    [row.primary_ip4_id, row.primary_ip6_id]
                        .into_iter()
                        .flatten(),
                );
            }
        }
        // Only this stage expands owners into addresses. Witness discovery must
        // not recursively import an unrelated owner's entire inventory.
        let mut query =
            QueryBuilder::<Postgres>::new(format!("SELECT ip.* FROM ({IP_ROWS}) ip WHERE "));
        query
            .push_bind(self.all)
            .push(" OR ip.id = ANY(")
            .push_bind(self.ip_ids.into_iter().collect::<Vec<_>>())
            .push(")");
        push_host_ip_demand(&mut query, &self.full_hosts, true);
        push_host_ip_demand(&mut query, &self.assigned_hosts, false);
        // Stored IP masks describe their subnet, not host membership in a /32 or /128.
        query.push(" OR EXISTS (SELECT 1 FROM ipam_prefix p WHERE p.id = ANY(").push_bind(self.prefix_ids.into_iter().collect::<Vec<_>>()).push(") AND p.vrf_id IS NOT DISTINCT FROM ip.vrf_id AND host(ip.address::inet)::inet <<= p.prefix) LIMIT 8193");
        let rows = query
            .build_query_as::<IpRow>()
            .fetch_all(connection)
            .await
            .map_err(classify_sqlx_error_owned)?;
        check_catalog_budget(rows.len(), 8192)?;
        Ok(rows)
    }
}
fn check_catalog_budget(count: usize, maximum: usize) -> Result<(), AppError> {
    if count > maximum {
        Err(AppError::TooManyRequests(
            "Inventory snapshot budget exceeded; no partial policy can be exported".into(),
        ))
    } else {
        Ok(())
    }
}
fn service_parent(service: &ServiceRow) -> Option<DataflowEndpoint> {
    // Unsupported parents remain in the catalog for service_candidates diagnostics.
    let kind = EndpointType::parse(&service.parent_kind).ok()?;
    kind.is_host().then_some(DataflowEndpoint {
        object_type: kind,
        object_id: service.parent_id,
    })
}
fn assigned_parent(ip: &IpRow) -> Option<DataflowEndpoint> {
    let object_type = match ip.assigned_kind.as_deref()? {
        "dcim.interface" => EndpointType::Device,
        "virtualization.vminterface" => EndpointType::VirtualMachine,
        // catalog_ip retains the unsupported-assignment diagnostic.
        _ => return None,
    };
    Some(DataflowEndpoint {
        object_type,
        object_id: ip.assigned_host_id?,
    })
}
fn row_owned_by_host(
    ip: &IpRow,
    host: &DataflowEndpoint,
    hosts: &BTreeMap<DataflowEndpoint, HostRow>,
    services: &BTreeMap<Uuid, ServiceRow>,
) -> bool {
    assigned_parent(ip).as_ref() == Some(host)
        || hosts.get(host).is_some_and(|row| {
            row.primary_ip4_id == Some(ip.id) || row.primary_ip6_id == Some(ip.id)
        })
        || services
            .values()
            .any(|s| service_parent(s).as_ref() == Some(host) && s.bindings.contains(&ip.id))
}
fn push_host_ip_demand(
    query: &mut QueryBuilder<'_, Postgres>,
    hosts: &BTreeSet<DataflowEndpoint>,
    full: bool,
) {
    for (kind, assignment) in [
        (EndpointType::Device, "dcim.interface"),
        (EndpointType::VirtualMachine, "virtualization.vminterface"),
    ] {
        let ids: Vec<_> = hosts
            .iter()
            .filter(|host| host.object_type == kind)
            .map(|host| host.object_id)
            .collect();
        if ids.is_empty() {
            continue;
        }
        query
            .push(" OR (ip.assigned_kind = ")
            .push_bind(assignment)
            .push(" AND ip.assigned_host_id = ANY(")
            .push_bind(ids.clone())
            .push("))");
        if full {
            query
                .push(format!(
                    " OR EXISTS (SELECT 1 FROM ({HOST_ROWS}) h WHERE h.kind = "
                ))
                .push_bind(kind.as_str())
                .push(" AND h.id = ANY(")
                .push_bind(ids.clone())
                .push(") AND (h.primary_ip4_id = ip.id OR h.primary_ip6_id = ip.id))");
            query.push(" OR EXISTS (SELECT 1 FROM ipam_service s JOIN inventory_content_type ct ON ct.id = s.parent_object_type_id JOIN ipam_service_ipaddresses binding ON binding.service_id = s.id WHERE ct.app_label || '.' || ct.model = ").push_bind(kind.as_str()).push(" AND s.parent_object_id = ANY(").push_bind(ids).push(") AND binding.ipaddress_id = ip.id)");
        }
    }
}
fn scope_host_facet(kind: EndpointType) -> &'static str {
    match kind {
        EndpointType::Region => "(SELECT region_id FROM dcim_site WHERE id = h.site_id)",
        EndpointType::Site => "h.site_id",
        EndpointType::SiteGroup => "(SELECT group_id FROM dcim_site WHERE id = h.site_id)",
        EndpointType::DeviceRole => "h.role_id",
        EndpointType::Tenant => "h.tenant_id",
        EndpointType::TenantGroup => "(SELECT group_id FROM tenancy_tenant WHERE id = h.tenant_id)",
        EndpointType::Platform => "h.platform_id",
        EndpointType::Manufacturer => {
            "(SELECT manufacturer_id FROM dcim_devicetype WHERE id = h.device_type_id)"
        }
        EndpointType::DeviceType => "h.device_type_id",
        EndpointType::Rack => "h.rack_id",
        EndpointType::Location => "h.location_id",
        EndpointType::Cluster => "h.cluster_id",
        EndpointType::ClusterGroup => {
            "(SELECT group_id FROM virtualization_cluster WHERE id = h.cluster_id)"
        }
        EndpointType::ClusterType => {
            "(SELECT type_id FROM virtualization_cluster WHERE id = h.cluster_id)"
        }
        // HostDemand::Scope is constructed only after is_scope().
        _ => "NULL::uuid",
    }
}

impl Catalog {
    pub(super) async fn load(
        connection: &mut PgConnection,
        selectors: &[&DataflowEndpointInput],
        purpose: CatalogPurpose,
    ) -> Result<Self, AppError> {
        let mut catalog = Self {
            scopes: BTreeMap::new(),
            hosts: BTreeMap::new(),
            services: BTreeMap::new(),
            ips: BTreeMap::new(),
            prefixes: BTreeMap::new(),
        };
        let mut demand = CatalogDemand::default();
        for selector in selectors {
            catalog
                .load_reference(connection, selector, &purpose, &mut demand)
                .await?;
        }
        let parents: Vec<_> = catalog
            .services
            .values()
            .filter_map(service_parent)
            .collect();
        catalog
            .load_hosts(connection, HostDemand::Identities(&parents))
            .await?;
        for selector in selectors {
            catalog
                .expand_selector(connection, selector, &purpose, &mut demand)
                .await?;
        }
        if demand.all {
            catalog.load_hosts(connection, HostDemand::All).await?;
        }
        if let CatalogPurpose::Policy { ref host, .. } = purpose {
            catalog
                .load_hosts(
                    connection,
                    HostDemand::Identities(std::slice::from_ref(host)),
                )
                .await?;
            demand.full_hosts.insert(host.clone());
        }
        let mut rows = demand.load_ips(connection, &catalog).await?;
        let demanded_ids: Vec<_> = rows.iter().map(|row| row.id).collect();
        catalog
            .load_services(connection, &[], &demanded_ids)
            .await?;
        rows.extend(
            catalog
                .load_address_witnesses(connection, &rows, selectors, &purpose)
                .await?,
        );
        let rows: BTreeMap<_, _> = rows.into_iter().map(|row| (row.id, row)).collect();
        check_catalog_budget(
            rows.len()
                + catalog
                    .ips
                    .keys()
                    .filter(|id| !rows.contains_key(id))
                    .count(),
            8192,
        )?;
        catalog.load_ownership_witnesses(connection, rows).await?;
        Ok(catalog)
    }
    async fn load_ownership_witnesses(
        &mut self,
        connection: &mut PgConnection,
        rows: BTreeMap<Uuid, IpRow>,
    ) -> Result<(), AppError> {
        let ids: Vec<_> = rows.keys().copied().collect();
        self.load_services(connection, &[], &ids).await?;
        let mut owners: Vec<_> = rows.values().filter_map(assigned_parent).collect();
        owners.extend(self.services.values().filter_map(service_parent));
        self.load_hosts(connection, HostDemand::Identities(&owners))
            .await?;
        self.load_hosts(connection, HostDemand::PrimaryOwners(&ids))
            .await?;
        self.load_host_facets(connection).await?;
        let mut primary_owners: BTreeMap<Uuid, BTreeSet<DataflowEndpoint>> = BTreeMap::new();
        for (host, row) in &self.hosts {
            for id in [row.primary_ip4_id, row.primary_ip6_id]
                .into_iter()
                .flatten()
            {
                primary_owners.entry(id).or_default().insert(host.clone());
            }
        }
        for (id, row) in rows {
            self.ips.insert(
                id,
                catalog_ip(row, primary_owners.remove(&id).unwrap_or_default()),
            );
        }
        for service in self.services.values() {
            bind_service_ownership(service, &self.hosts, &mut self.ips);
        }
        mark_ambiguous_contexts(&mut self.ips);
        Ok(())
    }
    async fn load_reference(
        &mut self,
        connection: &mut PgConnection,
        selector: &DataflowEndpointInput,
        purpose: &CatalogPurpose,
        demand: &mut CatalogDemand,
    ) -> Result<(), AppError> {
        demand.ip_ids.extend(selector.ipaddresses.iter().copied());
        let Some(id) = selector.object_id else {
            demand.all |= selector.object_type == EndpointType::All && purpose.expands(selector);
            return Ok(());
        };
        if selector.object_type.is_scope() {
            return self
                .load_scopes(
                    connection,
                    selector.object_type,
                    &[id],
                    purpose.expands(selector)
                        || selector.address_mode == DataflowAddressMode::Selected,
                )
                .await;
        }
        match selector.object_type {
            EndpointType::Device | EndpointType::VirtualMachine => {
                let host = DataflowEndpoint {
                    object_type: selector.object_type,
                    object_id: id,
                };
                self.load_hosts(connection, HostDemand::Identities(&[host]))
                    .await?;
            }
            EndpointType::Service => self.load_services(connection, &[id], &[]).await?,
            EndpointType::IpAddress => {
                self.load_ip_reference(connection, id, purpose, demand)
                    .await?
            }
            EndpointType::Prefix => {
                let row = sqlx::query_as::<_, PrefixRow>(
                    "SELECT id, prefix::text, status, vrf_id FROM ipam_prefix WHERE id = $1",
                )
                .bind(id)
                .fetch_optional(&mut *connection)
                .await
                .map_err(classify_sqlx_error_owned)?;
                if let Some(row) = row {
                    self.prefixes.insert(row.id, row);
                }
                check_catalog_budget(self.prefixes.len(), 4096)?;
                if purpose.expands(selector) {
                    demand.prefix_ids.insert(id);
                }
            }
            _ => {}
        }
        Ok(())
    }
    async fn load_ip_reference(
        &mut self,
        connection: &mut PgConnection,
        id: Uuid,
        purpose: &CatalogPurpose,
        demand: &mut CatalogDemand,
    ) -> Result<(), AppError> {
        if *purpose != CatalogPurpose::Mutation {
            demand.ip_ids.insert(id);
            return Ok(());
        }
        // A non-selected mutation needs only this reference and label, not owners.
        let row = sqlx::query_as::<_, IpRow>(&format!(
            "SELECT ip.* FROM ({IP_ROWS}) ip WHERE ip.id = $1"
        ))
        .bind(id)
        .fetch_optional(connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if let Some(row) = row {
            self.ips.insert(id, catalog_ip(row, BTreeSet::new()));
        }
        Ok(())
    }
    async fn expand_selector(
        &mut self,
        connection: &mut PgConnection,
        selector: &DataflowEndpointInput,
        purpose: &CatalogPurpose,
        demand: &mut CatalogDemand,
    ) -> Result<(), AppError> {
        if !purpose.expands(selector) {
            return Ok(());
        }
        let members = if selector.object_type.is_scope() {
            self.load_hosts(connection, HostDemand::Scope(selector))
                .await?;
            self.load_host_facets(connection).await?;
            // This stage determines demand only; resolve_scope retains diagnostics.
            self.scope_members(selector, &mut Vec::new())
        } else if selector.object_type.is_host() {
            selector.identity().into_iter().collect()
        } else {
            BTreeSet::new()
        };
        if *purpose == CatalogPurpose::Preview || selector.address_mode == DataflowAddressMode::All
        {
            demand.full_hosts.extend(members);
        } else {
            demand.primary_hosts.extend(members);
        }
        if let Some(service) = selector
            .object_id
            .filter(|_| selector.object_type == EndpointType::Service)
            .and_then(|id| self.services.get(&id))
        {
            demand.add_service(service, purpose);
        }
        Ok(())
    }
    async fn load_address_witnesses(
        &self,
        connection: &mut PgConnection,
        rows: &[IpRow],
        selectors: &[&DataflowEndpointInput],
        purpose: &CatalogPurpose,
    ) -> Result<Vec<IpRow>, AppError> {
        let demanded_ids: Vec<_> = rows.iter().map(|row| row.id).collect();
        let mut query = QueryBuilder::<Postgres>::new(format!(
            "SELECT ip.* FROM ({IP_ROWS}) ip WHERE EXISTS (SELECT 1 FROM ipam_ipaddress demanded WHERE demanded.id = ANY("
        ));
        query.push_bind(demanded_ids.clone()).push(") AND host(demanded.address) = host(ip.address::inet) AND demanded.vrf_id IS NOT DISTINCT FROM ip.vrf_id)");
        // Inactive/unresolved local-realm records still witness cross-VRF collisions.
        if let CatalogPurpose::Policy {
            host,
            cross_vrf: true,
        } = purpose
        {
            let collision_prefixes: Vec<_> = selectors
                .iter()
                .filter(|selector| {
                    selector.object_type == EndpointType::Prefix
                        && selector.address_mode == DataflowAddressMode::Default
                })
                .filter_map(|selector| selector.object_id)
                .collect();
            let local_vrfs: BTreeSet<_> = rows
                .iter()
                .filter(|ip| row_owned_by_host(ip, host, &self.hosts, &self.services))
                .map(|ip| ip.vrf_id)
                .collect();
            for vrf in local_vrfs {
                query.push(" OR (ip.vrf_id IS NOT DISTINCT FROM ").push_bind(vrf).push(" AND (EXISTS (SELECT 1 FROM ipam_ipaddress remote WHERE remote.id = ANY(").push_bind(demanded_ids.clone()).push(") AND host(remote.address) = host(ip.address::inet)) OR EXISTS (SELECT 1 FROM ipam_prefix remote WHERE remote.id = ANY(").push_bind(collision_prefixes.clone()).push(") AND host(ip.address::inet)::inet <<= remote.prefix)))");
            }
        }
        query.push(" LIMIT 8193");
        query
            .build_query_as::<IpRow>()
            .fetch_all(connection)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    async fn load_scopes(
        &mut self,
        connection: &mut PgConnection,
        kind: EndpointType,
        ids: &[Uuid],
        descendants: bool,
    ) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        let table = EndpointType::REFERENCES
            .iter()
            .find(|entry| entry.0 == kind)
            .map(|entry| entry.3)
            .ok_or_else(|| AppError::Validation("Unsupported scope".into()))?;
        let rows = ScopeRow::load(kind, table, ids, descendants, connection).await?;
        let accumulated = self.scopes.entry(kind).or_default();
        accumulated.extend(rows);
        check_catalog_budget(accumulated.len(), 4096)
    }
    async fn load_host_facets(&mut self, connection: &mut PgConnection) -> Result<(), AppError> {
        for (kind, ids) in [
            (
                EndpointType::Site,
                self.hosts
                    .values()
                    .filter_map(|h| h.site_id)
                    .collect::<BTreeSet<_>>(),
            ),
            (
                EndpointType::Tenant,
                self.hosts.values().filter_map(|h| h.tenant_id).collect(),
            ),
            (
                EndpointType::DeviceType,
                self.hosts
                    .values()
                    .filter_map(|h| h.device_type_id)
                    .collect(),
            ),
            (
                EndpointType::Cluster,
                self.hosts.values().filter_map(|h| h.cluster_id).collect(),
            ),
        ] {
            self.load_scopes(
                connection,
                kind,
                &ids.into_iter().collect::<Vec<_>>(),
                false,
            )
            .await?;
        }
        Ok(())
    }
    async fn load_services(
        &mut self,
        connection: &mut PgConnection,
        ids: &[Uuid],
        ip_ids: &[Uuid],
    ) -> Result<(), AppError> {
        let rows = sqlx::query_as::<_, ServiceRow>(&format!("SELECT s.* FROM ({SERVICE_ROWS}) s WHERE s.id = ANY($1) OR s.bindings && $2::uuid[] LIMIT 4097")).bind(ids).bind(ip_ids).fetch_all(connection).await.map_err(classify_sqlx_error_owned)?;
        self.services
            .extend(rows.into_iter().map(|row| (row.id, row)));
        check_catalog_budget(self.services.len(), 4096)
    }
    async fn load_hosts(
        &mut self,
        connection: &mut PgConnection,
        demand: HostDemand<'_>,
    ) -> Result<(), AppError> {
        let mut query =
            QueryBuilder::<Postgres>::new(format!("SELECT h.* FROM ({HOST_ROWS}) h WHERE "));
        match demand {
            HostDemand::All => {
                query.push("true");
            }
            HostDemand::Identities(hosts) => {
                query.push("false");
                for kind in [EndpointType::Device, EndpointType::VirtualMachine] {
                    query
                        .push(" OR (h.kind = ")
                        .push_bind(kind.as_str())
                        .push(" AND h.id = ANY(")
                        .push_bind(
                            hosts
                                .iter()
                                .filter(|host| host.object_type == kind)
                                .map(|host| host.object_id)
                                .collect::<Vec<_>>(),
                        )
                        .push("))");
                }
            }
            HostDemand::PrimaryOwners(ids) => {
                query
                    .push("h.primary_ip4_id = ANY(")
                    .push_bind(ids.to_vec())
                    .push(") OR h.primary_ip6_id = ANY(")
                    .push_bind(ids.to_vec())
                    .push(")");
            }
            HostDemand::Scope(selector) => {
                query
                    .push(scope_host_facet(selector.object_type))
                    .push(" = ANY(")
                    .push_bind(
                        self.scope_descendants(selector)
                            .into_iter()
                            .collect::<Vec<_>>(),
                    )
                    .push(")");
            }
        }
        query.push(" LIMIT 4097");
        let rows = query
            .build_query_as::<HostRow>()
            .fetch_all(connection)
            .await
            .map_err(classify_sqlx_error_owned)?;
        for row in rows {
            self.hosts.insert(
                DataflowEndpoint {
                    object_type: EndpointType::parse(&row.kind)?,
                    object_id: row.id,
                },
                row,
            );
        }
        check_catalog_budget(self.hosts.len(), 4096)
    }
    fn scope_descendants(&self, selector: &DataflowEndpointInput) -> BTreeSet<Uuid> {
        let mut ids: BTreeSet<_> = selector.object_id.into_iter().collect();
        if selector.object_type.is_hierarchical()
            && let Some(rows) = self.scopes.get(&selector.object_type)
        {
            loop {
                let before = ids.len();
                let children: Vec<_> = rows
                    .values()
                    .filter(|row| row.parent_id.is_some_and(|id| ids.contains(&id)))
                    .map(|row| row.id)
                    .collect();
                ids.extend(children);
                if ids.len() == before {
                    break;
                }
            }
        }
        ids
    }
}
