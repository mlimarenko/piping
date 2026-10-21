//! Piping-native cabling workstation read model.
//!
//! This module intentionally exposes endpoint/trace language instead of raw
//! NetBox cable internals. SQL is built only from the endpoint definitions below;
//! user input is passed through binds.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::pagination::clamp_limit;
use crate::errors::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CableEndpointKind {
    Interface,
    FrontPort,
    RearPort,
    ConsolePort,
    ConsoleServerPort,
    PowerPort,
    PowerOutlet,
    PowerFeed,
    CircuitTermination,
}

impl CableEndpointKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interface => "interface",
            Self::FrontPort => "front-port",
            Self::RearPort => "rear-port",
            Self::ConsolePort => "console-port",
            Self::ConsoleServerPort => "console-server-port",
            Self::PowerPort => "power-port",
            Self::PowerOutlet => "power-outlet",
            Self::PowerFeed => "power-feed",
            Self::CircuitTermination => "circuit-termination",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "interface" => Some(Self::Interface),
            "front-port" => Some(Self::FrontPort),
            "rear-port" => Some(Self::RearPort),
            "console-port" => Some(Self::ConsolePort),
            "console-server-port" => Some(Self::ConsoleServerPort),
            "power-port" => Some(Self::PowerPort),
            "power-outlet" => Some(Self::PowerOutlet),
            "power-feed" => Some(Self::PowerFeed),
            "circuit-termination" => Some(Self::CircuitTermination),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CablingLinkState {
    Available,
    Cabled,
    MarkedConnected,
    Unknown,
}

