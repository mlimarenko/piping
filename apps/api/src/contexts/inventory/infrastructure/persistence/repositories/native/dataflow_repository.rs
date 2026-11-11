//! Typed intent persistence and repeatable-read inventory resolution.
use crate::{
    contexts::{
        inventory::{
            application::dataflow_policy::{
                MAX_ACTIVE_FLOWS, MAX_ALL_ADDRESSES, MAX_ENDPOINT_ADDRESSES, MAX_HOST_MEMBERSHIP,
                PolicyAddress, PolicyNetwork, PolicySnapshot, ResolvedEndpoint,
            },
            domain::native::dataflow::{
                Dataflow, DataflowAddressMode, DataflowEndpoint, DataflowEndpointInput,
                DataflowEndpointResponse, DataflowStatus, DataflowTransport, EndpointType,
            },
        },
        shared::pagination::{clamp_limit, clamp_offset},
    },
    errors::{AppError, classify_sqlx_error_owned},
};
use sqlx::{FromRow, PgConnection, PgPool, Postgres, QueryBuilder};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

mod catalog_loader;
use catalog_loader::CatalogPurpose;

fn endpoint_label(kind: EndpointType, label: Option<String>) -> String {
    label.unwrap_or_else(|| match kind {
        EndpointType::All => "All inventory hosts".into(),
        EndpointType::Internet => "Internet (any IP)".into(),
        EndpointType::Device | EndpointType::VirtualMachine => "Host".into(),
        EndpointType::Service => "Service".into(),
        EndpointType::IpAddress => "IP address".into(),
        EndpointType::Prefix => "Prefix".into(),
        _ => "Scope".into(),
    })
}

fn endpoint_projection(
    selector: &DataflowEndpointInput,
    label: Option<String>,
) -> DataflowEndpointResponse {
    DataflowEndpointResponse {
        object_type: selector.object_type,
        object_id: selector.object_id,
        tag_object_type: selector.tag_object_type,
        address_mode: selector.address_mode,
        ipaddresses: selector.ipaddresses.clone(),
        label,
    }
}

fn select_flows() -> &'static str {
    "SELECT f.id, f.name, f.description, f.comments, f.status, f.transport, f.destination_ports, f.source_ports, f.icmp_types, f.allow_cross_vrf, f.application_id, f.group_id, f.owner_id, f.tenant_id, f.tags, f.custom_field_data FROM inventory_dataflow f"
}

#[derive(Debug, FromRow)]
struct DataflowRow {
    id: Uuid,
    name: String,
    description: String,
    comments: String,
    status: String,
    transport: String,
    destination_ports: Option<Vec<i32>>,
    source_ports: Option<Vec<i32>>,
    icmp_types: Option<Vec<i32>>,
    allow_cross_vrf: bool,
    application_id: Option<Uuid>,
    group_id: Option<Uuid>,
    owner_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    tags: Vec<String>,
    custom_field_data: serde_json::Value,
}

#[derive(Debug, FromRow)]
struct SelectorRow {
    id: Uuid,
    dataflow_id: Option<Uuid>,
    side: String,
    object_type: String,
    address_mode: String,
    tag_object_type: Option<String>,
    device_id: Option<Uuid>,
    vm_id: Option<Uuid>,
    service_id: Option<Uuid>,
    ipaddress_id: Option<Uuid>,
    prefix_id: Option<Uuid>,
    iprange_id: Option<Uuid>,
    tag_id: Option<Uuid>,
    addressgroup_id: Option<Uuid>,
    region_id: Option<Uuid>,
    site_id: Option<Uuid>,
    sitegroup_id: Option<Uuid>,
    devicerole_id: Option<Uuid>,
    #[sqlx(rename = "tenant_id")]
    tenant_ref_id: Option<Uuid>,
    tenantgroup_id: Option<Uuid>,
    platform_id: Option<Uuid>,
    manufacturer_id: Option<Uuid>,
    devicetype_id: Option<Uuid>,
    rack_id: Option<Uuid>,
    location_id: Option<Uuid>,
    cluster_id: Option<Uuid>,
    clustergroup_id: Option<Uuid>,
    clustertype_id: Option<Uuid>,
}

fn parse_mode(mode: &str) -> Result<DataflowAddressMode, AppError> {
    match mode {
        "default" => Ok(DataflowAddressMode::Default),
        "selected" => Ok(DataflowAddressMode::Selected),
        "all" => Ok(DataflowAddressMode::All),
        "oob" => Ok(DataflowAddressMode::Oob),
        _ => Err(AppError::Database("Invalid endpoint address mode".into())),
    }
}

fn parse_status(status: &str) -> Result<DataflowStatus, AppError> {
    match status {
        "draft" => Ok(DataflowStatus::Draft),
        "active" => Ok(DataflowStatus::Active),
        "disabled" => Ok(DataflowStatus::Disabled),
        _ => Err(AppError::Database("Invalid dataflow status".into())),
    }
}

fn parse_transport(transport: &str) -> Result<DataflowTransport, AppError> {
    match transport {
        "any" => Ok(DataflowTransport::Any),
        "tcp" => Ok(DataflowTransport::Tcp),
        "udp" => Ok(DataflowTransport::Udp),
        "tcp_udp" => Ok(DataflowTransport::TcpUdp),
        "sctp" => Ok(DataflowTransport::Sctp),
        "icmp" => Ok(DataflowTransport::Icmp),
        "icmpv6" => Ok(DataflowTransport::Icmpv6),
        _ => Err(AppError::Database("Invalid dataflow transport".into())),
    }
}

impl SelectorRow {
    fn typed_id(&self, kind: EndpointType) -> Option<Uuid> {
        match kind {
            EndpointType::Device => self.device_id,
            EndpointType::VirtualMachine => self.vm_id,
            EndpointType::Service => self.service_id,
            EndpointType::IpAddress => self.ipaddress_id,
            EndpointType::Prefix => self.prefix_id,
            EndpointType::IpRange => self.iprange_id,
            EndpointType::Tag => self.tag_id,
            EndpointType::AddressGroup => self.addressgroup_id,
            EndpointType::Region => self.region_id,
            EndpointType::Site => self.site_id,
            EndpointType::SiteGroup => self.sitegroup_id,
            EndpointType::DeviceRole => self.devicerole_id,
            EndpointType::Tenant => self.tenant_ref_id,
            EndpointType::TenantGroup => self.tenantgroup_id,
            EndpointType::Platform => self.platform_id,
            EndpointType::Manufacturer => self.manufacturer_id,
            EndpointType::DeviceType => self.devicetype_id,
            EndpointType::Rack => self.rack_id,
            EndpointType::Location => self.location_id,
            EndpointType::Cluster => self.cluster_id,
            EndpointType::ClusterGroup => self.clustergroup_id,
            EndpointType::ClusterType => self.clustertype_id,
            EndpointType::All | EndpointType::Internet => None,
        }
    }
    fn into_input(self) -> Result<DataflowEndpointInput, AppError> {
        let object_type = EndpointType::parse(&self.object_type)?;
        let tag_object_type = self
            .tag_object_type
            .as_deref()
            .map(EndpointType::parse)
            .transpose()?;
        Ok(DataflowEndpointInput {
            object_type,
            object_id: self.typed_id(object_type),
            address_mode: parse_mode(&self.address_mode)?,
            ipaddresses: Vec::new(),
            tag_object_type,
        })
    }
}

#[derive(Debug, FromRow)]
struct GroupRow {
    id: Uuid,
    parent_id: Option<Uuid>,
    status: String,
    tags: Vec<String>,
}

fn group_ancestry<'a>(
    groups: &'a BTreeMap<Uuid, GroupRow>,
    start: Option<Uuid>,
) -> Vec<&'a GroupRow> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = start;
    while let Some(id) = current {
        if !seen.insert(id) || chain.len() >= 64 {
            break;
        }
        let Some(group) = groups.get(&id) else {
            break;
        };
        chain.push(group);
        current = group.parent_id;
    }
    chain
}

fn effective_state(
    own: DataflowStatus,
    ancestors: &[&GroupRow],
) -> Result<(DataflowStatus, Option<Uuid>), AppError> {
    let nearest_disabled = ancestors
        .iter()
        .find(|group| group.status == "disabled")
        .map(|group| group.id);
    let nearest_draft = ancestors
        .iter()
        .find(|group| group.status == "draft")
        .map(|group| group.id);
    Ok(match own {
        DataflowStatus::Disabled => (DataflowStatus::Disabled, None),
        _ if nearest_disabled.is_some() => (DataflowStatus::Disabled, nearest_disabled),
        DataflowStatus::Draft => (DataflowStatus::Draft, None),
        _ if nearest_draft.is_some() => (DataflowStatus::Draft, nearest_draft),
        _ => (DataflowStatus::Active, None),
    })
}

