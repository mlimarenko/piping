use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::contexts::connection_config::ConnConfScope;
use crate::contexts::connections::domain::{
    ConnectionProtocol, ConnectionSession, ConnectionStatus, ConnectionTemplate,
    NewProxyNodeAssignment, ProxyNode, ProxyNodeAssignment, TargetProxyAssignment,
    UpdateProxyNodeAssignment, UserCredentialSet,
};
use crate::contexts::shared::list_query::{ListColumn, SortSpec};
use crate::errors::AppError;

pub const CONNECTION_SESSION_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable(
        "started_at",
        "started_at",
        "connections.sessions.columns.started_at",
    ),
    ListColumn::fk_filter(
        "host",
        "target_id",
        "target_id",
        "target_id",
        "connection_targets",
        "connections.sessions.columns.host",
    ),
    ListColumn::fk_filter(
        "user",
        "user_id",
        "user_id",
        "user_id",
        "users",
        "connections.sessions.columns.user",
    ),
    ListColumn::enum_filter(
        "protocol",
        "protocol",
        "protocol",
        "connections.sessions.columns.protocol",
    ),
    ListColumn::enum_filter(
        "status",
        "status",
        "status",
        "connections.sessions.columns.status",
    ),
    ListColumn::sortable(
        "last_activity",
        "last_activity",
        "connections.sessions.columns.last_activity",
    ),
];

pub const CONNECTION_TEMPLATE_COLUMNS: &[ListColumn] = &[ListColumn::sortable(
    "name",
    "name",
    "connections.templates.columns.name",
)];

/// Proxy-node list columns. Sort SQL values are compile-time literals and the
/// ONLY thing interpolated into `ORDER BY`; user input never reaches SQL.
pub const PROXY_NODE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "connections.proxyNodes.columns.name"),
    ListColumn::sortable("slug", "slug", "connections.proxyNodes.columns.slug"),
    ListColumn::sortable(
        "base_url",
        "base_url",
        "connections.proxyNodes.columns.baseUrl",
    ),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "connections.proxyNodes.columns.enabled",
    ),
    ListColumn::sortable(
        "secret_version",
        "secret_version",
        "connections.proxyNodes.columns.secretVersion",
    ),
];

pub const PROXY_NODE_ASSIGNMENT_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "proxy_node_id",
        "proxy_node_id",
        "proxy_node_id",
        "proxy_node_id",
        "proxy_nodes",
        "connections.proxyAssignments.columns.proxy",
    ),
    ListColumn::enum_filter(
        "scope_type",
        "scope_type",
        "scope_type",
        "connections.proxyAssignments.columns.scope_type",
    ),
    ListColumn::sortable(
        "weight",
        "weight",
        "connections.proxyAssignments.columns.weight",
    ),
    ListColumn::enum_filter(
        "enabled",
        "enabled",
        "enabled::text",
        "connections.proxyAssignments.columns.enabled",
    ),
];