impl CablingLinkState {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "available" => Some(Self::Available),
            "cabled" => Some(Self::Cabled),
            "marked-connected" => Some(Self::MarkedConnected),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CablingTraceState {
    Complete,
    Incomplete,
    Split,
    Invalid,
    PositionConflict,
    Unsupported,
}

impl CablingTraceState {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "complete" => Some(Self::Complete),
            "incomplete" => Some(Self::Incomplete),
            "split" => Some(Self::Split),
            "invalid" => Some(Self::Invalid),
            "position-conflict" => Some(Self::PositionConflict),
            "unsupported" => Some(Self::Unsupported),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CablingTraversal {
    Cable,
    FrontRearMap,
    PowerChain,
    CircuitHandOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CablingIssueCode {
    OpenEnd,
    MultipleNextHops,
    DanglingReference,
    PositionMismatch,
    UnsupportedEndpoint,
    MarkedConnected,
    TraceTruncated,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CablingEndpointRef {
    pub kind: CableEndpointKind,
    pub id: Uuid,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CablingResourceLink {
    pub entity: String,
    pub id: Uuid,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CablingLocation {
    pub site: Option<CablingResourceLink>,
    pub rack: Option<CablingResourceLink>,
    pub location: Option<CablingResourceLink>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CablingCableSummary {
    pub id: Uuid,
    pub label: String,
    pub status: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CablingEndpointItem {
    pub endpoint: CablingEndpointRef,
    pub parent: Option<CablingResourceLink>,
    pub location: CablingLocation,
    pub link_state: CablingLinkState,
    pub trace_state: Option<CablingTraceState>,
    pub cable: Option<CablingCableSummary>,
    pub related_links: Vec<CablingResourceLink>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CablingTraceSegment {
    pub index: u16,
    pub traversal: CablingTraversal,
    pub from: CablingEndpointRef,
    pub to: Option<CablingEndpointRef>,
    pub cable: Option<CablingCableSummary>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CablingTraceIssue {
    pub code: CablingIssueCode,
    pub endpoint: Option<CablingEndpointRef>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CablingTraceSummary {
    pub hop_count: u16,
    pub cable_count: u16,
    pub issue_count: u16,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CablingTrace {
    pub start: CablingEndpointRef,
    pub state: CablingTraceState,
    pub summary: CablingTraceSummary,
    pub segments: Vec<CablingTraceSegment>,
    pub issues: Vec<CablingTraceIssue>,
    pub related_links: Vec<CablingResourceLink>,
    pub generated_at: DateTime<Utc>,
}

type TraceEndpointKey = (CableEndpointKind, Uuid);
type TraceCandidate = (
    CablingTraversal,
    Option<CablingCableSummary>,
    CablingEndpointItem,
);

#[derive(Debug, Clone, Default)]
pub struct CablingEndpointFilter {
    pub query: Option<String>,
    pub endpoint_kind: Vec<CableEndpointKind>,
    pub site_id: Vec<Uuid>,
    pub rack_id: Vec<Uuid>,
    pub device_id: Vec<Uuid>,
    pub link_state: Vec<CablingLinkState>,
    pub trace_state: Vec<CablingTraceState>,
    pub issues_only: bool,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait CablingRepository: Send + Sync {
    async fn search_endpoints(
        &self,
        filter: CablingEndpointFilter,
        limit: Option<u32>,
    ) -> Result<Vec<CablingEndpointItem>, AppError>;

    async fn trace_endpoint(
        &self,
        kind: CableEndpointKind,
        id: Uuid,
        max_hops: u16,
    ) -> Result<Option<CablingTrace>, AppError>;
}

#[derive(Clone)]
pub struct SqlxCablingRepository {
    pool: PgPool,
}

impl SqlxCablingRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn load_endpoint(
        &self,
        kind: CableEndpointKind,
        id: Uuid,
    ) -> Result<Option<CablingEndpointItem>, AppError> {
        let mut filter = CablingEndpointFilter {
            endpoint_kind: vec![kind],
            ..Default::default()
        };
        filter.query = None;
        let mut builder = QueryBuilder::<Postgres>::new(endpoint_union_sql(&filter.endpoint_kind));
        builder.push(" WHERE endpoint_id = ");
        builder.push_bind(id);
        builder.push(" LIMIT 1");

        let row = builder
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load cabling endpoint: {e}")))?;
        row.as_ref().map(row_to_endpoint).transpose()
    }

    async fn cable_peers(
        &self,
        cable_id: Uuid,
        exclude_kind: CableEndpointKind,
        exclude_id: Uuid,
    ) -> Result<Vec<CablingEndpointItem>, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(endpoint_union_sql(&[]));
        builder.push(" WHERE cable_id = ");
        builder.push_bind(cable_id);
        builder.push(" AND NOT (endpoint_kind = ");
        builder.push_bind(exclude_kind.as_str());
        builder.push(" AND endpoint_id = ");
        builder.push_bind(exclude_id);
        builder.push(") ORDER BY endpoint_label LIMIT 3");

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load cable peers: {e}")))?;
        rows.iter().map(row_to_endpoint).collect()
    }

    async fn mapped_peers(
        &self,
        endpoint: &CablingEndpointItem,
    ) -> Result<Vec<(CablingTraversal, CablingEndpointItem)>, AppError> {
        match endpoint.endpoint.kind {
            CableEndpointKind::FrontPort => {
                let rows = sqlx::query(
                    "SELECT rear_port_id AS id FROM dcim_portmapping WHERE front_port_id = $1 ORDER BY rear_port_position LIMIT 3",
                )
                .bind(endpoint.endpoint.id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| AppError::Database(format!("Failed to load front/rear mapping: {e}")))?;
                let mut peers = Vec::new();
                for row in rows {
                    let id: Uuid = row.try_get("id").map_err(|e| {
                        AppError::Database(format!("Failed to read rear port mapping id: {e}"))
                    })?;
                    if let Some(peer) = self.load_endpoint(CableEndpointKind::RearPort, id).await? {
                        peers.push((CablingTraversal::FrontRearMap, peer));
                    }
                }
                Ok(peers)
            }
            CableEndpointKind::RearPort => {
                let rows = sqlx::query(
                    "SELECT front_port_id AS id FROM dcim_portmapping WHERE rear_port_id = $1 ORDER BY front_port_position LIMIT 3",
                )
                .bind(endpoint.endpoint.id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| AppError::Database(format!("Failed to load rear/front mapping: {e}")))?;
                let mut peers = Vec::new();
                for row in rows {
                    let id: Uuid = row.try_get("id").map_err(|e| {
                        AppError::Database(format!("Failed to read front port mapping id: {e}"))
                    })?;
                    if let Some(peer) = self.load_endpoint(CableEndpointKind::FrontPort, id).await?
                    {
                        peers.push((CablingTraversal::FrontRearMap, peer));
                    }
                }
                Ok(peers)
            }
            CableEndpointKind::PowerPort => {
                let rows = sqlx::query(
                    "SELECT id FROM dcim_poweroutlet WHERE power_port_id = $1 ORDER BY name LIMIT 3",
                )
                .bind(endpoint.endpoint.id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| AppError::Database(format!("Failed to load power outlets: {e}")))?;
                let mut peers = Vec::new();
                for row in rows {
                    let id: Uuid = row.try_get("id").map_err(|e| {
                        AppError::Database(format!("Failed to read power outlet id: {e}"))
                    })?;
                    if let Some(peer) = self
                        .load_endpoint(CableEndpointKind::PowerOutlet, id)
                        .await?
                    {
                        peers.push((CablingTraversal::PowerChain, peer));
                    }
                }
                Ok(peers)
            }
            CableEndpointKind::PowerOutlet => {
                let row = sqlx::query("SELECT power_port_id FROM dcim_poweroutlet WHERE id = $1")
                    .bind(endpoint.endpoint.id)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| {
                        AppError::Database(format!("Failed to load power outlet upstream: {e}"))
                    })?;
                let Some(row) = row else {
                    return Ok(Vec::new());
                };
                let id: Option<Uuid> = row.try_get("power_port_id").ok().flatten();
                if let Some(id) = id
                    && let Some(peer) = self.load_endpoint(CableEndpointKind::PowerPort, id).await?
                {
                    return Ok(vec![(CablingTraversal::PowerChain, peer)]);
                }
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }
}

impl CablingRepository for SqlxCablingRepository {
    async fn search_endpoints(
        &self,
        filter: CablingEndpointFilter,
        limit: Option<u32>,
    ) -> Result<Vec<CablingEndpointItem>, AppError> {
        let limit = clamp_limit(limit).min(100) as i64;
        let mut builder = QueryBuilder::<Postgres>::new(endpoint_union_sql(&filter.endpoint_kind));
        push_endpoint_filters(&mut builder, &filter);
        builder.push(" ORDER BY parent_label NULLS LAST, endpoint_label LIMIT ");
        builder.push_bind(limit);

        let rows =
            builder.build().fetch_all(&self.pool).await.map_err(|e| {
                AppError::Database(format!("Failed to search cabling endpoints: {e}"))
            })?;
        rows.iter().map(row_to_endpoint).collect()
    }

    async fn trace_endpoint(
        &self,
        kind: CableEndpointKind,
        id: Uuid,
        max_hops: u16,
    ) -> Result<Option<CablingTrace>, AppError> {
        let Some(start) = self.load_endpoint(kind, id).await? else {
            return Ok(None);
        };
        if kind == CableEndpointKind::CircuitTermination || kind == CableEndpointKind::PowerFeed {
            let issue = CablingTraceIssue {
                code: CablingIssueCode::UnsupportedEndpoint,
                endpoint: Some(start.endpoint.clone()),
                message: "Endpoint kind is indexed but not traversable in this workstation version"
                    .to_string(),
            };
            return Ok(Some(trace_from_parts(
                start,
                CablingTraceState::Unsupported,
                Vec::new(),
                vec![issue],
            )));
        }

        let mut current = start.clone();
        let mut segments = Vec::new();
        let mut issues = Vec::new();
        let mut seen = vec![(current.endpoint.kind, current.endpoint.id)];
        let mut cable_count = 0u16;
        let mut terminal_reached = false;

        for index in 0..max_hops {
            let mut next: Vec<TraceCandidate> = Vec::new();
            if let Some(cable) = current.cable.clone() {
                for peer in self
                    .cable_peers(cable.id, current.endpoint.kind, current.endpoint.id)
                    .await?
                {
                    next.push((CablingTraversal::Cable, Some(cable.clone()), peer));
                }
            } else if current.link_state == CablingLinkState::MarkedConnected {
                issues.push(CablingTraceIssue {
                    code: CablingIssueCode::MarkedConnected,
                    endpoint: Some(current.endpoint.clone()),
                    message: "Endpoint is marked connected without a cable reference".to_string(),
                });
            }
            for (traversal, peer) in self.mapped_peers(&current).await? {
                next.push((traversal, None, peer));
            }

            let had_candidates = filter_unseen_trace_candidates(&mut next, &seen);
            if next.is_empty() {
                if had_candidates && !segments.is_empty() && issues.is_empty() {
                    terminal_reached = true;
                    break;
                }
                issues.push(CablingTraceIssue {
                    code: CablingIssueCode::OpenEnd,
                    endpoint: Some(current.endpoint.clone()),
                    message: "No further cable, port map, or power-chain hop found".to_string(),
                });
                terminal_reached = true;
                break;
            }
            if next.len() > 1 {
                issues.push(CablingTraceIssue {
                    code: CablingIssueCode::MultipleNextHops,
                    endpoint: Some(current.endpoint.clone()),
                    message: "More than one next hop exists for this endpoint".to_string(),
                });
                let (traversal, cable, peer) = next.remove(0);
                if traversal == CablingTraversal::Cable {
                    cable_count = cable_count.saturating_add(1);
                }
                segments.push(CablingTraceSegment {
                    index,
                    traversal,
                    from: current.endpoint.clone(),
                    to: Some(peer.endpoint.clone()),
                    cable,
                    note: Some(
                        "First hop shown; additional branches are reported as an issue".to_string(),
                    ),
                });
                terminal_reached = true;
                break;
            }

            let (traversal, cable, peer) = next.remove(0);
            if traversal == CablingTraversal::Cable {
                cable_count = cable_count.saturating_add(1);
            }
            segments.push(CablingTraceSegment {
                index,
                traversal,
                from: current.endpoint.clone(),
                to: Some(peer.endpoint.clone()),
                cable,
                note: None,
            });
            seen.push((peer.endpoint.kind, peer.endpoint.id));
            current = peer;
        }
        push_trace_truncated_issue(&mut issues, terminal_reached, &current, max_hops);

        let state = trace_state_from_issues(&issues);
        let mut trace = trace_from_parts(start, state, segments, issues);
        trace.summary.cable_count = cable_count;
        Ok(Some(trace))
    }
}

fn filter_unseen_trace_candidates(
    candidates: &mut Vec<TraceCandidate>,
    seen: &[TraceEndpointKey],
) -> bool {
    let had_candidates = !candidates.is_empty();
    candidates.retain(|(_, _, peer)| !seen.contains(&(peer.endpoint.kind, peer.endpoint.id)));
    had_candidates
}

fn push_trace_truncated_issue(
    issues: &mut Vec<CablingTraceIssue>,
    terminal_reached: bool,
    current: &CablingEndpointItem,
    max_hops: u16,
) {
    if terminal_reached {
        return;
    }
    issues.push(CablingTraceIssue {
        code: CablingIssueCode::TraceTruncated,
        endpoint: Some(current.endpoint.clone()),
        message: format!(
            "Trace stopped after {max_hops} hops before a terminal endpoint was proven"
        ),
    });
}

fn trace_state_from_issues(issues: &[CablingTraceIssue]) -> CablingTraceState {
    if issues
        .iter()
        .any(|issue| issue.code == CablingIssueCode::MultipleNextHops)
    {
        CablingTraceState::Split
    } else if issues.iter().any(|issue| {
        matches!(
            issue.code,
            CablingIssueCode::MarkedConnected
                | CablingIssueCode::OpenEnd
                | CablingIssueCode::TraceTruncated
        )
    }) {
        CablingTraceState::Incomplete
    } else {
        CablingTraceState::Complete
    }
}

fn trace_from_parts(
    start: CablingEndpointItem,
    state: CablingTraceState,
    segments: Vec<CablingTraceSegment>,
    issues: Vec<CablingTraceIssue>,
) -> CablingTrace {
    let mut related_links = start.related_links.clone();
    if let Some(parent) = start.parent.clone() {
        related_links.push(parent);
    }
    let hop_count = u16::try_from(segments.len()).unwrap_or(u16::MAX);
    let issue_count = u16::try_from(issues.len()).unwrap_or(u16::MAX);
    CablingTrace {
        start: start.endpoint,
        state,
        summary: CablingTraceSummary {
            hop_count,
            cable_count: 0,
            issue_count,
        },
        segments,
        issues,
        related_links,
        generated_at: Utc::now(),
    }
}

fn endpoint_union_sql(kinds: &[CableEndpointKind]) -> String {
    let mut parts = Vec::new();
    let include = |kind: CableEndpointKind| kinds.is_empty() || kinds.contains(&kind);

    if include(CableEndpointKind::Interface) {
        parts.push(endpoint_select(EndpointSelectSpec {
            kind: "interface",
            table: "dcim_interface",
            endpoint_label_sql: "COALESCE(NULLIF(e.label, ''), e.name)",
            parent_entity: "Devices",
            parent_join: "dcim_device d ON d.id = e.device_id",
            parent_id_sql: Some("d.id"),
            parent_label_sql: Some("d.name"),
            device_id_sql: "e.device_id",
            site_id_sql: "e._site_id",
            rack_id_sql: "e._rack_id",
            location_id_sql: "e._location_id",
        }));
    }
    for (kind, table) in [
        (CableEndpointKind::FrontPort, "dcim_frontport"),
        (CableEndpointKind::RearPort, "dcim_rearport"),
        (CableEndpointKind::ConsolePort, "dcim_consoleport"),
        (
            CableEndpointKind::ConsoleServerPort,
            "dcim_consoleserverport",
        ),
        (CableEndpointKind::PowerPort, "dcim_powerport"),
        (CableEndpointKind::PowerOutlet, "dcim_poweroutlet"),
    ] {
        if include(kind) {
            parts.push(endpoint_select(EndpointSelectSpec {
                kind: kind.as_str(),
                table,
                endpoint_label_sql: "COALESCE(NULLIF(e.label, ''), e.name)",
                parent_entity: "Devices",
                parent_join: "dcim_device d ON d.id = e.device_id",
                parent_id_sql: Some("d.id"),
                parent_label_sql: Some("d.name"),
                device_id_sql: "e.device_id",
                site_id_sql: "e._site_id",
                rack_id_sql: "e._rack_id",
                location_id_sql: "e._location_id",
            }));
        }
    }
    if include(CableEndpointKind::PowerFeed) {
        parts.push(endpoint_select(EndpointSelectSpec {
            kind: "power-feed",
            table: "dcim_powerfeed",
            endpoint_label_sql: "e.name",
            parent_entity: "Power Panels",
            parent_join: "dcim_powerpanel d ON d.id = e.power_panel_id",
            parent_id_sql: Some("d.id"),
            parent_label_sql: Some("d.name"),
            device_id_sql: "NULL::uuid",
            site_id_sql: "NULL::uuid",
            rack_id_sql: "e.rack_id",
            location_id_sql: "NULL::uuid",
        }));
    }
    if include(CableEndpointKind::CircuitTermination) {
        parts.push(endpoint_select(EndpointSelectSpec {
            kind: "circuit-termination",
            table: "circuits_circuittermination",
            endpoint_label_sql: "('Termination ' || e.term_side)",
            parent_entity: "Circuits",
            parent_join: "circuits_circuit d ON d.id = e.circuit_id",
            parent_id_sql: Some("d.id"),
            parent_label_sql: Some("d.cid"),
            device_id_sql: "NULL::uuid",
            site_id_sql: "e._site_id",
            rack_id_sql: "NULL::uuid",
            location_id_sql: "e._location_id",
        }));
    }

    format!(
        "SELECT * FROM ({}) cabling_endpoint",
        parts.join(" UNION ALL ")
    )
}

struct EndpointSelectSpec<'a> {
    kind: &'a str,
    table: &'a str,
    endpoint_label_sql: &'a str,
    parent_entity: &'a str,
    parent_join: &'a str,
    parent_id_sql: Option<&'a str>,
    parent_label_sql: Option<&'a str>,
    device_id_sql: &'a str,
    site_id_sql: &'a str,
    rack_id_sql: &'a str,
    location_id_sql: &'a str,
}

fn endpoint_select(spec: EndpointSelectSpec<'_>) -> String {
    let EndpointSelectSpec {
        kind,
        table,
        endpoint_label_sql,
        parent_entity,
        parent_join,
        parent_id_sql,
        parent_label_sql,
        device_id_sql,
        site_id_sql,
        rack_id_sql,
        location_id_sql,
    } = spec;
    let parent_id = parent_id_sql.unwrap_or("NULL::uuid");
    let parent_label = parent_label_sql.unwrap_or("NULL::text");
    format!(
        r#"
        SELECT
            '{kind}'::text AS endpoint_kind,
            e.id AS endpoint_id,
            {endpoint_label_sql}::text AS endpoint_label,
            '{parent_entity}'::text AS parent_entity,
            {parent_id} AS parent_id,
            {parent_label}::text AS parent_label,
            {device_id_sql} AS device_id,
            COALESCE({site_id_sql}, rack.site_id, loc.site_id) AS site_id,
            site.name::text AS site_name,
            {rack_id_sql} AS rack_id,
            rack.name::text AS rack_name,
            {location_id_sql} AS location_id,
            loc.name::text AS location_name,
            CASE
              WHEN e.cable_id IS NOT NULL THEN 'cabled'
              WHEN e.mark_connected THEN 'marked-connected'
              ELSE 'available'
            END AS link_state,
            CASE
              WHEN e.cable_id IS NOT NULL THEN 'complete'
              WHEN e.mark_connected THEN 'incomplete'
              ELSE 'incomplete'
            END AS trace_state,
            e.cable_id,
            cable.label::text AS cable_label,
            cable.status::text AS cable_status,
            NULLIF(cable.color, '')::text AS cable_color
        FROM {table} e
        LEFT JOIN {parent_join}
        LEFT JOIN dcim_cable cable ON cable.id = e.cable_id
        LEFT JOIN dcim_rack rack ON rack.id = {rack_id_sql}
        LEFT JOIN dcim_location loc ON loc.id = {location_id_sql}
        LEFT JOIN dcim_site site ON site.id = COALESCE({site_id_sql}, rack.site_id, loc.site_id)
        "#
    )
}

fn push_endpoint_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &CablingEndpointFilter) {
    let mut where_state = WhereState::new();

    if let Some(query) = filter.query.as_deref() {
        where_state.and(builder);
        let pattern = format!("%{query}%");
        builder.push("(endpoint_label ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR parent_label ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR cable_label ILIKE ");
        builder.push_bind(pattern);
        builder.push(")");
    }

    push_uuid_filter(builder, &mut where_state, "site_id", &filter.site_id);
    push_uuid_filter(builder, &mut where_state, "rack_id", &filter.rack_id);
    push_uuid_filter(builder, &mut where_state, "device_id", &filter.device_id);

    if !filter.link_state.is_empty() {
        where_state.and(builder);
        builder.push("link_state = ANY(");
        let states: Vec<&str> = filter
            .link_state
            .iter()
            .map(|state| match state {
                CablingLinkState::Available => "available",
                CablingLinkState::Cabled => "cabled",
                CablingLinkState::MarkedConnected => "marked-connected",
                CablingLinkState::Unknown => "unknown",
            })
            .collect();
        builder.push_bind(states);
        builder.push(")");
    }
    if !filter.trace_state.is_empty() {
        where_state.and(builder);
        builder.push("trace_state = ANY(");
        let states: Vec<&str> = filter
            .trace_state
            .iter()
            .map(|state| match state {
                CablingTraceState::Complete => "complete",
                CablingTraceState::Incomplete => "incomplete",
                CablingTraceState::Split => "split",
                CablingTraceState::Invalid => "invalid",
                CablingTraceState::PositionConflict => "position-conflict",
                CablingTraceState::Unsupported => "unsupported",
            })
            .collect();
        builder.push_bind(states);
        builder.push(")");
    }
    if filter.issues_only {
        where_state.and(builder);
        builder.push("trace_state <> 'complete'");
    }
}

fn push_uuid_filter(
    builder: &mut QueryBuilder<'_, Postgres>,
    state: &mut WhereState,
    column: &'static str,
    values: &[Uuid],
) {
    if values.is_empty() {
        return;
    }
    state.and(builder);
    builder.push(column);
    builder.push(" = ANY(");
    builder.push_bind(values.to_vec());
    builder.push(")");
}

fn row_to_endpoint(row: &PgRow) -> Result<CablingEndpointItem, AppError> {
    let kind_raw: String = row
        .try_get("endpoint_kind")
        .map_err(|e| AppError::Database(format!("Failed to read cabling endpoint kind: {e}")))?;
    let kind = CableEndpointKind::parse(&kind_raw)
        .ok_or_else(|| AppError::Database(format!("Unknown cabling endpoint kind {kind_raw}")))?;
    let link_raw: String = row
        .try_get("link_state")
        .map_err(|e| AppError::Database(format!("Failed to read cabling link state: {e}")))?;
    let link_state = CablingLinkState::parse(&link_raw)
        .ok_or_else(|| AppError::Database(format!("Unknown cabling link state {link_raw}")))?;
    let trace_raw: Option<String> = row.try_get("trace_state").ok();
    let trace_state = trace_raw.as_deref().and_then(CablingTraceState::parse);
    let endpoint = CablingEndpointRef {
        kind,
        id: row
            .try_get("endpoint_id")
            .map_err(|e| AppError::Database(format!("Failed to read endpoint id: {e}")))?,
        label: row
            .try_get("endpoint_label")
            .map_err(|e| AppError::Database(format!("Failed to read endpoint label: {e}")))?,
    };
    let parent = link_from_row(row, "parent_entity", "parent_id", "parent_label")?;
    let site = link_from_row_const_entity(row, "Sites", "site_id", "site_name")?;
    let rack = link_from_row_const_entity(row, "Racks", "rack_id", "rack_name")?;
    let location = link_from_row_const_entity(row, "Locations", "location_id", "location_name")?;
    let cable = row
        .try_get::<Option<Uuid>, _>("cable_id")
        .ok()
        .flatten()
        .map(|id| CablingCableSummary {
            id,
            label: row
                .try_get::<Option<String>, _>("cable_label")
                .ok()
                .flatten()
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| id.to_string()),
            status: row
                .try_get::<Option<String>, _>("cable_status")
                .ok()
                .flatten()
                .unwrap_or_else(|| "unknown".to_string()),
            color: row
                .try_get::<Option<String>, _>("cable_color")
                .ok()
                .flatten(),
        });
    let mut related_links = Vec::new();
    if let Some(parent) = parent.clone() {
        related_links.push(parent);
    }
    if let Some(cable) = &cable {
        related_links.push(CablingResourceLink {
            entity: "Cables".to_string(),
            id: cable.id,
            label: cable.label.clone(),
        });
    }
    Ok(CablingEndpointItem {
        endpoint,
        parent,
        location: CablingLocation {
            site,
            rack,
            location,
        },
        link_state,
        trace_state,
        cable,
        related_links,
    })
}

fn link_from_row(
    row: &PgRow,
    entity_col: &str,
    id_col: &str,
    label_col: &str,
) -> Result<Option<CablingResourceLink>, AppError> {
    let Some(id) = row.try_get::<Option<Uuid>, _>(id_col).ok().flatten() else {
        return Ok(None);
    };
    let entity: String = row
        .try_get(entity_col)
        .map_err(|e| AppError::Database(format!("Failed to read cabling link entity: {e}")))?;
    let label = row
        .try_get::<Option<String>, _>(label_col)
        .ok()
        .flatten()
        .unwrap_or_else(|| id.to_string());
    Ok(Some(CablingResourceLink { entity, id, label }))
}

fn link_from_row_const_entity(
    row: &PgRow,
    entity: &str,
    id_col: &str,
    label_col: &str,
) -> Result<Option<CablingResourceLink>, AppError> {
    let Some(id) = row.try_get::<Option<Uuid>, _>(id_col).ok().flatten() else {
        return Ok(None);
    };
    let label = row
        .try_get::<Option<String>, _>(label_col)
        .ok()
        .flatten()
        .unwrap_or_else(|| id.to_string());
    Ok(Some(CablingResourceLink {
        entity: entity.to_string(),
        id,
        label,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_endpoint(kind: CableEndpointKind, id: Uuid, label: &str) -> CablingEndpointItem {
        CablingEndpointItem {
            endpoint: CablingEndpointRef {
                kind,
                id,
                label: label.to_string(),
            },
            parent: None,
            location: CablingLocation {
                site: None,
                rack: None,
                location: None,
            },
            link_state: CablingLinkState::Available,
            trace_state: Some(CablingTraceState::Incomplete),
            cable: None,
            related_links: Vec::new(),
        }
    }

    #[test]
    fn parses_public_endpoint_kinds_only() {
        assert_eq!(
            CableEndpointKind::parse("front-port"),
            Some(CableEndpointKind::FrontPort)
        );
        assert_eq!(CableEndpointKind::parse("cable_end"), None);
        assert_eq!(CableEndpointKind::parse("termination_type_id"), None);
    }

    #[test]
    fn trace_candidate_filter_removes_seen_backedges_before_split_or_segment() {
        let seen_id = Uuid::from_u128(1);
        let current_id = Uuid::from_u128(2);
        let mut candidates = vec![(
            CablingTraversal::Cable,
            None,
            test_endpoint(CableEndpointKind::Interface, seen_id, "eth0"),
        )];
        let seen = vec![
            (CableEndpointKind::Interface, seen_id),
            (CableEndpointKind::Interface, current_id),
        ];

        let had_candidates = filter_unseen_trace_candidates(&mut candidates, &seen);

        assert!(had_candidates);
        assert!(
            candidates.is_empty(),
            "seen cable peer must be filtered before split/open-end handling"
        );
    }

    #[test]
    fn trace_candidate_filter_preserves_multiple_unseen_forward_candidates() {
        let seen_id = Uuid::from_u128(1);
        let rear_id = Uuid::from_u128(2);
        let other_id = Uuid::from_u128(3);
        let mut candidates = vec![
            (
                CablingTraversal::Cable,
                None,
                test_endpoint(CableEndpointKind::Interface, seen_id, "eth0"),
            ),
            (
                CablingTraversal::FrontRearMap,
                None,
                test_endpoint(CableEndpointKind::RearPort, rear_id, "rear-1"),
            ),
            (
                CablingTraversal::Cable,
                None,
                test_endpoint(CableEndpointKind::Interface, other_id, "eth1"),
            ),
        ];

        let had_candidates = filter_unseen_trace_candidates(
            &mut candidates,
            &[(CableEndpointKind::Interface, seen_id)],
        );

        assert!(had_candidates);
        assert_eq!(
            candidates.len(),
            2,
            "multiple unseen next hops must remain available for true split detection"
        );
        assert!(candidates.iter().all(|(_, _, peer)| {
            (peer.endpoint.kind, peer.endpoint.id) != (CableEndpointKind::Interface, seen_id)
        }));
    }

    #[test]
    fn hop_budget_exhaustion_after_first_hop_is_incomplete_not_complete() {
        let current = test_endpoint(CableEndpointKind::FrontPort, Uuid::from_u128(2), "panel-1");
        let mut issues = Vec::new();

        push_trace_truncated_issue(&mut issues, false, &current, 1);

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, CablingIssueCode::TraceTruncated);
        assert_eq!(
            trace_state_from_issues(&issues),
            CablingTraceState::Incomplete
        );
        assert!(
            issues[0].message.contains("1 hops"),
            "truncation issue should name the exhausted caller budget"
        );
    }

    #[test]
    fn proven_terminal_trace_does_not_add_truncation_issue() {
        let current = test_endpoint(CableEndpointKind::Interface, Uuid::from_u128(3), "eth1");
        let mut issues = Vec::new();

        push_trace_truncated_issue(&mut issues, true, &current, 1);

        assert!(issues.is_empty());
        assert_eq!(
            trace_state_from_issues(&issues),
            CablingTraceState::Complete
        );
    }

    #[test]
    fn public_endpoint_item_serialization_excludes_netbox_internals() {
        let item = CablingEndpointItem {
            endpoint: CablingEndpointRef {
                kind: CableEndpointKind::Interface,
                id: Uuid::nil(),
                label: "eth0".to_string(),
            },
            parent: None,
            location: CablingLocation {
                site: None,
                rack: None,
                location: None,
            },
            link_state: CablingLinkState::Available,
            trace_state: Some(CablingTraceState::Incomplete),
            cable: None,
            related_links: Vec::new(),
        };

        let json = serde_json::to_value(item).expect("serialize endpoint item");
        let text = json.to_string();
        for forbidden in [
            "cable_end",
            "_path_id",
            "path",
            "positions",
            "cable_positions",
            "termination_type_id",
            "termination_id",
            "connector",
        ] {
            assert!(
                !text.contains(forbidden),
                "public cabling DTO leaked {forbidden}: {text}"
            );
        }
    }

    #[test]
    fn endpoint_union_sql_uses_table_safe_label_expressions() {
        let interface_sql = endpoint_union_sql(&[CableEndpointKind::Interface]);
        assert!(
            interface_sql.contains("COALESCE(NULLIF(e.label, ''), e.name)::text AS endpoint_label"),
            "label-bearing endpoint SQL should coalesce label and name: {interface_sql}"
        );

        let power_feed_sql = endpoint_union_sql(&[CableEndpointKind::PowerFeed]);
        assert!(
            power_feed_sql.contains("FROM dcim_powerfeed e"),
            "power feed SQL should select from dcim_powerfeed: {power_feed_sql}"
        );
        assert!(
            power_feed_sql.contains("e.name::text AS endpoint_label"),
            "power feed SQL should use e.name directly: {power_feed_sql}"
        );
        assert!(
            !power_feed_sql.contains(" e.label") && !power_feed_sql.contains("(e.label"),
            "power feed SQL must not reference missing e.label column: {power_feed_sql}"
        );

        let circuit_sql = endpoint_union_sql(&[CableEndpointKind::CircuitTermination]);
        assert!(
            circuit_sql.contains("FROM circuits_circuittermination e"),
            "circuit termination SQL should select from circuits_circuittermination: {circuit_sql}"
        );
        assert!(
            circuit_sql.contains("('Termination ' || e.term_side)::text AS endpoint_label"),
            "circuit termination SQL should derive a safe label from term_side: {circuit_sql}"
        );
        assert!(
            !circuit_sql.contains(" e.label") && !circuit_sql.contains("(e.label"),
            "circuit termination SQL must not reference missing e.label column: {circuit_sql}"
        );
    }
}