fn tag_uuid(tag: &str) -> Option<Uuid> {
    Uuid::parse_str(tag).ok()
}
#[derive(Debug, Default)]
pub struct DataflowFilter {
    pub status: Option<DataflowStatus>,
    pub effective_status: Option<DataflowStatus>,
    pub search: Option<String>,
    pub source: Option<DataflowEndpointInput>,
    pub destination: Option<DataflowEndpointInput>,
    pub application_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub transport: Option<DataflowTransport>,
    pub tag_id: Option<Uuid>,
}
#[derive(Debug, Default)]
pub struct DataflowPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub status: Option<DataflowStatus>,
    pub sources: Option<Vec<DataflowEndpointInput>>,
    pub destinations: Option<Vec<DataflowEndpointInput>>,
    pub transport: Option<DataflowTransport>,
    pub destination_ports: Option<Option<Vec<i32>>>,
    pub source_ports: Option<Option<Vec<i32>>>,
    pub icmp_types: Option<Option<Vec<i32>>>,
    pub allow_cross_vrf: Option<bool>,
    pub group_id: Option<Option<Uuid>>,
    pub application_id: Option<Option<Uuid>>,
    pub owner_id: Option<Option<Uuid>>,
    pub tenant_id: Option<Option<Uuid>>,
    pub tags: Option<Vec<String>>,
    pub custom_field_data: Option<serde_json::Value>,
}
impl DataflowPatch {
    fn apply(self, flow: &mut Dataflow) -> (bool, bool) {
        if let Some(value) = self.name {
            flow.name = value;
        }
        if let Some(value) = self.description {
            flow.description = value;
        }
        if let Some(value) = self.comments {
            flow.comments = value;
        }
        if let Some(value) = self.status {
            flow.status = value;
        }
        let replace_sources = self.sources.is_some();
        if let Some(value) = self.sources {
            flow.sources = value;
        }
        let replace_destinations = self.destinations.is_some();
        if let Some(value) = self.destinations {
            flow.destinations = value;
        }
        if let Some(value) = self.transport {
            flow.transport = value;
        }
        if let Some(value) = self.destination_ports {
            flow.destination_ports = value;
        }
        if let Some(value) = self.source_ports {
            flow.source_ports = value;
        }
        if let Some(value) = self.icmp_types {
            flow.icmp_types = value;
        }
        if let Some(value) = self.allow_cross_vrf {
            flow.allow_cross_vrf = value;
        }
        if let Some(value) = self.group_id {
            flow.group_id = value;
        }
        if let Some(value) = self.application_id {
            flow.application_id = value;
        }
        if let Some(value) = self.owner_id {
            flow.owner_id = value;
        }
        if let Some(value) = self.tenant_id {
            flow.tenant_id = value;
        }
        if let Some(value) = self.tags {
            flow.tags = value;
        }
        if let Some(value) = self.custom_field_data {
            flow.custom_field_data = value;
        }
        (replace_sources, replace_destinations)
    }
}
#[derive(Clone)]
pub struct SqlxDataflowRepository {
    pool: PgPool,
}
impl SqlxDataflowRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn get(&self, id: Uuid) -> Result<Option<Dataflow>, AppError> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::<Postgres>::new(select_flows());
        query.push(" WHERE f.id = ").push_bind(id);
        let row = query
            .build_query_as::<DataflowRow>()
            .fetch_optional(&mut *connection)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(hydrate_flows(&mut connection, vec![row]).await?.pop())
    }
    pub async fn list(
        &self,
        filter: &DataflowFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<Dataflow>, i64), AppError> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::<Postgres>::new(select_flows());
        push_filters(&mut query, filter);
        query
            .push(" ORDER BY f.name, f.id LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = query
            .build_query_as::<DataflowRow>()
            .fetch_all(&mut *connection)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut count = QueryBuilder::<Postgres>::new("SELECT count(*) FROM inventory_dataflow f");
        push_filters(&mut count, filter);
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&mut *connection)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok((hydrate_flows(&mut connection, rows).await?, total))
    }
    pub async fn create(&self, flow: &Dataflow) -> Result<Dataflow, AppError> {
        let mut flow = flow.clone();
        for selector in flow.sources.iter_mut().chain(flow.destinations.iter_mut()) {
            selector.normalize()?;
        }
        if flow.sources.is_empty()
            || flow.destinations.is_empty()
            || flow.sources.len() > 32
            || flow.destinations.len() > 32
        {
            return Err(AppError::Validation(
                "sources and destinations must contain 1..32 endpoints".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let refs: Vec<_> = flow
            .sources
            .iter()
            .chain(flow.destinations.iter())
            .collect();
        let catalog = Catalog::load(&mut tx, &refs, CatalogPurpose::Mutation).await?;
        if flow.name.is_empty() {
            flow.name = format!(
                "{} → {}",
                catalog.label(&flow.sources[0]),
                catalog.label(&flow.destinations[0])
            )
            .chars()
            .take(60)
            .collect();
        }
        flow.normalize()?;
        validate_selectors(&mut tx, &catalog, &flow.sources).await?;
        validate_selectors(&mut tx, &catalog, &flow.destinations).await?;
        resolve_flow_application(&mut tx, &mut flow).await?;
        insert_flow(&mut tx, &flow).await?;
        insert_side(&mut tx, flow.id, "source", &flow.sources).await?;
        insert_side(&mut tx, flow.id, "destination", &flow.destinations).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.get(flow.id)
            .await?
            .ok_or_else(|| AppError::Internal("Created dataflow is missing".into()))
    }
    pub async fn update(&self, id: Uuid, patch: DataflowPatch) -> Result<Dataflow, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::<Postgres>::new(select_flows());
        query
            .push(" WHERE f.id = ")
            .push_bind(id)
            .push(" FOR UPDATE OF f");
        let row = query
            .build_query_as::<DataflowRow>()
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?
            .ok_or_else(|| AppError::NotFound("Dataflow not found".into()))?;
        let mut flow = hydrate_flows(&mut tx, vec![row])
            .await?
            .pop()
            .ok_or_else(|| AppError::Internal("Dataflow row is missing".into()))?;
        let (replace_sources, replace_destinations) = patch.apply(&mut flow);
        flow.normalize()?;
        let refs: Vec<_> = flow
            .sources
            .iter()
            .chain(flow.destinations.iter())
            .collect();
        let catalog = Catalog::load(&mut tx, &refs, CatalogPurpose::Mutation).await?;
        validate_selectors(&mut tx, &catalog, &flow.sources).await?;
        validate_selectors(&mut tx, &catalog, &flow.destinations).await?;
        resolve_flow_application(&mut tx, &mut flow).await?;
        update_flow(&mut tx, &flow).await?;
        if replace_sources {
            replace_side(&mut tx, id, "source", &flow.sources).await?;
        }
        if replace_destinations {
            replace_side(&mut tx, id, "destination", &flow.destinations).await?;
        }
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        self.get(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Dataflow not found".into()))
    }
    /// All-or-nothing status, move or delete over one locked flow set.
    pub async fn bulk_action(
        &self,
        ids: &[Uuid],
        action: &BulkDataflowAction,
    ) -> Result<Vec<Uuid>, AppError> {
        let mut unique: Vec<Uuid> = ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        if unique.is_empty() {
            return Err(AppError::Validation(
                "ids must contain at least one dataflow".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let locked: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM inventory_dataflow WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind(&unique)
        .fetch_all(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if locked.len() != unique.len() {
            return Err(AppError::Validation(
                "Bulk action references an unknown dataflow; nothing was changed".into(),
            ));
        }
        match action {
            BulkDataflowAction::SetStatus(status) => {
                sqlx::query("UPDATE inventory_dataflow SET status = $2 WHERE id = ANY($1)")
                    .bind(&unique)
                    .bind(status.as_str())
                    .execute(&mut *tx)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
            }
            BulkDataflowAction::Move {
                group_id,
                application_id,
            } => {
                let mut query = QueryBuilder::<Postgres>::new(select_flows());
                query
                    .push(" WHERE f.id = ANY(")
                    .push_bind(&unique)
                    .push(") ORDER BY f.id");
                let mut rows = query
                    .build_query_as::<DataflowRow>()
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
                if rows.len() != unique.len() {
                    return Err(AppError::Validation(
                        "Bulk move references an unknown dataflow; nothing was changed".into(),
                    ));
                }
                for row in rows.iter_mut() {
                    if let Some(group) = group_id {
                        row.group_id = *group;
                    }
                    if let Some(application) = application_id {
                        row.application_id = *application;
                    }
                }
                for row in rows {
                    let mut flow = hydrate_from_row(&row)?;
                    resolve_flow_application(&mut tx, &mut flow).await?;
                    sqlx::query(
                        "UPDATE inventory_dataflow SET group_id = $2, application_id = $3 WHERE id = $1",
                    )
                    .bind(flow.id)
                    .bind(flow.group_id)
                    .bind(flow.application_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
                }
            }
            BulkDataflowAction::Delete => {
                sqlx::query("DELETE FROM inventory_dataflow WHERE id = ANY($1)")
                    .bind(&unique)
                    .execute(&mut *tx)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
            }
        }
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(unique)
    }
    pub async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        let result = sqlx::query("DELETE FROM inventory_dataflow WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        if result.rows_affected() == 0 {
            return Err(AppError::NotFound("Dataflow not found".into()));
        }
        Ok(())
    }
    pub async fn projections(&self, flows: &[Dataflow]) -> Result<Vec<FlowProjection>, AppError> {
        if flows.is_empty() {
            return Ok(Vec::new());
        }
        let mut demanded: BTreeMap<EndpointType, BTreeSet<Uuid>> = BTreeMap::new();
        for selector in flows
            .iter()
            .flat_map(|flow| flow.sources.iter().chain(flow.destinations.iter()))
        {
            if let Some(identity) = selector.identity() {
                demanded
                    .entry(identity.object_type)
                    .or_default()
                    .insert(identity.object_id);
            }
        }
        let mut labels = BTreeMap::new();
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        for &(kind, _, _, table) in EndpointType::REFERENCES {
            let Some(ids) = demanded.get(&kind) else {
                continue;
            };
            let column = match kind {
                EndpointType::DeviceType => "model",
                EndpointType::IpAddress => "address::text",
                EndpointType::Prefix => "prefix::text",
                EndpointType::IpRange => "start_address::text || ' – ' || host(end_address)::text",
                _ => "name",
            };
            // Identifiers come only from the typed registry; endpoint IDs are bound.
            let mut query = QueryBuilder::<Postgres>::new("SELECT id, ");
            query
                .push(column)
                .push(" FROM ")
                .push(table)
                .push(" WHERE id = ANY(")
                .push_bind(ids.iter().copied().collect::<Vec<_>>())
                .push("::uuid[])");
            let rows = query
                .build_query_as::<(Uuid, Option<String>)>()
                .fetch_all(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
            for (id, label) in rows {
                labels.insert(
                    DataflowEndpoint {
                        object_type: kind,
                        object_id: id,
                    },
                    endpoint_label(kind, label),
                );
            }
        }
        let projection = |selector: &DataflowEndpointInput| {
            let label = if selector.object_type.is_builtin() {
                Some(endpoint_label(selector.object_type, None))
            } else {
                selector
                    .identity()
                    .and_then(|identity| labels.get(&identity).cloned())
            };
            endpoint_projection(selector, label)
        };
        Ok(flows
            .iter()
            .map(|flow| {
                (
                    flow.sources.iter().map(&projection).collect(),
                    flow.destinations.iter().map(&projection).collect(),
                )
            })
            .collect())
    }
    pub async fn policy_snapshot(
        &self,
        host: DataflowEndpoint,
    ) -> Result<PolicySnapshot, AppError> {
        if !host.object_type.is_host() {
            return Err(AppError::Validation(
                "host_type must be dcim.device or virtualization.virtualmachine".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut query = QueryBuilder::<Postgres>::new(select_flows());
        query
            .push(" WHERE ")
            .push(effective_status_expr())
            .push(" = 'active' ORDER BY f.id LIMIT ")
            .push_bind((MAX_ACTIVE_FLOWS + 1) as i64);
        let rows = query
            .build_query_as::<DataflowRow>()
            .fetch_all(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let flows = hydrate_flows(&mut tx, rows).await?;
        let selectors: BTreeSet<_> = flows
            .iter()
            .flat_map(|flow| flow.sources.iter().chain(flow.destinations.iter()))
            .collect();
        let selectors: Vec<_> = selectors.into_iter().collect();
        let catalog = Catalog::load(
            &mut tx,
            &selectors,
            CatalogPurpose::Policy {
                host: host.clone(),
                cross_vrf: flows.iter().any(|flow| flow.allow_cross_vrf),
            },
        )
        .await?;
        let host_row = catalog
            .hosts
            .get(&host)
            .ok_or_else(|| AppError::NotFound("Host not found".into()))?;
        let mut host_projection = host.projection();
        host_projection.label = host_row.name.clone();
        let host_membership = catalog.eligible_host_ips(&host);
        let mut errors = Vec::new();
        if flows.len() > MAX_ACTIVE_FLOWS || host_membership.len() > MAX_HOST_MEMBERSHIP {
            errors.push("Policy snapshot budget exceeded".into());
        }
        let mut endpoints = BTreeMap::new();
        for selector in flows
            .iter()
            .flat_map(|flow| flow.sources.iter().chain(flow.destinations.iter()))
        {
            endpoints
                .entry(selector.clone())
                .or_insert_with(|| catalog.resolve(selector));
        }
        let mut snapshot = PolicySnapshot {
            host,
            host_projection,
            host_membership,
            inventory_addresses: catalog
                .ips
                .values()
                .filter_map(|ip| ip.address.clone())
                .collect(),
            flows,
            endpoints,
            errors,
            authorization_facets: BTreeMap::new(),
        };
        snapshot.authorization_facets = crate::contexts::inventory::application::connection_target_authorization::resolve_typed_connection_target_authorization_facets(&mut tx, &snapshot.required_hosts()).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(snapshot)
    }
    pub async fn endpoint_resolution(
        &self,
        mut selector: DataflowEndpointInput,
    ) -> Result<
        crate::contexts::inventory::domain::native::dataflow::DataflowEndpointResolutionResponse,
        AppError,
    > {
        selector.normalize()?;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let catalog = Catalog::load(&mut tx, &[&selector], CatalogPurpose::Preview).await?;
        if !catalog.exists(&selector) {
            return Err(AppError::NotFound("Endpoint object not found".into()));
        }
        let preview = catalog.preview(&selector)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(preview)
    }
}

type FlowProjection = (Vec<DataflowEndpointResponse>, Vec<DataflowEndpointResponse>);

#[derive(Debug, Clone)]
pub enum BulkDataflowAction {
    SetStatus(DataflowStatus),
    Move {
        group_id: Option<Option<Uuid>>,
        application_id: Option<Option<Uuid>>,
    },
    Delete,
}

impl BulkDataflowAction {
    pub fn requires_delete_permission(&self) -> bool {
        matches!(self, Self::Delete)
    }
}

/// Effective state derived from the owning group ancestry, never stored.
fn effective_status_expr() -> &'static str {
    "CASE
       WHEN f.status = 'disabled' THEN 'disabled'
       WHEN COALESCE((WITH RECURSIVE ancestors AS (
           SELECT g.id, g.parent_id, g.status FROM inventory_dataflow_group g WHERE g.id = f.group_id
           UNION ALL
           SELECT p.id, p.parent_id, p.status FROM inventory_dataflow_group p JOIN ancestors a ON p.id = a.parent_id
         ) SELECT bool_or(status = 'disabled') FROM ancestors), false) THEN 'disabled'
       WHEN f.status = 'draft' THEN 'draft'
       WHEN COALESCE((WITH RECURSIVE ancestors AS (
           SELECT g.id, g.parent_id, g.status FROM inventory_dataflow_group g WHERE g.id = f.group_id
           UNION ALL
           SELECT p.id, p.parent_id, p.status FROM inventory_dataflow_group p JOIN ancestors a ON p.id = a.parent_id
         ) SELECT bool_or(status = 'draft') FROM ancestors), false) THEN 'draft'
       ELSE 'active'
     END"
}

async fn hydrate_flows(
    connection: &mut PgConnection,
    rows: Vec<DataflowRow>,
) -> Result<Vec<Dataflow>, AppError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut groups = BTreeMap::new();
    {
        let group_rows = sqlx::query_as::<_, GroupRow>(
            "SELECT id, parent_id, status, tags FROM inventory_dataflow_group ORDER BY id LIMIT 4097",
        )
        .fetch_all(&mut *connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if group_rows.len() > 4096 {
            return Err(AppError::TooManyRequests(
                "Dataflow group hierarchy budget exceeded".into(),
            ));
        }
        groups.extend(group_rows.into_iter().map(|row| (row.id, row)));
    }
    let selector_rows = sqlx::query_as::<_, SelectorRow>(
        "SELECT id, dataflow_id, side, object_type, address_mode, tag_object_type, device_id, vm_id, service_id, ipaddress_id, prefix_id, iprange_id, tag_id, addressgroup_id, region_id, site_id, sitegroup_id, devicerole_id, tenant_id, tenantgroup_id, platform_id, manufacturer_id, devicetype_id, rack_id, location_id, cluster_id, clustergroup_id, clustertype_id FROM inventory_dataflow_selector WHERE dataflow_id = ANY($1) ORDER BY dataflow_id, side, position",
    )
    .bind(&ids)
    .fetch_all(&mut *connection)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let selected_rows = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT s.id, ip.ipaddress_id FROM inventory_dataflow_selector s JOIN inventory_dataflow_selector_ip ip ON ip.selector_id = s.id WHERE s.dataflow_id = ANY($1) ORDER BY s.id, ip.ipaddress_id",
    )
    .bind(&ids)
    .fetch_all(&mut *connection)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let mut selector_ip_by_id: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
    for (selector_id, ipaddress_id) in selected_rows {
        selector_ip_by_id
            .entry(selector_id)
            .or_default()
            .push(ipaddress_id);
    }
    let mut mapped: BTreeMap<Uuid, (Vec<DataflowEndpointInput>, Vec<DataflowEndpointInput>)> =
        BTreeMap::new();
    for row in selector_rows {
        let selector_id = row.id;
        let dataflow_id = row
            .dataflow_id
            .ok_or_else(|| AppError::Database("Flow selector has no owner".into()))?;
        let side = row.side.clone();
        let mut input = row.into_input()?;
        input.ipaddresses = selector_ip_by_id.remove(&selector_id).unwrap_or_default();
        let entry = mapped.entry(dataflow_id).or_default();
        if side == "source" {
            entry.0.push(input);
        } else {
            entry.1.push(input);
        }
    }
    let mut flows = Vec::with_capacity(rows.len());
    for row in rows {
        let (sources, destinations) = mapped.remove(&row.id).unwrap_or_default();
        if sources.is_empty() || destinations.is_empty() {
            return Err(AppError::Database(
                "Dataflow selector set is incomplete".into(),
            ));
        }
        let mut flow = hydrate_from_row(&row)?;
        flow.sources = sources;
        flow.destinations = destinations;
        let ancestors = group_ancestry(&groups, flow.group_id);
        let (effective_status, blocked_by_group_id) = effective_state(flow.status, &ancestors)?;
        flow.effective_status = effective_status;
        flow.blocked_by_group_id = blocked_by_group_id;
        let mut inherited: BTreeSet<Uuid> = BTreeSet::new();
        for group in &ancestors {
            inherited.extend(group.tags.iter().filter_map(|tag| tag_uuid(tag)));
        }
        let mut effective: BTreeSet<Uuid> =
            flow.tags.iter().filter_map(|tag| tag_uuid(tag)).collect();
        effective.extend(inherited.iter().copied());
        flow.inherited_tags = inherited.into_iter().collect();
        flow.effective_tags = effective.into_iter().collect();
        flows.push(flow);
    }
    Ok(flows)
}

fn hydrate_from_row(row: &DataflowRow) -> Result<Dataflow, AppError> {
    Ok(Dataflow {
        id: row.id,
        name: row.name.clone(),
        description: row.description.clone(),
        comments: row.comments.clone(),
        status: parse_status(&row.status)?,
        sources: Vec::new(),
        destinations: Vec::new(),
        transport: parse_transport(&row.transport)?,
        destination_ports: row.destination_ports.clone(),
        source_ports: row.source_ports.clone(),
        icmp_types: row.icmp_types.clone(),
        allow_cross_vrf: row.allow_cross_vrf,
        group_id: row.group_id,
        application_id: row.application_id,
        owner_id: row.owner_id,
        tenant_id: row.tenant_id,
        tags: row.tags.clone(),
        custom_field_data: row.custom_field_data.clone(),
        effective_status: parse_status(&row.status)?,
        blocked_by_group_id: None,
        inherited_tags: Vec::new(),
        effective_tags: Vec::new(),
    })
}
fn push_filters(query: &mut QueryBuilder<Postgres>, filter: &DataflowFilter) {
    query.push(" WHERE true");
    if let Some(status) = filter.status {
        query.push(" AND f.status = ").push_bind(status.as_str());
    }
    if let Some(effective) = filter.effective_status {
        query
            .push(" AND ")
            .push(effective_status_expr())
            .push(" = ")
            .push_bind(effective.as_str());
    }
    if let Some(search) = &filter.search {
        query
            .push(" AND (f.name ILIKE ")
            .push_bind(search)
            .push(" OR f.description ILIKE ")
            .push_bind(search)
            .push(" OR f.comments ILIKE ")
            .push_bind(search)
            .push(")");
    }
    if let Some(application_id) = filter.application_id {
        query
            .push(" AND f.application_id = ")
            .push_bind(application_id);
    }
    if let Some(transport) = filter.transport {
        query
            .push(" AND f.transport = ")
            .push_bind(transport.as_str());
    }
    if let Some(group_id) = filter.group_id {
        query
            .push(" AND f.group_id IN (WITH RECURSIVE descendants AS (SELECT id FROM inventory_dataflow_group WHERE id = ")
            .push_bind(group_id)
            .push(" UNION SELECT child.id FROM inventory_dataflow_group child JOIN descendants parent ON child.parent_id = parent.id) SELECT id FROM descendants)");
    }
    if let Some(tag_id) = filter.tag_id {
        let tag = tag_id.to_string();
        query
            .push(" AND (f.tags @> ARRAY[")
            .push_bind(tag.clone())
            .push("]::text[] OR EXISTS (WITH RECURSIVE ancestors AS (SELECT g.id, g.parent_id, g.tags FROM inventory_dataflow_group g WHERE g.id = f.group_id UNION ALL SELECT p.id, p.parent_id, p.tags FROM inventory_dataflow_group p JOIN ancestors a ON p.id = a.parent_id) SELECT 1 FROM ancestors WHERE tags @> ARRAY[")
            .push_bind(tag)
            .push("]::text[]))");
    }
    for (side, selector) in [
        ("source", &filter.source),
        ("destination", &filter.destination),
    ] {
        if let Some(selector) = selector {
            query
                .push(" AND EXISTS (SELECT 1 FROM inventory_dataflow_selector s WHERE s.dataflow_id = f.id AND s.side = ")
                .push_bind(side)
                .push(" AND s.object_type = ")
                .push_bind(selector.object_type.as_str());
            if !selector.object_type.is_builtin() {
                if let Some(id) = selector.object_id {
                    query
                        .push(" AND s.")
                        .push(selector.object_type.column())
                        .push(" = ")
                        .push_bind(id);
                }
            }
            if selector.object_type.is_tag() {
                if let Some(scope) = selector.tag_object_type {
                    query
                        .push(" AND s.tag_object_type = ")
                        .push_bind(scope.as_str());
                }
            }
            query.push(")");
        }
    }
}

async fn insert_flow(connection: &mut PgConnection, flow: &Dataflow) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO inventory_dataflow (id, name, description, comments, status, transport, destination_ports, source_ports, icmp_types, allow_cross_vrf, application_id, group_id, owner_id, tenant_id, tags, custom_field_data) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
    )
    .bind(flow.id)
    .bind(&flow.name)
    .bind(&flow.description)
    .bind(&flow.comments)
    .bind(flow.status.as_str())
    .bind(flow.transport.as_str())
    .bind(&flow.destination_ports)
    .bind(&flow.source_ports)
    .bind(&flow.icmp_types)
    .bind(flow.allow_cross_vrf)
    .bind(flow.application_id)
    .bind(flow.group_id)
    .bind(flow.owner_id)
    .bind(flow.tenant_id)
    .bind(&flow.tags)
    .bind(&flow.custom_field_data)
    .execute(&mut *connection)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

async fn update_flow(connection: &mut PgConnection, flow: &Dataflow) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE inventory_dataflow SET name=$2, description=$3, comments=$4, status=$5, transport=$6, destination_ports=$7, source_ports=$8, icmp_types=$9, allow_cross_vrf=$10, application_id=$11, group_id=$12, owner_id=$13, tenant_id=$14, tags=$15, custom_field_data=$16 WHERE id=$1",
    )
    .bind(flow.id)
    .bind(&flow.name)
    .bind(&flow.description)
    .bind(&flow.comments)
    .bind(flow.status.as_str())
    .bind(flow.transport.as_str())
    .bind(&flow.destination_ports)
    .bind(&flow.source_ports)
    .bind(&flow.icmp_types)
    .bind(flow.allow_cross_vrf)
    .bind(flow.application_id)
    .bind(flow.group_id)
    .bind(flow.owner_id)
    .bind(flow.tenant_id)
    .bind(&flow.tags)
    .bind(&flow.custom_field_data)
    .execute(&mut *connection)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

async fn insert_side(
    connection: &mut PgConnection,
    flow_id: Uuid,
    side: &str,
    selectors: &[DataflowEndpointInput],
) -> Result<(), AppError> {
    for (index, selector) in selectors.iter().enumerate() {
        insert_selector(
            connection,
            Some(flow_id),
            None,
            side,
            i16::try_from(index + 1)
                .map_err(|_| AppError::Validation("Too many selectors".into()))?,
            selector,
        )
        .await?;
    }
    Ok(())
}

async fn replace_side(
    connection: &mut PgConnection,
    flow_id: Uuid,
    side: &str,
    selectors: &[DataflowEndpointInput],
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM inventory_dataflow_selector WHERE dataflow_id = $1 AND side = $2")
        .bind(flow_id)
        .bind(side)
        .execute(&mut *connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
    insert_side(connection, flow_id, side, selectors).await
}

fn push_selector_columns(query: &mut QueryBuilder<Postgres>) {
    for &(kind, _, _, _) in EndpointType::REFERENCES {
        query.push(", ").push(kind.column());
    }
}

async fn insert_selector(
    connection: &mut PgConnection,
    flow_id: Option<Uuid>,
    address_group_owner_id: Option<Uuid>,
    side: &str,
    position: i16,
    selector: &DataflowEndpointInput,
) -> Result<(), AppError> {
    let id = Uuid::now_v7();
    let mut query = QueryBuilder::<Postgres>::new(
        "INSERT INTO inventory_dataflow_selector (id, dataflow_id, group_id, side, position, object_type, address_mode, tag_object_type",
    );
    push_selector_columns(&mut query);
    query.push(") VALUES (");
    let mut values = query.separated(", ");
    values
        .push_bind(id)
        .push_bind(flow_id)
        .push_bind(address_group_owner_id)
        .push_bind(side)
        .push_bind(position)
        .push_bind(selector.object_type.as_str())
        .push_bind(selector.address_mode.as_str())
        .push_bind(selector.tag_object_type.map(EndpointType::as_str));
    for &(kind, _, _, _) in EndpointType::REFERENCES {
        values.push_bind(if selector.object_type == kind {
            selector.object_id
        } else {
            None
        });
    }
    query.push(")");
    query
        .build()
        .execute(&mut *connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
    if !selector.ipaddresses.is_empty() {
        sqlx::query(
            "INSERT INTO inventory_dataflow_selector_ip (selector_id, ipaddress_id) SELECT $1, unnest($2::uuid[])",
        )
        .bind(id)
        .bind(&selector.ipaddresses)
        .execute(&mut *connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }
    Ok(())
}

async fn resolve_flow_application(
    connection: &mut PgConnection,
    flow: &mut Dataflow,
) -> Result<(), AppError> {
    let Some(group_id) = flow.group_id else {
        return Ok(());
    };
    let group_application: Option<Uuid> =
        sqlx::query_scalar("SELECT application_id FROM inventory_dataflow_group WHERE id = $1")
            .bind(group_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(classify_sqlx_error_owned)?
            .ok_or_else(|| AppError::NotFound("Dataflow group not found".into()))?;
    match flow.application_id {
        Some(application) if Some(application) != group_application => Err(AppError::Validation(
            "Flow application must match its group application".into(),
        )),
        None => {
            flow.application_id = group_application;
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn validate_selectors(
    connection: &mut PgConnection,
    catalog: &Catalog,
    selectors: &[DataflowEndpointInput],
) -> Result<(), AppError> {
    for selector in selectors {
        if !selector.object_type.is_builtin() && !catalog.exists(selector) {
            return Err(AppError::Validation(
                "Endpoint object does not exist".into(),
            ));
        }
        if selector.address_mode != DataflowAddressMode::Selected {
            continue;
        }
        let locked: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM ipam_ipaddress WHERE id = ANY($1) ORDER BY id FOR SHARE",
        )
        .bind(&selector.ipaddresses)
        .fetch_all(&mut *connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if locked.len() != selector.ipaddresses.len() {
            return Err(AppError::Validation("Selected IP does not exist".into()));
        }
        let resolved = catalog.resolve(selector);
        if !resolved.errors.is_empty() || resolved.addresses.len() != selector.ipaddresses.len() {
            return Err(AppError::Validation("Selected IPs must be eligible members of the referenced object and belong to its host".into()));
        }
    }
    Ok(())
}

/// Validate a typed selector set at a catalog mutation boundary.
///
/// Address-group members are the same selector contract as flow sides, so they
/// reuse this snapshot and eligibility owner instead of resolving selected IPs
/// again. A second resolver would drift from flow create/update validation.
///
/// # Errors
///
/// [`AppError::Validation`] for a missing reference, a missing selected IP or a
/// selected IP that does not belong to the referenced host.
pub(super) async fn validate_mutation_selectors(
    connection: &mut PgConnection,
    selectors: &[&DataflowEndpointInput],
) -> Result<(), AppError> {
    let catalog = Catalog::load(&mut *connection, selectors, CatalogPurpose::Mutation).await?;
    for selector in selectors {
        validate_selectors(&mut *connection, &catalog, std::slice::from_ref(*selector)).await?;
    }
    Ok(())
}

#[derive(Debug, FromRow)]
struct HostRow {
    kind: String,
    id: Uuid,
    name: Option<String>,
    primary_ip4_id: Option<Uuid>,
    primary_ip6_id: Option<Uuid>,
    oob_ip_id: Option<Uuid>,
    tags: Vec<String>,
    site_id: Option<Uuid>,
    role_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    platform_id: Option<Uuid>,
    device_type_id: Option<Uuid>,
    rack_id: Option<Uuid>,
    location_id: Option<Uuid>,
    cluster_id: Option<Uuid>,
}
#[derive(Debug, FromRow)]
struct ScopeRow {
    id: Uuid,
    label: String,
    parent_id: Option<Uuid>,
    region_id: Option<Uuid>,
    group_id: Option<Uuid>,
    type_id: Option<Uuid>,
    manufacturer_id: Option<Uuid>,
}
impl ScopeRow {
    async fn load(
        kind: EndpointType,
        table: &str,
        ids: &[Uuid],
        descendants: bool,
        connection: &mut PgConnection,
    ) -> Result<BTreeMap<Uuid, Self>, AppError> {
        let label = if kind == EndpointType::DeviceType {
            "model"
        } else {
            "name"
        };
        let parent = if kind.is_hierarchical() {
            "parent_id"
        } else {
            "NULL::uuid"
        };
        let region = if kind == EndpointType::Site {
            "region_id"
        } else {
            "NULL::uuid"
        };
        let group = if matches!(
            kind,
            EndpointType::Site | EndpointType::Tenant | EndpointType::Cluster
        ) {
            "group_id"
        } else {
            "NULL::uuid"
        };
        let cluster_type = if kind == EndpointType::Cluster {
            "type_id"
        } else {
            "NULL::uuid"
        };
        let manufacturer = if kind == EndpointType::DeviceType {
            "manufacturer_id"
        } else {
            "NULL::uuid"
        };
        // UNION (not UNION ALL) terminates corrupt cycles; scope_members reports
        // them from retained parent links rather than accepting partial membership.
        let mut query = QueryBuilder::<Postgres>::new("");
        let recursive = descendants && kind.is_hierarchical();
        if recursive {
            query
                .push("WITH RECURSIVE wanted AS (SELECT id FROM ")
                .push(table)
                .push(" WHERE id = ANY(")
                .push_bind(ids)
                .push(") UNION SELECT child.id FROM ")
                .push(table)
                .push(" child JOIN wanted parent ON child.parent_id = parent.id) ");
        }
        query
            .push("SELECT id,")
            .push(label)
            .push(" AS label,")
            .push(parent)
            .push(" AS parent_id,")
            .push(region)
            .push(" AS region_id,")
            .push(group)
            .push(" AS group_id,")
            .push(cluster_type)
            .push(" AS type_id,")
            .push(manufacturer)
            .push(" AS manufacturer_id FROM ")
            .push(table)
            .push(" WHERE ");
        if recursive {
            query.push("id IN (SELECT id FROM wanted)");
        } else {
            query.push("id = ANY(").push_bind(ids).push(")");
        }
        query.push(" LIMIT 4097");
        let rows = query
            .build_query_as::<Self>()
            .fetch_all(connection)
            .await
            .map_err(classify_sqlx_error_owned)?;
        if rows.len() > 4096 {
            return Err(AppError::TooManyRequests(
                "Scope catalog budget exceeded; no partial membership".into(),
            ));
        }
        Ok(rows.into_iter().map(|row| (row.id, row)).collect())
    }
}
#[derive(Debug, FromRow)]
struct ServiceRow {
    id: Uuid,
    name: String,
    parent_kind: String,
    parent_id: Uuid,
    bindings: Vec<Uuid>,
}
#[derive(Debug, FromRow)]
struct IpRow {
    id: Uuid,
    address: String,
    status: String,
    vrf_id: Option<Uuid>,
    assigned_object_id: Option<Uuid>,
    assigned_kind: Option<String>,
    assigned_host_id: Option<Uuid>,
}
#[derive(Debug, FromRow)]
struct PrefixRow {
    id: Uuid,
    prefix: String,
    status: String,
    vrf_id: Option<Uuid>,
}
#[derive(Debug, FromRow)]
struct RangeRow {
    id: Uuid,
    start_address: String,
    end_address: String,
    status: String,
    vrf_id: Option<Uuid>,
}
const AMBIGUOUS_ADDRESS_OWNERSHIP: &str = "Ambiguous address ownership in the same routing context";

#[derive(Debug)]
struct CatalogIp {
    row: IpRow,
    owners: BTreeSet<DataflowEndpoint>,
    address: Option<PolicyAddress>,
    errors: Vec<String>,
    assigned_owner: Option<DataflowEndpoint>,
}
struct Catalog {
    scopes: BTreeMap<EndpointType, BTreeMap<Uuid, ScopeRow>>,
    hosts: BTreeMap<DataflowEndpoint, HostRow>,
    services: BTreeMap<Uuid, ServiceRow>,
    ips: BTreeMap<Uuid, CatalogIp>,
    prefixes: BTreeMap<Uuid, PrefixRow>,
    ranges: BTreeMap<Uuid, RangeRow>,
    tags: BTreeMap<Uuid, String>,
    address_groups: BTreeMap<Uuid, (String, Vec<DataflowEndpointInput>)>,
}
impl Catalog {
    fn scope_members(
        &self,
        selector: &DataflowEndpointInput,
        errors: &mut Vec<String>,
    ) -> BTreeSet<DataflowEndpoint> {
        let kind = selector.object_type;
        let Some((root, rows)) = selector.object_id.zip(self.scopes.get(&kind)) else {
            errors.push("Scope requires a supported kind and object_id".into());
            return BTreeSet::new();
        };
        if !rows.contains_key(&root) {
            errors.push("Scope object is missing".into());
            return BTreeSet::new();
        }
        let mut ids = BTreeSet::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if !ids.insert(id) {
                errors.push("Selected scope hierarchy contains a cycle".into());
                continue;
            }
            if kind.is_hierarchical() {
                pending.extend(
                    rows.values()
                        .filter(|row| row.parent_id == Some(id))
                        .map(|row| row.id),
                );
            }
            if ids.len() > 4096 || pending.len() > 4096 {
                errors.push("Scope hierarchy budget exceeded".into());
                break;
            }
        }
        let related = |kind, id: Option<Uuid>| id.and_then(|id| self.scopes.get(&kind)?.get(&id));
        self.hosts
            .iter()
            .filter_map(|(host, row)| {
                let facet = match kind {
                    EndpointType::Region => {
                        related(EndpointType::Site, row.site_id).and_then(|row| row.region_id)
                    }
                    EndpointType::Site => row.site_id,
                    EndpointType::SiteGroup => {
                        related(EndpointType::Site, row.site_id).and_then(|row| row.group_id)
                    }
                    EndpointType::DeviceRole => row.role_id,
                    EndpointType::Tenant => row.tenant_id,
                    EndpointType::TenantGroup => {
                        related(EndpointType::Tenant, row.tenant_id).and_then(|row| row.group_id)
                    }
                    EndpointType::Platform => row.platform_id,
                    EndpointType::Manufacturer => {
                        related(EndpointType::DeviceType, row.device_type_id)
                            .and_then(|row| row.manufacturer_id)
                    }
                    EndpointType::DeviceType => row.device_type_id,
                    EndpointType::Rack => row.rack_id,
                    EndpointType::Location => row.location_id,
                    EndpointType::Cluster => row.cluster_id,
                    EndpointType::ClusterGroup => {
                        related(EndpointType::Cluster, row.cluster_id).and_then(|row| row.group_id)
                    }
                    EndpointType::ClusterType => {
                        related(EndpointType::Cluster, row.cluster_id).and_then(|row| row.type_id)
                    }
                    _ => None,
                };
                facet.filter(|id| ids.contains(id)).map(|_| host.clone())
            })
            .collect()
    }
    fn scope_candidates(
        &self,
        selector: &DataflowEndpointInput,
        result: &mut ResolvedEndpoint,
    ) -> Vec<Uuid> {
        let members = self.scope_members(selector, &mut result.errors);
        let mut ids = BTreeSet::new();
        for host in members {
            ids.extend(self.host_candidates(&DataflowEndpointInput {
                object_type: host.object_type,
                object_id: Some(host.object_id),
                address_mode: if selector.address_mode == DataflowAddressMode::Default {
                    DataflowAddressMode::Default
                } else {
                    DataflowAddressMode::All
                },
                ipaddresses: Vec::new(),
                tag_object_type: None,
            }));
        }
        ids.into_iter().collect()
    }
    fn resolve_scope(&self, selector: &DataflowEndpointInput) -> ResolvedEndpoint {
        let mut result = ResolvedEndpoint::default();
        let members = self.scope_members(selector, &mut result.errors);
        let candidates = self.scope_candidates(selector, &mut result);
        if selector.address_mode == DataflowAddressMode::Selected {
            for &id in &selector.ipaddresses {
                self.append_address(selector, id, &candidates, false, &mut result);
            }
        } else {
            for host in &members {
                let host_selector = DataflowEndpointInput {
                    object_type: host.object_type,
                    object_id: Some(host.object_id),
                    address_mode: selector.address_mode,
                    ipaddresses: Vec::new(),
                    tag_object_type: None,
                };
                let host_candidates = self.host_candidates(&host_selector);
                for &id in &host_candidates {
                    self.append_address(&host_selector, id, &host_candidates, true, &mut result);
                }
            }
        }
        result.exclusions += members.difference(&result.owners).count();
        if !result.errors.is_empty() {
            result.owners.extend(members);
        }
        if result.addresses.len() > MAX_ALL_ADDRESSES {
            result.errors.push("Scope address budget exceeded".into());
        }
        result
    }
    fn member_hosts(
        &self,
        selector: &DataflowEndpointInput,
    ) -> Result<BTreeSet<DataflowEndpoint>, String> {
        if selector.object_type.is_scope() {
            return Ok(self.scope_members(selector, &mut Vec::new()));
        }
        if selector.object_type == EndpointType::All {
            return Ok(self.hosts.keys().cloned().collect());
        }
        if selector.object_type.is_host() {
            return Ok(selector.identity().into_iter().collect());
        }
        if selector.object_type == EndpointType::Internet {
            return Ok(BTreeSet::new());
        }
        if selector.object_type.is_tag() {
            let (Some(tag_id), Some(scope)) = (selector.object_id, selector.tag_object_type) else {
                return Ok(BTreeSet::new());
            };
            return Ok(self
                .hosts
                .iter()
                .filter(|(host, row)| {
                    host.object_type == scope
                        && row.tags.iter().any(|tag| tag_uuid(tag) == Some(tag_id))
                })
                .map(|(host, _)| host.clone())
                .collect());
        }
        if selector.object_type.is_range() || selector.object_type.is_address_group() {
            return Ok(self.resolve(selector).owners);
        }
        let mut resolved = ResolvedEndpoint::default();
        let ids = self.candidates(selector, &mut resolved);
        for id in ids {
            if let Some(ip) = self.ips.get(&id) {
                resolved.owners.extend(ip.owners.iter().cloned());
            }
        }
        Ok(resolved.owners)
    }
    fn preview(
        &self,
        selector: &DataflowEndpointInput,
    ) -> Result<
        crate::contexts::inventory::domain::native::dataflow::DataflowEndpointResolutionResponse,
        AppError,
    > {
        use crate::contexts::inventory::domain::native::dataflow::{
            DataflowEndpointAddressResponse, DataflowEndpointMemberResponse,
            DataflowEndpointResolutionResponse, DataflowResolutionDiagnostic,
            DataflowResolvedAddressResponse,
        };
        let effective = self.resolve(selector);
        let mut default = selector.clone();
        default.address_mode = DataflowAddressMode::Default;
        default.ipaddresses.clear();
        let defaults = self.resolve(&default);
        let mut choices = default.clone();
        // Selectable membership can exceed Service default/all assigned-only fallback.
        choices.address_mode = DataflowAddressMode::Selected;
        let candidate_ids = if choices.object_type == EndpointType::All {
            self.ips
                .values()
                .filter(|ip| !ip.owners.is_empty())
                .map(|ip| ip.row.id)
                .collect()
        } else {
            self.candidates(&choices, &mut ResolvedEndpoint::default())
        };
        let eligible: Vec<_> = candidate_ids
            .into_iter()
            .filter_map(|id| self.ips.get(&id))
            .filter(|ip| ip.errors.is_empty())
            .collect();
        let mut default_ipaddresses: Vec<_> = eligible
            .iter()
            .filter(|ip| {
                ip.address.as_ref().is_some_and(|address| {
                    defaults.addresses.iter().any(|chosen| {
                        chosen.network == address.network && chosen.vrf_id == address.vrf_id
                    })
                })
            })
            .map(|ip| ip.row.id)
            .collect();
        default_ipaddresses.sort_unstable();
        default_ipaddresses.dedup();
        let ipaddresses = eligible
            .into_iter()
            .map(|ip| DataflowEndpointAddressResponse {
                id: ip.row.id,
                address: ip.row.address.clone(),
                vrf_id: ip.row.vrf_id,
            })
            .collect();
        let normalized = |values: Vec<&PolicyAddress>| {
            let mut result: Vec<_> = values
                .into_iter()
                .map(|address| DataflowResolvedAddressResponse {
                    cidr: address.network.cidr(),
                    vrf_id: address.vrf_id,
                })
                .collect();
            result.sort();
            result.dedup();
            result
        };
        let diagnostic = |message: String| DataflowResolutionDiagnostic {
            code: "endpoint_unresolved".into(),
            message,
        };
        let mut diagnostics: Vec<_> = effective
            .errors
            .iter()
            .chain(
                effective
                    .addresses
                    .iter()
                    .flat_map(|address| &address.errors),
            )
            .cloned()
            .map(diagnostic)
            .collect();
        if effective.exclusions > 0 {
            diagnostics.push(DataflowResolutionDiagnostic {
                code: "inventory_excluded".into(),
                message: "Some inventory members have no eligible chosen addresses".into(),
            });
        }
        let member_hosts = match self.member_hosts(selector) {
            Ok(hosts) => hosts,
            Err(error) => {
                diagnostics.push(diagnostic(error));
                BTreeSet::new()
            }
        };
        let mut members = Vec::new();
        for host in member_hosts {
            let chosen: Vec<_> = if selector.object_type == EndpointType::Prefix {
                self.ips
                    .values()
                    .filter(|ip| {
                        ip.owners.contains(&host)
                            && ip
                                .errors
                                .iter()
                                .all(|error| error == AMBIGUOUS_ADDRESS_OWNERSHIP)
                    })
                    .filter_map(|ip| ip.address.as_ref())
                    .filter(|ip| {
                        effective.addresses.iter().any(|prefix| {
                            prefix.vrf_id == ip.vrf_id && prefix.network.contains(&ip.network)
                        })
                    })
                    .collect()
            } else {
                effective
                    .addresses
                    .iter()
                    .filter(|address| address.owners.contains(&host))
                    .collect()
            };
            let mut member_diagnostics: Vec<_> = chosen
                .iter()
                .flat_map(|address| &address.errors)
                .cloned()
                .map(diagnostic)
                .collect();
            if chosen.is_empty() {
                member_diagnostics.push(DataflowResolutionDiagnostic {
                    code: "inventory_excluded".into(),
                    message: "Member has no eligible addresses chosen by this selector".into(),
                });
            }
            let mut projection = host.projection();
            projection.label = self.hosts.get(&host).and_then(|row| row.name.clone());
            member_diagnostics.sort();
            member_diagnostics.dedup();
            members.push(DataflowEndpointMemberResponse {
                host: projection,
                addresses: normalized(chosen),
                diagnostics: member_diagnostics,
            });
        }
        diagnostics.sort();
        diagnostics.dedup();
        Ok(DataflowEndpointResolutionResponse {
            endpoint: self.projection(selector),
            members,
            ipaddresses,
            default_ipaddresses,
            addresses: normalized(effective.addresses.iter().collect()),
            diagnostics,
        })
    }
    fn exists(&self, selector: &DataflowEndpointInput) -> bool {
        match selector.object_type {
            EndpointType::Device | EndpointType::VirtualMachine => selector
                .identity()
                .is_some_and(|host| self.hosts.contains_key(&host)),
            EndpointType::Service => selector
                .object_id
                .is_some_and(|id| self.services.contains_key(&id)),
            EndpointType::IpAddress => selector
                .object_id
                .is_some_and(|id| self.ips.contains_key(&id)),
            EndpointType::Prefix => selector
                .object_id
                .is_some_and(|id| self.prefixes.contains_key(&id)),
            EndpointType::IpRange => selector
                .object_id
                .is_some_and(|id| self.ranges.contains_key(&id)),
            EndpointType::Tag => selector
                .object_id
                .is_some_and(|id| self.tags.contains_key(&id)),
            EndpointType::AddressGroup => selector
                .object_id
                .is_some_and(|id| self.address_groups.contains_key(&id)),
            EndpointType::All | EndpointType::Internet => true,
            kind => selector.object_id.is_some_and(|id| {
                self.scopes
                    .get(&kind)
                    .is_some_and(|rows| rows.contains_key(&id))
            }),
        }
    }
    fn label(&self, selector: &DataflowEndpointInput) -> String {
        let label = match selector.object_type {
            EndpointType::All | EndpointType::Internet => None,
            EndpointType::Device | EndpointType::VirtualMachine => selector
                .identity()
                .and_then(|host| self.hosts.get(&host))
                .and_then(|row| row.name.clone()),
            EndpointType::Service => selector
                .object_id
                .and_then(|id| self.services.get(&id))
                .map(|row| row.name.clone()),
            EndpointType::IpAddress => selector
                .object_id
                .and_then(|id| self.ips.get(&id))
                .map(|ip| ip.row.address.clone()),
            EndpointType::Prefix => selector
                .object_id
                .and_then(|id| self.prefixes.get(&id))
                .map(|row| row.prefix.clone()),
            EndpointType::IpRange => selector
                .object_id
                .and_then(|id| self.ranges.get(&id))
                .map(|row| format!("{} – {}", row.start_address, row.end_address)),
            EndpointType::Tag => selector
                .object_id
                .and_then(|id| self.tags.get(&id).cloned()),
            EndpointType::AddressGroup => selector
                .object_id
                .and_then(|id| self.address_groups.get(&id))
                .map(|(name, _)| name.clone()),
            kind => selector
                .object_id
                .and_then(|id| self.scopes.get(&kind)?.get(&id))
                .map(|row| row.label.clone()),
        };
        endpoint_label(selector.object_type, label)
    }
    fn projection(&self, selector: &DataflowEndpointInput) -> DataflowEndpointResponse {
        endpoint_projection(
            selector,
            self.exists(selector).then(|| self.label(selector)),
        )
    }
    fn eligible_host_ips(&self, host: &DataflowEndpoint) -> Vec<PolicyAddress> {
        self.ips
            .values()
            .filter(|ip| ip.row.status == "active")
            .filter_map(|ip| ip.address.as_ref())
            .filter(|ip| ip.owners.contains(host))
            .cloned()
            .collect()
    }
    fn resolve(&self, selector: &DataflowEndpointInput) -> ResolvedEndpoint {
        if selector.object_type.is_scope() {
            return self.resolve_scope(selector);
        }
        if selector.object_type == EndpointType::Internet {
            return ResolvedEndpoint {
                addresses: ["0.0.0.0/0", "::/0"]
                    .into_iter()
                    .filter_map(|cidr| PolicyNetwork::parse(cidr, false).ok())
                    .map(|network| PolicyAddress {
                        network,
                        vrf_id: None,
                        owners: BTreeSet::new(),
                        errors: Vec::new(),
                    })
                    .collect(),
                ..Default::default()
            };
        }
        if selector.object_type == EndpointType::All {
            return self.resolve_all();
        }
        if selector.object_type.is_range() {
            return self.resolve_range(selector);
        }
        if selector.object_type.is_tag() {
            return self.resolve_tag(selector);
        }
        if selector.object_type.is_address_group() {
            return self.resolve_address_group(selector);
        }
        let mut result = ResolvedEndpoint::default();
        if let Some(identity) = selector
            .identity()
            .filter(|identity| identity.object_type.is_host())
        {
            result.owners.insert(identity);
        }
        if !self.exists(selector) {
            result.errors.push("Endpoint object is missing".into());
            return result;
        }
        if selector.object_type == EndpointType::Prefix
            && selector.address_mode == DataflowAddressMode::Default
        {
            return self.resolve_prefix(selector);
        }
        let candidates = self.candidates(selector, &mut result);
        let ids = if selector.address_mode == DataflowAddressMode::Selected {
            selector.ipaddresses.clone()
        } else {
            candidates.clone()
        };
        for id in ids {
            self.append_address(selector, id, &candidates, false, &mut result);
        }
        if result.addresses.len() > MAX_ENDPOINT_ADDRESSES {
            result
                .errors
                .push("Endpoint address budget exceeded".into());
        }
        result
    }
    fn resolve_range(&self, selector: &DataflowEndpointInput) -> ResolvedEndpoint {
        let mut result = ResolvedEndpoint::default();
        let Some(row) = selector.object_id.and_then(|id| self.ranges.get(&id)) else {
            result.errors.push("IP range is missing".into());
            return result;
        };
        if row.status != "active" {
            result.errors.push("IP range is not active".into());
        }
        let mut networks = match self.range_cidrs(row) {
            Ok(networks) => networks,
            Err(error) => {
                result.errors.push(error);
                return result;
            }
        };
        if networks.is_empty() {
            result.errors.push("IP range is empty".into());
            return result;
        }
        let mut address_errors = BTreeSet::new();
        for network in &mut networks {
            for ip in self.ips.values() {
                let Some(address) = &ip.address else {
                    continue;
                };
                if address.vrf_id == row.vrf_id && network.network.contains(&address.network) {
                    network.owners.extend(address.owners.iter().cloned());
                    address_errors.extend(
                        address
                            .errors
                            .iter()
                            .filter(|error| error.as_str() == AMBIGUOUS_ADDRESS_OWNERSHIP)
                            .cloned(),
                    );
                }
            }
            result.owners.extend(network.owners.iter().cloned());
        }
        if !address_errors.is_empty() {
            result.errors.extend(address_errors);
            return result;
        }
        result.addresses = networks;
        if result.addresses.len() > MAX_ALL_ADDRESSES {
            result
                .errors
                .push("IP range address budget exceeded".into());
        }
        result
    }

    fn resolve_tag(&self, selector: &DataflowEndpointInput) -> ResolvedEndpoint {
        let mut result = ResolvedEndpoint::default();
        let (Some(tag_id), Some(scope)) = (selector.object_id, selector.tag_object_type) else {
            result
                .errors
                .push("Tag selector requires a tag and tag_object_type".into());
            return result;
        };
        if !scope.is_tag_scope() {
            result
                .errors
                .push("Tag selector scope must be a device or virtual machine".into());
            return result;
        }
        if !self.tags.contains_key(&tag_id) {
            result.errors.push("Tag is missing".into());
            return result;
        }
        let members: BTreeSet<DataflowEndpoint> = self
            .hosts
            .iter()
            .filter(|(host, row)| {
                host.object_type == scope
                    && row.tags.iter().any(|tag| tag_uuid(tag) == Some(tag_id))
            })
            .map(|(host, _)| host.clone())
            .collect();
        if members.is_empty() {
            result.errors.push("Tag selector has no members".into());
            return result;
        }
        for host in &members {
            let host_selector = DataflowEndpointInput {
                object_type: host.object_type,
                object_id: Some(host.object_id),
                address_mode: selector.address_mode,
                ipaddresses: Vec::new(),
                tag_object_type: None,
            };
            let candidates = self.host_candidates(&host_selector);
            if candidates.is_empty() {
                if selector.address_mode == DataflowAddressMode::Oob {
                    result
                        .errors
                        .push("Tag member has no out-of-band address".into());
                } else {
                    result.exclusions += 1;
                }
                continue;
            }
            for id in candidates.clone() {
                self.append_address(&host_selector, id, &candidates, false, &mut result);
            }
        }
        if result.addresses.len() > MAX_ENDPOINT_ADDRESSES {
            result
                .errors
                .push("Endpoint address budget exceeded".into());
        }
        result
    }

    fn resolve_address_group(&self, selector: &DataflowEndpointInput) -> ResolvedEndpoint {
        let mut result = ResolvedEndpoint::default();
        let Some((_, members)) = selector
            .object_id
            .and_then(|id| self.address_groups.get(&id))
        else {
            result.errors.push("Address group is missing".into());
            return result;
        };
        if members.is_empty() {
            result.errors.push("Address group has no members".into());
            return result;
        }
        if selector.address_mode != DataflowAddressMode::Default {
            result
                .errors
                .push("Address group selectors accept the default address mode only".into());
            return result;
        }
        for member in members {
            let resolved = self.resolve(member);
            result.owners.extend(resolved.owners);
            result.addresses.extend(resolved.addresses);
            result
                .ownership_addresses
                .extend(resolved.ownership_addresses);
            result.errors.extend(resolved.errors);
            result.exclusions += resolved.exclusions;
        }
        if result.addresses.len() > MAX_ALL_ADDRESSES {
            result
                .errors
                .push("Address group address budget exceeded".into());
        }
        result
    }

    fn resolve_all(&self) -> ResolvedEndpoint {
        let addresses: Vec<_> = self
            .ips
            .values()
            .filter(|ip| {
                ip.row.status == "active"
                    && ip
                        .errors
                        .iter()
                        .all(|error| error == AMBIGUOUS_ADDRESS_OWNERSHIP)
            })
            .filter_map(|ip| ip.address.as_ref())
            .filter(|ip| !ip.owners.is_empty())
            .cloned()
            .collect();
        let owners = addresses
            .iter()
            .flat_map(|ip| ip.owners.iter().cloned())
            .collect::<BTreeSet<_>>();
        let mut result = ResolvedEndpoint {
            exclusions: self.hosts.len().saturating_sub(owners.len()),
            addresses,
            owners,
            ..Default::default()
        };
        if result.addresses.len() > MAX_ALL_ADDRESSES {
            result
                .errors
                .push("All inventory address budget exceeded".into());
        }
        result
    }
    fn resolve_prefix(&self, selector: &DataflowEndpointInput) -> ResolvedEndpoint {
        let mut result = ResolvedEndpoint::default();
        let Some(row) = selector.object_id.and_then(|id| self.prefixes.get(&id)) else {
            result.errors.push("Prefix is missing".into());
            return result;
        };
        if row.status != "active" {
            result.errors.push("Prefix is not active".into());
        }
        let mut address_errors = BTreeSet::new();
        for id in self.prefix_candidates(selector) {
            if let Some(address) = self.ips.get(&id).and_then(|ip| ip.address.as_ref()) {
                result.owners.extend(address.owners.iter().cloned());
                address_errors.extend(
                    address
                        .errors
                        .iter()
                        .filter(|error| error.as_str() == AMBIGUOUS_ADDRESS_OWNERSHIP)
                        .cloned(),
                );
            }
        }
        match PolicyNetwork::parse(&row.prefix, false) {
            Ok(network) => result.addresses.push(PolicyAddress {
                network,
                vrf_id: row.vrf_id,
                owners: result.owners.clone(),
                errors: address_errors.into_iter().collect(),
            }),
            Err(error) => result.errors.push(error),
        }
        result
    }
    fn candidates(
        &self,
        selector: &DataflowEndpointInput,
        result: &mut ResolvedEndpoint,
    ) -> Vec<Uuid> {
        match selector.object_type {
            EndpointType::Device | EndpointType::VirtualMachine => self.host_candidates(selector),
            EndpointType::Service => self.service_candidates(selector, result),
            EndpointType::IpAddress => selector.object_id.into_iter().collect(),
            EndpointType::Prefix => self.prefix_candidates(selector),
            EndpointType::IpRange => self.range_candidates(selector),
            EndpointType::Tag => self.tag_candidates(selector),
            EndpointType::AddressGroup => self
                .address_groups
                .get(&selector.object_id.unwrap_or_default())
                .map(|(_, members)| {
                    members
                        .iter()
                        .flat_map(|member| {
                            self.candidates(member, &mut ResolvedEndpoint::default())
                        })
                        .collect()
                })
                .unwrap_or_default(),
            EndpointType::All | EndpointType::Internet => Vec::new(),
            kind if kind.is_scope() => self.scope_candidates(selector, result),
            _ => Vec::new(),
        }
    }
    fn range_candidates(&self, selector: &DataflowEndpointInput) -> Vec<Uuid> {
        let Some(row) = selector.object_id.and_then(|id| self.ranges.get(&id)) else {
            return Vec::new();
        };
        let Ok(networks) = self.range_cidrs(row) else {
            return Vec::new();
        };
        self.ips
            .values()
            .filter(|ip| {
                ip.address.as_ref().is_some_and(|address| {
                    address.vrf_id == row.vrf_id
                        && networks
                            .iter()
                            .any(|network| network.network.contains(&address.network))
                })
            })
            .map(|ip| ip.row.id)
            .collect()
    }
    fn tag_candidates(&self, selector: &DataflowEndpointInput) -> Vec<Uuid> {
        let (Some(tag_id), Some(scope)) = (selector.object_id, selector.tag_object_type) else {
            return Vec::new();
        };
        let mut ids = BTreeSet::new();
        for (host, row) in &self.hosts {
            if host.object_type != scope
                || !row.tags.iter().any(|tag| tag_uuid(tag) == Some(tag_id))
            {
                continue;
            }
            let host_selector = DataflowEndpointInput {
                object_type: host.object_type,
                object_id: Some(host.object_id),
                address_mode: selector.address_mode,
                ipaddresses: Vec::new(),
                tag_object_type: None,
            };
            ids.extend(self.host_candidates(&host_selector));
        }
        ids.into_iter().collect()
    }
    fn host_candidates(&self, selector: &DataflowEndpointInput) -> Vec<Uuid> {
        let Some(host) = selector.identity() else {
            return Vec::new();
        };
        if selector.address_mode == DataflowAddressMode::Default {
            return self
                .hosts
                .get(&host)
                .into_iter()
                .flat_map(|row| [row.primary_ip4_id, row.primary_ip6_id])
                .flatten()
                .collect();
        }
        if selector.address_mode == DataflowAddressMode::Oob {
            return self
                .hosts
                .get(&host)
                .and_then(|row| row.oob_ip_id)
                .into_iter()
                .collect();
        }
        self.ips
            .values()
            .filter(|ip| {
                ip.address
                    .as_ref()
                    .is_some_and(|address| address.owners.contains(&host))
            })
            .map(|ip| ip.row.id)
            .collect()
    }
    fn service_candidates(
        &self,
        selector: &DataflowEndpointInput,
        result: &mut ResolvedEndpoint,
    ) -> Vec<Uuid> {
        let Some(service) = selector.object_id.and_then(|id| self.services.get(&id)) else {
            return Vec::new();
        };
        let Ok(kind) = EndpointType::parse(&service.parent_kind) else {
            result
                .errors
                .push("Service parent type is unsupported".into());
            return Vec::new();
        };
        let host = DataflowEndpoint {
            object_type: kind,
            object_id: service.parent_id,
        };
        result.owners.insert(host.clone());
        if !kind.is_host() || !self.hosts.contains_key(&host) {
            result.errors.push("Service parent host is missing".into());
            return Vec::new();
        }
        if !service.bindings.is_empty() {
            return service.bindings.clone();
        }
        self.ips
            .values()
            .filter(|ip| {
                ip.assigned_owner.as_ref() == Some(&host)
                    || (selector.address_mode == DataflowAddressMode::Selected
                        && ip
                            .address
                            .as_ref()
                            .is_some_and(|address| address.owners.contains(&host)))
            })
            .map(|ip| ip.row.id)
            .collect()
    }
    fn prefix_candidates(&self, selector: &DataflowEndpointInput) -> Vec<Uuid> {
        let Some(row) = selector.object_id.and_then(|id| self.prefixes.get(&id)) else {
            return Vec::new();
        };
        let Ok(network) = PolicyNetwork::parse(&row.prefix, false) else {
            return Vec::new();
        };
        self.ips
            .values()
            .filter(|ip| {
                ip.address.as_ref().is_some_and(|address| {
                    address.vrf_id == row.vrf_id && network.contains(&address.network)
                })
            })
            .map(|ip| ip.row.id)
            .collect()
    }
    fn append_address(
        &self,
        selector: &DataflowEndpointInput,
        id: Uuid,
        candidates: &[Uuid],
        discovered: bool,
        result: &mut ResolvedEndpoint,
    ) {
        let strict = self.strict_address_selection(selector, discovered);
        let Some(ip) = self.ips.get(&id) else {
            reject_address(discovered, "Selected or primary IP is missing", result);
            return;
        };
        if !candidates.contains(&id) {
            result
                .errors
                .push("Selected IP does not belong to the endpoint".into());
            return;
        }
        // Typed identity survives network/status validation for explicit endpoints.
        if strict {
            result.owners.extend(ip.owners.iter().cloned());
            if let Some(address) = &ip.address {
                result.ownership_addresses.push(address.clone());
            }
        }
        if !retain_address_for_projection(ip, strict, discovered, result) {
            return;
        }
        let Some(mut address) = ip.address.clone() else {
            return;
        };
        if let Some(error) = self.host_address_error(selector, id, &address) {
            reject_address(discovered, error, result);
            return;
        }
        if selector.object_type == EndpointType::Service {
            address.owners.extend(result.owners.iter().cloned());
        }
        result.owners.extend(address.owners.iter().cloned());
        result.addresses.push(address);
    }
    fn strict_address_selection(&self, selector: &DataflowEndpointInput, discovered: bool) -> bool {
        let strict = selector.address_mode == DataflowAddressMode::Selected
            || selector.address_mode == DataflowAddressMode::Oob
            || (selector.object_type.is_host()
                && selector.address_mode == DataflowAddressMode::Default)
            || selector.object_type == EndpointType::IpAddress;
        !discovered
            && (strict
                || (selector.object_type == EndpointType::Service
                    && selector
                        .object_id
                        .and_then(|id| self.services.get(&id))
                        .is_some_and(|service| !service.bindings.is_empty())))
    }
    fn host_address_error(
        &self,
        selector: &DataflowEndpointInput,
        id: Uuid,
        address: &PolicyAddress,
    ) -> Option<&'static str> {
        let host = selector
            .identity()
            .filter(|identity| identity.object_type.is_host())?;
        if !address.owners.contains(&host) {
            return Some("IP binding has the wrong host ownership");
        }
        if selector.address_mode == DataflowAddressMode::Oob
            && self
                .hosts
                .get(&host)
                .is_some_and(|row| row.oob_ip_id != Some(id))
        {
            return Some("Out-of-band address does not belong to the host");
        }
        if selector.address_mode == DataflowAddressMode::Default
            && self.hosts.get(&host).is_some_and(|row| {
                (row.primary_ip4_id == Some(id) && !address.network.address.is_ipv4())
                    || (row.primary_ip6_id == Some(id) && address.network.address.is_ipv4())
            })
        {
            return Some("Primary IP slot has the wrong address family");
        }
        None
    }
}
fn reject_address(discovered: bool, message: &str, result: &mut ResolvedEndpoint) {
    if discovered {
        result.exclusions += 1;
    } else {
        result.errors.push(message.into());
    }
}
fn retain_address_for_projection(
    ip: &CatalogIp,
    strict: bool,
    discovered: bool,
    result: &mut ResolvedEndpoint,
) -> bool {
    if ip.errors.is_empty() {
        return true;
    }
    if discovered
        && ip
            .errors
            .iter()
            .all(|error| error == AMBIGUOUS_ADDRESS_OWNERSHIP)
    {
        // Keep ambiguous discovered addresses until realm-compatible projection.
        return true;
    }
    if strict
        || ip
            .errors
            .iter()
            .any(|error| error == AMBIGUOUS_ADDRESS_OWNERSHIP)
    {
        result.errors.extend(ip.errors.clone());
    } else {
        result.exclusions += 1;
    }
    false
}
fn catalog_ip(row: IpRow, mut owners: BTreeSet<DataflowEndpoint>) -> CatalogIp {
    let mut errors = Vec::new();
    let assigned_owner = match (row.assigned_kind.as_deref(), row.assigned_host_id) {
        (Some("dcim.interface"), Some(id)) => Some(DataflowEndpoint {
            object_type: EndpointType::Device,
            object_id: id,
        }),
        (Some("virtualization.vminterface"), Some(id)) => Some(DataflowEndpoint {
            object_type: EndpointType::VirtualMachine,
            object_id: id,
        }),
        _ => None,
    };
    if let Some(owner) = &assigned_owner {
        owners.insert(owner.clone());
    }
    if (row.assigned_object_id.is_some() || row.assigned_kind.is_some()) && assigned_owner.is_none()
    {
        errors.push("IP assignment is missing or unsupported".into());
    }
    if row.status != "active" {
        errors.push("IP is not active".into());
    }
    let address = match PolicyNetwork::parse(&row.address, true) {
        Ok(network) => Some(PolicyAddress {
            network,
            vrf_id: row.vrf_id,
            owners: owners.clone(),
            errors: Vec::new(),
        }),
        Err(error) => {
            errors.push(error);
            None
        }
    };
    CatalogIp {
        row,
        owners,
        address,
        errors,
        assigned_owner,
    }
}
fn bind_service_ownership(
    service: &ServiceRow,
    hosts: &BTreeMap<DataflowEndpoint, HostRow>,
    ips: &mut BTreeMap<Uuid, CatalogIp>,
) {
    let Ok(kind) = EndpointType::parse(&service.parent_kind) else {
        return;
    };
    let parent = DataflowEndpoint {
        object_type: kind,
        object_id: service.parent_id,
    };
    if !hosts.contains_key(&parent) {
        return;
    }
    for id in &service.bindings {
        if let Some(ip) = ips.get_mut(id) {
            ip.owners.insert(parent.clone());
            if ip
                .assigned_owner
                .as_ref()
                .is_some_and(|owner| owner != &parent)
            {
                ip.errors
                    .push("Service binding does not belong to its parent".into());
            }
            if let Some(address) = &mut ip.address {
                address.owners.insert(parent.clone());
            }
        }
    }
}
fn mark_ambiguous_contexts(ips: &mut BTreeMap<Uuid, CatalogIp>) {
    let mut contexts: BTreeMap<(String, Option<Uuid>), BTreeSet<DataflowEndpoint>> =
        BTreeMap::new();
    for ip in ips.values().filter(|ip| ip.row.status == "active") {
        if let Some(address) = &ip.address {
            contexts
                .entry((address.network.cidr(), address.vrf_id))
                .or_default()
                .extend(address.owners.iter().cloned());
        }
    }
    for ip in ips.values_mut() {
        if let Some(address) = &mut ip.address {
            if contexts
                .get(&(address.network.cidr(), address.vrf_id))
                .is_some_and(|owners| owners.len() > 1)
            {
                ip.errors.push(AMBIGUOUS_ADDRESS_OWNERSHIP.into());
            }
            address.errors = ip.errors.clone();
        }
    }
}
