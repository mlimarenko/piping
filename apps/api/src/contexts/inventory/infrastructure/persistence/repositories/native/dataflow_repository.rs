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
        address_mode: selector.address_mode,
        ipaddresses: selector.ipaddresses.clone(),
        label,
    }
}

fn select_flows() -> String {
    let columns = |side: &str| {
        EndpointType::REFERENCES
            .iter()
            .map(|entry| format!("{side}_{}", entry.2))
            .collect::<Vec<_>>()
            .join(",")
    };
    format!("SELECT f.id, f.name, f.description, f.status, f.transport, f.destination_ports, f.allow_cross_vrf,
 source_kind, destination_kind, source_mode, destination_mode,
 COALESCE({}) AS source_id,
 COALESCE({}) AS destination_id,
 ARRAY(SELECT ipaddress_id FROM inventory_dataflow_selected_ip WHERE dataflow_id = f.id AND side = 'source' ORDER BY ipaddress_id) AS source_ips,
  ARRAY(SELECT ipaddress_id FROM inventory_dataflow_selected_ip WHERE dataflow_id = f.id AND side = 'destination' ORDER BY ipaddress_id) AS destination_ips FROM inventory_dataflow f",columns("source"),columns("destination"))
}
#[derive(Debug, FromRow)]
struct DataflowRow {
    id: Uuid,
    name: String,
    description: String,
    status: String,
    transport: String,
    destination_ports: Option<Vec<i32>>,
    allow_cross_vrf: bool,
    source_kind: String,
    destination_kind: String,
    source_mode: String,
    destination_mode: String,
    source_id: Option<Uuid>,
    destination_id: Option<Uuid>,
    source_ips: Vec<Uuid>,
    destination_ips: Vec<Uuid>,
}
impl TryFrom<DataflowRow> for Dataflow {
    type Error = AppError;
    fn try_from(row: DataflowRow) -> Result<Self, AppError> {
        let parse_mode = |mode: &str| match mode {
            "default" => Ok(DataflowAddressMode::Default),
            "selected" => Ok(DataflowAddressMode::Selected),
            "all" => Ok(DataflowAddressMode::All),
            _ => Err(AppError::Database("Invalid endpoint address mode".into())),
        };
        Ok(Self {
            id: row.id,
            name: row.name,
            description: row.description,
            status: match row.status.as_str() {
                "draft" => DataflowStatus::Draft,
                "active" => DataflowStatus::Active,
                "disabled" => DataflowStatus::Disabled,
                _ => return Err(AppError::Database("Invalid dataflow status".into())),
            },
            transport: match row.transport.as_str() {
                "any" => DataflowTransport::Any,
                "tcp" => DataflowTransport::Tcp,
                "udp" => DataflowTransport::Udp,
                "sctp" => DataflowTransport::Sctp,
                _ => return Err(AppError::Database("Invalid dataflow transport".into())),
            },
            source: DataflowEndpointInput {
                object_type: EndpointType::parse(&row.source_kind)?,
                object_id: row.source_id,
                address_mode: parse_mode(&row.source_mode)?,
                ipaddresses: row.source_ips,
            },
            destination: DataflowEndpointInput {
                object_type: EndpointType::parse(&row.destination_kind)?,
                object_id: row.destination_id,
                address_mode: parse_mode(&row.destination_mode)?,
                ipaddresses: row.destination_ips,
            },
            destination_ports: row.destination_ports,
            allow_cross_vrf: row.allow_cross_vrf,
        })
    }
}
#[derive(Debug, Default)]
pub struct DataflowFilter {
    pub status: Option<DataflowStatus>,
    pub search: Option<String>,
    pub source: Option<DataflowEndpointInput>,
    pub destination: Option<DataflowEndpointInput>,
}
#[derive(Debug, Default)]
pub struct DataflowPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<DataflowStatus>,
    pub source: Option<DataflowEndpointInput>,
    pub destination: Option<DataflowEndpointInput>,
    pub transport: Option<DataflowTransport>,
    pub destination_ports: Option<Option<Vec<i32>>>,
    pub allow_cross_vrf: Option<bool>,
}
impl DataflowPatch {
    fn apply(self, flow: &mut Dataflow) {
        if let Some(value) = self.name {
            flow.name = value;
        }
        if let Some(value) = self.description {
            flow.description = value;
        }
        if let Some(value) = self.status {
            flow.status = value;
        }
        if let Some(value) = self.source {
            flow.source = value;
        }
        if let Some(value) = self.destination {
            flow.destination = value;
        }
        if let Some(value) = self.transport {
            flow.transport = value;
        }
        if let Some(value) = self.destination_ports {
            flow.destination_ports = value;
        }
        if let Some(value) = self.allow_cross_vrf {
            flow.allow_cross_vrf = value;
        }
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
        sqlx::query_as::<_, DataflowRow>(&format!("{} WHERE f.id = $1", select_flows()))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .map(Dataflow::try_from)
            .transpose()
    }
    pub async fn list(
        &self,
        filter: &DataflowFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<Dataflow>, i64), AppError> {
        let mut query = QueryBuilder::<Postgres>::new(select_flows());
        push_filters(&mut query, filter);
        query
            .push(" ORDER BY f.name, f.id LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = query
            .build_query_as::<DataflowRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut count = QueryBuilder::<Postgres>::new("SELECT count(*) FROM inventory_dataflow f");
        push_filters(&mut count, filter);
        let total = count
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok((
            rows.into_iter()
                .map(Dataflow::try_from)
                .collect::<Result<_, _>>()?,
            total,
        ))
    }
    pub async fn create(&self, flow: &Dataflow) -> Result<Dataflow, AppError> {
        let mut flow = flow.clone();
        flow.source.normalize()?;
        flow.destination.normalize()?;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let catalog = Catalog::load(
            &mut tx,
            &[&flow.source, &flow.destination],
            CatalogPurpose::Mutation,
        )
        .await?;
        if flow.name.is_empty() {
            flow.name = format!(
                "{} → {}",
                catalog.label(&flow.source),
                catalog.label(&flow.destination)
            )
            .chars()
            .take(60)
            .collect();
        }
        flow.normalize()?;
        validate_selected(&mut tx, &catalog, &flow).await?;
        insert_flow(&mut tx, &flow).await?;
        replace_selections(&mut tx, &flow).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(flow)
    }
    pub async fn update(&self, id: Uuid, patch: DataflowPatch) -> Result<Dataflow, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let row = sqlx::query_as::<_, DataflowRow>(&format!(
            "{} WHERE f.id = $1 FOR UPDATE OF f",
            select_flows()
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?
        .ok_or_else(|| AppError::NotFound("Dataflow not found".into()))?;
        let mut flow = Dataflow::try_from(row)?;
        patch.apply(&mut flow);
        flow.normalize()?;
        let catalog = Catalog::load(
            &mut tx,
            &[&flow.source, &flow.destination],
            CatalogPurpose::Mutation,
        )
        .await?;
        validate_selected(&mut tx, &catalog, &flow).await?;
        sqlx::query("UPDATE inventory_dataflow SET name = $2, description = $3, status = $4, transport = $5, destination_ports = $6, allow_cross_vrf = $7 WHERE id = $1")
            .bind(id).bind(&flow.name).bind(&flow.description).bind(flow.status.as_str()).bind(flow.transport.as_str()).bind(&flow.destination_ports).bind(flow.allow_cross_vrf).execute(&mut *tx).await.map_err(classify_sqlx_error_owned)?;
        persist_endpoints(&mut tx, &flow).await?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(flow)
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
    pub async fn projections(
        &self,
        flows: &[Dataflow],
    ) -> Result<Vec<(DataflowEndpointResponse, DataflowEndpointResponse)>, AppError> {
        if flows.is_empty() {
            return Ok(Vec::new());
        }
        let mut demanded: BTreeMap<EndpointType, BTreeSet<Uuid>> = BTreeMap::new();
        for selector in flows
            .iter()
            .flat_map(|flow| [&flow.source, &flow.destination])
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
                _ => "name",
            };
            // Identifiers come only from the typed registry; endpoint IDs are bound.
            let rows = sqlx::query_as::<_, (Uuid, Option<String>)>(&format!(
                "SELECT id, {column} FROM {table} WHERE id = ANY($1::uuid[])"
            ))
            .bind(ids.iter().copied().collect::<Vec<_>>())
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
            .map(|flow| (projection(&flow.source), projection(&flow.destination)))
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
        let rows = sqlx::query_as::<_, DataflowRow>(&format!(
            "{} WHERE status = 'active' ORDER BY f.id LIMIT $1",
            select_flows()
        ))
        .bind((MAX_ACTIVE_FLOWS + 1) as i64)
        .fetch_all(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        let flows: Vec<Dataflow> = rows
            .into_iter()
            .map(Dataflow::try_from)
            .collect::<Result<_, _>>()?;
        let selectors: BTreeSet<_> = flows
            .iter()
            .flat_map(|flow| [&flow.source, &flow.destination])
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
            .flat_map(|flow| [&flow.source, &flow.destination])
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
fn push_filters<'a>(query: &mut QueryBuilder<'a, Postgres>, filter: &'a DataflowFilter) {
    query.push(" WHERE true");
    if let Some(status) = filter.status {
        query.push(" AND f.status = ").push_bind(status.as_str());
    }
    if let Some(search) = &filter.search {
        query
            .push(" AND (f.name ILIKE ")
            .push_bind(search)
            .push(" OR f.description ILIKE ")
            .push_bind(search)
            .push(")");
    }
    for (side, selector) in [
        ("source", &filter.source),
        ("destination", &filter.destination),
    ] {
        if let Some(selector) = selector {
            query
                .push(format!(" AND {side}_kind = "))
                .push_bind(selector.object_type.as_str());
            if let Some(id) = selector.object_id {
                query
                    .push(format!(" AND {side}_{} = ", selector.object_type.column()))
                    .push_bind(id);
            }
        }
    }
}
async fn persist_endpoints(connection: &mut PgConnection, flow: &Dataflow) -> Result<(), AppError> {
    let mut query = QueryBuilder::<Postgres>::new("UPDATE inventory_dataflow SET ");
    let mut fields = query.separated(", ");
    for (side, endpoint) in [("source", &flow.source), ("destination", &flow.destination)] {
        fields
            .push(format!("{side}_kind = "))
            .push_bind_unseparated(endpoint.object_type.as_str());
        fields
            .push(format!("{side}_mode = "))
            .push_bind_unseparated(endpoint.address_mode.as_str());
        for &(kind, _, _, _) in EndpointType::REFERENCES {
            fields
                .push(format!("{side}_{} = ", kind.column()))
                .push_bind_unseparated(if endpoint.object_type == kind {
                    endpoint.object_id
                } else {
                    None
                });
        }
    }
    query.push(" WHERE id = ").push_bind(flow.id);
    query
        .build()
        .execute(&mut *connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
    replace_selections(connection, flow).await
}
async fn insert_flow(connection: &mut PgConnection, flow: &Dataflow) -> Result<(), AppError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "INSERT INTO inventory_dataflow (id, name, description, status, transport, destination_ports, allow_cross_vrf",
    );
    for side in ["source", "destination"] {
        query.push(format!(", {side}_kind, {side}_mode"));
        for &(kind, _, _, _) in EndpointType::REFERENCES {
            query.push(format!(", {side}_{}", kind.column()));
        }
    }
    query.push(") VALUES (");
    let mut values = query.separated(", ");
    values
        .push_bind(flow.id)
        .push_bind(&flow.name)
        .push_bind(&flow.description)
        .push_bind(flow.status.as_str())
        .push_bind(flow.transport.as_str())
        .push_bind(&flow.destination_ports)
        .push_bind(flow.allow_cross_vrf);
    for endpoint in [&flow.source, &flow.destination] {
        values
            .push_bind(endpoint.object_type.as_str())
            .push_bind(endpoint.address_mode.as_str());
        for &(kind, _, _, _) in EndpointType::REFERENCES {
            values.push_bind(if endpoint.object_type == kind {
                endpoint.object_id
            } else {
                None
            });
        }
    }
    query.push(")");
    query
        .build()
        .execute(connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(())
}
async fn replace_selections(
    connection: &mut PgConnection,
    flow: &Dataflow,
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM inventory_dataflow_selected_ip WHERE dataflow_id = $1")
        .bind(flow.id)
        .execute(&mut *connection)
        .await
        .map_err(classify_sqlx_error_owned)?;
    for (side, endpoint) in [("source", &flow.source), ("destination", &flow.destination)] {
        sqlx::query("INSERT INTO inventory_dataflow_selected_ip (dataflow_id, side, ipaddress_id) SELECT $1, $2, unnest($3::uuid[])").bind(flow.id).bind(side).bind(&endpoint.ipaddresses).execute(&mut *connection).await.map_err(classify_sqlx_error_owned)?;
    }
    Ok(())
}
async fn validate_selected(
    connection: &mut PgConnection,
    catalog: &Catalog,
    flow: &Dataflow,
) -> Result<(), AppError> {
    for selector in [&flow.source, &flow.destination] {
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
#[derive(Debug, FromRow)]
struct HostRow {
    kind: String,
    id: Uuid,
    name: Option<String>,
    primary_ip4_id: Option<Uuid>,
    primary_ip6_id: Option<Uuid>,
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
        let (recursive, predicate) = if descendants && kind.is_hierarchical() {
            (
                format!(
                    "WITH RECURSIVE wanted AS (SELECT id FROM {table} WHERE id = ANY($1) UNION SELECT child.id FROM {table} child JOIN wanted parent ON child.parent_id = parent.id) "
                ),
                "id IN (SELECT id FROM wanted)",
            )
        } else {
            (String::new(), "id = ANY($1)")
        };
        let rows = sqlx::query_as::<_,Self>(&format!("{recursive}SELECT id,{label} AS label,{parent} AS parent_id,{region} AS region_id,{group} AS group_id,{cluster_type} AS type_id,{manufacturer} AS manufacturer_id FROM {table} WHERE {predicate} LIMIT 4097")).bind(ids).fetch_all(connection).await.map_err(classify_sqlx_error_owned)?;
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
            EndpointType::All | EndpointType::Internet => Vec::new(),
            kind if kind.is_scope() => self.scope_candidates(selector, result),
            _ => Vec::new(),
        }
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