pub const USER_CREDENTIAL_SET_COLUMNS: &[ListColumn] = &[
    ListColumn::fk_filter(
        "user_id",
        "user_id",
        "user_id",
        "user_id",
        "access.users",
        "userSettings.credentials.columns.user",
    ),
    ListColumn::enum_filter(
        "protocol",
        "protocol",
        "protocol",
        "userSettings.credentials.columns.protocol",
    ),
    ListColumn::sortable("name", "name", "userSettings.credentials.columns.name"),
    ListColumn::enum_filter(
        "is_default",
        "is_default",
        "is_default::text",
        "userSettings.credentials.columns.isDefault",
    ),
    ListColumn::sortable(
        "created_at",
        "created_at",
        "userSettings.credentials.columns.created_at",
    ),
    ListColumn::sortable(
        "updated_at",
        "updated_at",
        "userSettings.credentials.columns.updated_at",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct ConnectionTemplateFilter {
    pub ids: Option<Vec<Uuid>>,
    pub protocol: Option<ConnectionProtocol>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct ConnectionSessionFilter {
    /// Multi-value `target_id` IN filter (matches any).
    pub target_id: Vec<Uuid>,
    /// Multi-value `user_id` IN filter (matches any).
    pub user_id: Vec<Uuid>,
    /// Multi-value `protocol` IN filter (matches any).
    pub protocol: Vec<ConnectionProtocol>,
    /// Multi-value `status` IN filter (matches any).
    pub status: Vec<ConnectionStatus>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct ProxyNodeFilter {
    /// Multi-value boolean-as-enum filter (`true` / `false`, matches any).
    pub enabled: Vec<String>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct ProxyNodeAssignmentFilter {
    /// Multi-value `proxy_node_id` IN filter (matches any).
    pub proxy_node_id: Vec<Uuid>,
    /// Multi-value `scope_type` IN filter (matches any).
    pub scope_type: Vec<ConnConfScope>,
    pub scope_id: Option<Uuid>,
    /// Multi-value boolean-as-enum filter (`true` / `false`, matches any).
    pub enabled: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct UserCredentialSetFilter {
    /// Multi-value owner FK filter (matches any).
    pub user_id: Vec<Uuid>,
    /// Multi-value credential protocol filter (matches any).
    pub protocol: Vec<ConnectionProtocol>,
    /// Multi-value boolean-as-enum filter (`true` / `false`, matches any).
    pub is_default: Vec<String>,
    pub sort: Option<SortSpec>,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait ConnectionRepository: Send + Sync {
    async fn create_template(&self, template: &ConnectionTemplate) -> Result<(), AppError>;
    async fn get_template(&self, template_id: Uuid)
    -> Result<Option<ConnectionTemplate>, AppError>;
    async fn list_templates(
        &self,
        filter: ConnectionTemplateFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ConnectionTemplate>, AppError>;
    async fn count_templates(&self, filter: ConnectionTemplateFilter) -> Result<u64, AppError>;
    async fn create_session(&self, session: &ConnectionSession) -> Result<(), AppError>;
    async fn get_session(&self, session_id: Uuid) -> Result<Option<ConnectionSession>, AppError>;
    async fn list_sessions(
        &self,
        filter: ConnectionSessionFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ConnectionSession>, AppError>;
    async fn count_sessions(&self, filter: ConnectionSessionFilter) -> Result<u64, AppError>;
    async fn update_session(&self, session: &ConnectionSession) -> Result<(), AppError>;
    async fn update_status(
        &self,
        session_id: Uuid,
        status: ConnectionStatus,
        ended_at: Option<DateTime<Utc>>,
    ) -> Result<(), AppError>;
    async fn update_last_activity(
        &self,
        session_id: Uuid,
        last_activity: DateTime<Utc>,
    ) -> Result<(), AppError>;

    async fn create_proxy_node(&self, proxy_node: &ProxyNode) -> Result<(), AppError>;
    async fn get_proxy_node(&self, proxy_node_id: Uuid) -> Result<Option<ProxyNode>, AppError>;
    async fn get_proxy_node_by_slug(&self, slug: &str) -> Result<Option<ProxyNode>, AppError>;
    async fn list_proxy_nodes(
        &self,
        filter: ProxyNodeFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ProxyNode>, AppError>;
    async fn count_proxy_nodes(&self, filter: ProxyNodeFilter) -> Result<u64, AppError>;
    async fn update_proxy_node(&self, proxy_node: &ProxyNode) -> Result<(), AppError>;
    async fn mark_proxy_node_secret_rotated(
        &self,
        proxy_node_id: Uuid,
        rotated_at: DateTime<Utc>,
    ) -> Result<(), AppError>;
    /// Returns `true` when a row was deleted, `false` when no such node exists.
    async fn delete_proxy_node(&self, proxy_node_id: Uuid) -> Result<bool, AppError>;
    async fn create_proxy_node_assignment(
        &self,
        input: &NewProxyNodeAssignment,
    ) -> Result<ProxyNodeAssignment, AppError>;
    async fn get_proxy_node_assignment(
        &self,
        assignment_id: Uuid,
    ) -> Result<Option<ProxyNodeAssignment>, AppError>;
    async fn list_proxy_node_assignments(
        &self,
        filter: ProxyNodeAssignmentFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ProxyNodeAssignment>, AppError>;
    async fn count_proxy_node_assignments(
        &self,
        filter: ProxyNodeAssignmentFilter,
    ) -> Result<u64, AppError>;
    async fn update_proxy_node_assignment(
        &self,
        assignment_id: Uuid,
        input: &UpdateProxyNodeAssignment,
    ) -> Result<Option<ProxyNodeAssignment>, AppError>;
    async fn delete_proxy_node_assignment(&self, assignment_id: Uuid) -> Result<bool, AppError>;
    async fn list_proxy_node_assignments_for_scopes(
        &self,
        scopes: &[(ConnConfScope, Option<Uuid>)],
    ) -> Result<Vec<ProxyNodeAssignment>, AppError>;
    async fn get_target_proxy_assignment(
        &self,
        target_id: Uuid,
    ) -> Result<Option<TargetProxyAssignment>, AppError>;
    async fn upsert_target_proxy_assignment(
        &self,
        assignment: &TargetProxyAssignment,
    ) -> Result<(), AppError>;
    async fn clear_target_proxy_assignment(&self, target_id: Uuid) -> Result<(), AppError>;
    async fn is_endpoint_bound_to_target(
        &self,
        target_id: Uuid,
        endpoint_host: &str,
    ) -> Result<bool, AppError>;
    async fn create_user_credential_set(&self, set: &UserCredentialSet) -> Result<(), AppError>;
    async fn get_user_credential_set(
        &self,
        set_id: Uuid,
    ) -> Result<Option<UserCredentialSet>, AppError>;
    /// One page of credential sets plus the TOTAL number of matches, so
    /// callers can report a page count. Returning only the page made `total`
    /// the page length, which capped every UI at one page. `limit`/`offset`
    /// are optional because internal callers (default resolution, launch
    /// options, proxy payload assembly) need the complete per-user set.
    async fn list_user_credential_sets(
        &self,
        filter: UserCredentialSetFilter,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<(Vec<UserCredentialSet>, i64), AppError>;
    async fn update_user_credential_set(&self, set: &UserCredentialSet) -> Result<(), AppError>;
    async fn delete_user_credential_set(&self, set_id: Uuid) -> Result<(), AppError>;
    async fn set_default_user_credential_set(
        &self,
        user_id: Uuid,
        protocol: ConnectionProtocol,
        set_id: Uuid,
        updated_at: DateTime<Utc>,
    ) -> Result<(), AppError>;

    /// The pinned `known_hosts` lines for one SSH endpoint, as the connect path
    /// must seed them into the proxy payload.
    ///
    /// Default: this repository has no pin store, so the endpoint is unpinned.
    /// The SQLx repository overrides this against `ssh_host_key_pin`;
    /// in-memory test doubles inherit the default and need no database.
    async fn resolve_host_key_lines(&self, host: &str, port: u16) -> Result<Vec<String>, AppError> {
        let _ = (host, port);
        Ok(Vec::new())
    }

    /// Persist the `known_hosts` lines the proxy observed for a connection.
    ///
    /// Trust-on-first-use lives in the store, so a repeat observation is
    /// idempotent. Default: no pin store, so nothing is recorded.
    /// The SQLx repository overrides this against `ssh_host_key_pin`.
    async fn persist_observed_host_key_lines(&self, lines: &[String]) -> Result<(), AppError> {
        let _ = lines;
        Ok(())
    }
}
