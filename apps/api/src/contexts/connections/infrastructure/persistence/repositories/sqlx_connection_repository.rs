use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::connection_repository::{
    CONNECTION_SESSION_COLUMNS, CONNECTION_TEMPLATE_COLUMNS, ConnectionRepository,
    ConnectionSessionFilter, ConnectionTemplateFilter, PROXY_NODE_ASSIGNMENT_COLUMNS,
    PROXY_NODE_COLUMNS, ProxyNodeAssignmentFilter, ProxyNodeFilter, USER_CREDENTIAL_SET_COLUMNS,
    UserCredentialSetFilter,
};
use crate::contexts::connection_config::ConnConfScope;
use crate::contexts::connections::domain::{
    ConnectionSession, ConnectionStatus, ConnectionTemplate, NewProxyNodeAssignment, ProxyNode,
    ProxyNodeAssignment, TargetProxyAssignment, UpdateProxyNodeAssignment, UserCredentialSet,
    parse_connection_protocol, parse_connection_status, parse_routing_source,
};
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, list_sort_column, push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

const PROXY_ASSIGNMENT_COLUMNS: &str =
    "id, proxy_node_id, scope_type, scope_id, weight, enabled, created_at, updated_at";

#[derive(Debug, Clone)]
pub struct SqlxConnectionRepository {
    pool: PgPool,
}

impl SqlxConnectionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_template_row(row: &PgRow) -> Result<ConnectionTemplate, AppError> {
        let protocol: String = row.try_get("protocol").map_err(|e| {
            AppError::Database(format!(
                "Failed to read connection template protocol: {}",
                e
            ))
        })?;
        Ok(ConnectionTemplate {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read template id: {}", e)))?,
            name: row
                .try_get("name")
                .map_err(|e| AppError::Database(format!("Failed to read template name: {}", e)))?,
            protocol: parse_connection_protocol(&protocol).map_err(|error| {
                AppError::Database(format!("Invalid template protocol: {}", error))
            })?,
            defaults: row.try_get("defaults").map_err(|e| {
                AppError::Database(format!("Failed to read template defaults: {}", e))
            })?,
        })
    }

    fn parse_proxy_node_row(row: &PgRow) -> Result<ProxyNode, AppError> {
        Ok(ProxyNode {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read proxy node id: {}", e)))?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read proxy node name: {}", e))
            })?,
            slug: row.try_get("slug").map_err(|e| {
                AppError::Database(format!("Failed to read proxy node slug: {}", e))
            })?,
            base_url: row.try_get("base_url").map_err(|e| {
                AppError::Database(format!("Failed to read proxy node base_url: {}", e))
            })?,
            enabled: row.try_get("enabled").map_err(|e| {
                AppError::Database(format!("Failed to read proxy node enabled flag: {}", e))
            })?,
            secret_version: row.try_get("secret_version").map_err(|e| {
                AppError::Database(format!("Failed to read proxy node secret_version: {}", e))
            })?,
            secret_rotated_at: row.try_get("secret_rotated_at").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read proxy node secret_rotated_at: {}",
                    e
                ))
            })?,
            created_at: row.try_get("created_at").map_err(|e| {
                AppError::Database(format!("Failed to read proxy node created_at: {}", e))
            })?,
            updated_at: row.try_get("updated_at").map_err(|e| {
                AppError::Database(format!("Failed to read proxy node updated_at: {}", e))
            })?,
        })
    }

    fn parse_proxy_node_assignment_row(row: &PgRow) -> Result<ProxyNodeAssignment, AppError> {
        let scope_raw: String = row.try_get("scope_type").map_err(|e| {
            AppError::Database(format!(
                "Failed to read proxy node assignment scope_type: {}",
                e
            ))
        })?;
        let scope_type = ConnConfScope::parse(&scope_raw).ok_or_else(|| {
            AppError::Database(format!(
                "Unknown proxy node assignment scope_type '{}'",
                scope_raw
            ))
        })?;
        Ok(ProxyNodeAssignment {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read proxy node assignment id: {}", e))
            })?,
            proxy_node_id: row.try_get("proxy_node_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read proxy node assignment proxy_node_id: {}",
                    e
                ))
            })?,
            scope_type,
            scope_id: row.try_get("scope_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read proxy node assignment scope_id: {}",
                    e
                ))
            })?,
            weight: row.try_get("weight").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read proxy node assignment weight: {}",
                    e
                ))
            })?,
            enabled: row.try_get("enabled").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read proxy node assignment enabled: {}",
                    e
                ))
            })?,
            created_at: row.try_get("created_at").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read proxy node assignment created_at: {}",
                    e
                ))
            })?,
            updated_at: row.try_get("updated_at").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read proxy node assignment updated_at: {}",
                    e
                ))
            })?,
        })
    }

    fn parse_target_proxy_assignment_row(row: &PgRow) -> Result<TargetProxyAssignment, AppError> {
        Ok(TargetProxyAssignment {
            target_id: row.try_get("target_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read target proxy assignment target_id: {}",
                    e
                ))
            })?,
            proxy_node_id: row.try_get("proxy_node_id").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read target proxy assignment proxy_node_id: {}",
                    e
                ))
            })?,
            updated_at: row.try_get("updated_at").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read target proxy assignment updated_at: {}",
                    e
                ))
            })?,
        })
    }

    fn parse_session_row(row: &PgRow) -> Result<ConnectionSession, AppError> {
        let protocol: String = row.try_get("protocol").map_err(|e| {
            AppError::Database(format!("Failed to read connection protocol: {}", e))
        })?;
        let status: String = row
            .try_get("status")
            .map_err(|e| AppError::Database(format!("Failed to read connection status: {}", e)))?;
        let routing_source: String = row
            .try_get("routing_source")
            .map_err(|e| AppError::Database(format!("Failed to read routing source: {}", e)))?;
        Ok(ConnectionSession {
            id: row
                .try_get("id")
                .map_err(|e| AppError::Database(format!("Failed to read session id: {}", e)))?,
            target_id: row.try_get("target_id").map_err(|e| {
                AppError::Database(format!("Failed to read session target_id: {}", e))
            })?,
            user_id: row.try_get("user_id").map_err(|e| {
                AppError::Database(format!("Failed to read session user_id: {}", e))
            })?,
            protocol: parse_connection_protocol(&protocol).map_err(|error| {
                AppError::Database(format!("Invalid session protocol: {}", error))
            })?,
            status: parse_connection_status(&status).map_err(|error| {
                AppError::Database(format!("Invalid session status: {}", error))
            })?,
            proxy_id: row.try_get("proxy_id").map_err(|e| {
                AppError::Database(format!("Failed to read session proxy_id: {}", e))
            })?,
            proxy_node_id: row.try_get("proxy_node_id").map_err(|e| {
                AppError::Database(format!("Failed to read session proxy_node_id: {}", e))
            })?,
            routing_source: parse_routing_source(&routing_source).map_err(|error| {
                AppError::Database(format!("Invalid routing source: {}", error))
            })?,
            client_ip: row.try_get("client_ip").map_err(|e| {
                AppError::Database(format!("Failed to read session client_ip: {}", e))
            })?,
            last_activity: row.try_get("last_activity").map_err(|e| {
                AppError::Database(format!("Failed to read session last_activity: {}", e))
            })?,
            session_metadata: row.try_get("session_metadata").map_err(|e| {
                AppError::Database(format!("Failed to read session session_metadata: {}", e))
            })?,
            started_at: row.try_get("started_at").map_err(|e| {
                AppError::Database(format!("Failed to read session started_at: {}", e))
            })?,
            ended_at: row.try_get("ended_at").map_err(|e| {
                AppError::Database(format!("Failed to read session ended_at: {}", e))
            })?,
        })
    }

    fn parse_user_credential_set_row(row: &PgRow) -> Result<UserCredentialSet, AppError> {
        let protocol: String = row.try_get("protocol").map_err(|e| {
            AppError::Database(format!(
                "Failed to read user credential set protocol: {}",
                e
            ))
        })?;
        Ok(UserCredentialSet {
            id: row.try_get("id").map_err(|e| {
                AppError::Database(format!("Failed to read user credential set id: {}", e))
            })?,
            user_id: row.try_get("user_id").map_err(|e| {
                AppError::Database(format!("Failed to read user credential set user_id: {}", e))
            })?,
            protocol: parse_connection_protocol(&protocol).map_err(|error| {
                AppError::Database(format!("Invalid user credential set protocol: {}", error))
            })?,
            name: row.try_get("name").map_err(|e| {
                AppError::Database(format!("Failed to read user credential set name: {}", e))
            })?,
            params: row.try_get("params").map_err(|e| {
                AppError::Database(format!("Failed to read user credential set params: {}", e))
            })?,
            is_default: row.try_get("is_default").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read user credential set is_default: {}",
                    e
                ))
            })?,
            created_at: row.try_get("created_at").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read user credential set created_at: {}",
                    e
                ))
            })?,
            updated_at: row.try_get("updated_at").map_err(|e| {
                AppError::Database(format!(
                    "Failed to read user credential set updated_at: {}",
                    e
                ))
            })?,
        })
    }

    fn push_template_filter(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &ConnectionTemplateFilter,
        state: &mut WhereState,
    ) {
        if let Some(ids) = &filter.ids {
            state.and(builder);
            builder.push("id = ANY(");
            builder.push_bind(ids.clone());
            builder.push(")");
        }

        if let Some(protocol) = &filter.protocol {
            state.and(builder);
            builder.push("protocol = ");
            builder.push_bind(protocol.as_str());
        }

        if let Some(search) = &filter.search {
            state.and(builder);
            builder.push("name ILIKE ");
            builder.push_bind(format!("%{}%", search));
        }
    }

    fn push_proxy_node_filter(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &ProxyNodeFilter,
        state: &mut WhereState,
    ) {
        if !filter.enabled.is_empty() {
            push_string_in(builder, state, "enabled::text", &filter.enabled);
        }

        if let Some(search) = &filter.search {
            state.and(builder);
            builder.push("(name ILIKE ");
            builder.push_bind(format!("%{}%", search));
            builder.push(" OR slug ILIKE ");
            builder.push_bind(format!("%{}%", search));
            builder.push(")");
        }
    }

    fn push_proxy_node_assignment_filter(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &ProxyNodeAssignmentFilter,
        state: &mut WhereState,
    ) {
        // Multi-value `proxy_node_id` IN filter (plain UUID column, matches any).
        if !filter.proxy_node_id.is_empty() {
            state.and(builder);
            builder.push("proxy_node_id IN (");
            let mut separated = builder.separated(", ");
            for proxy_node_id in &filter.proxy_node_id {
                separated.push_bind(*proxy_node_id);
            }
            builder.push(")");
        }

        // Multi-value `scope_type` IN filter (plain text column, matches any).
        if !filter.scope_type.is_empty() {
            state.and(builder);
            builder.push("scope_type IN (");
            let mut separated = builder.separated(", ");
            for scope_type in &filter.scope_type {
                separated.push_bind(scope_type.as_str());
            }
            builder.push(")");
        }

        if let Some(scope_id) = filter.scope_id {
            state.and(builder);
            builder.push("scope_id = ");
            builder.push_bind(scope_id);
        }

        if !filter.enabled.is_empty() {
            push_string_in(builder, state, "enabled::text", &filter.enabled);
        }
    }

    fn push_session_filter(
        builder: &mut QueryBuilder<'_, Postgres>,
        filter: &ConnectionSessionFilter,
        state: &mut WhereState,
    ) {
        push_uuid_in(builder, state, "target_id", &filter.target_id);
        push_uuid_in(builder, state, "user_id", &filter.user_id);

        // Multi-value `protocol` IN filter (plain text column, matches any).
        if !filter.protocol.is_empty() {
            state.and(builder);
            builder.push("protocol IN (");
            let mut separated = builder.separated(", ");
            for protocol in &filter.protocol {
                separated.push_bind(protocol.as_str());
            }
            builder.push(")");
        }

        // Multi-value `status` IN filter. `status` is the `connection_status`
        // enum column, so compare its text projection to plain text binds.
        if !filter.status.is_empty() {
            state.and(builder);
            builder.push("status::text IN (");
            let mut separated = builder.separated(", ");
            for status in &filter.status {
                separated.push_bind(status.as_str());
            }
            builder.push(")");
        }
    }

    /// Emit the allowlisted `ORDER BY` for a list query: a validated sort spec's
    /// column + direction with `id` as a stable tiebreaker, or the supplied
    /// `default` ordering when no sort is set. `allowlist` resolves the spec key
    /// to a safe literal column; user input never reaches the SQL string.
    fn push_order_by(
        builder: &mut QueryBuilder<'_, Postgres>,
        sort: Option<&SortSpec>,
        columns: &[ListColumn],
        default: &str,
    ) {
        builder.push(" ORDER BY ");
        match sort
            .and_then(|spec| list_sort_column(columns, &spec.key).map(|column| (column, spec)))
        {
            Some((column, spec)) => {
                builder.push(column);
                builder.push(spec.sql_direction());
                builder.push(", id");
            }
            None => {
                builder.push(default);
            }
        }
    }

    fn user_credential_set_filter_sql(param: &str) -> Option<&'static str> {
        USER_CREDENTIAL_SET_COLUMNS
            .iter()
            .filter_map(|column| column.filter)
            .find(|filter| filter.param == param)
            .map(|filter| filter.sql)
    }
}

impl ConnectionRepository for SqlxConnectionRepository {
    async fn create_template(&self, template: &ConnectionTemplate) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO connections_template (id, name, protocol, defaults)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(template.id)
        .bind(&template.name)
        .bind(template.protocol.as_str())
        .bind(&template.defaults)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create connection template: {}", e)))?;
        Ok(())
    }

    async fn get_template(
        &self,
        template_id: Uuid,
    ) -> Result<Option<ConnectionTemplate>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, protocol, defaults
            FROM connections_template
            WHERE id = $1
            "#,
        )
        .bind(template_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load connection template: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_template_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_templates(
        &self,
        filter: ConnectionTemplateFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ConnectionTemplate>, AppError> {
        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, protocol, defaults
            FROM connections_template
            "#,
        );
        let mut where_state = WhereState::new();
        Self::push_template_filter(&mut builder, &filter, &mut where_state);

        Self::push_order_by(
            &mut builder,
            filter.sort.as_ref(),
            CONNECTION_TEMPLATE_COLUMNS,
            "name ASC, id",
        );
        builder.push(" LIMIT ");
        builder.push_bind(clamp_limit(limit) as i64);
        builder.push(" OFFSET ");
        builder.push_bind(clamp_offset(offset) as i64);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list connection templates: {}", e))
        })?;
        rows.iter()
            .map(Self::parse_template_row)
            .collect::<Result<Vec<_>, _>>()
    }

    async fn count_templates(&self, filter: ConnectionTemplateFilter) -> Result<u64, AppError> {
        let mut builder = QueryBuilder::new(
            r#"
            SELECT COUNT(*) as count
            FROM connections_template
            "#,
        );
        let mut where_state = WhereState::new();
        Self::push_template_filter(&mut builder, &filter, &mut where_state);
        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count templates: {}", e)))?;
        let count: i64 = row
            .try_get("count")
            .map_err(|e| AppError::Database(format!("Failed to read template count: {}", e)))?;
        Ok(count as u64)
    }

    async fn create_session(&self, session: &ConnectionSession) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO connections_session (
                id,
                target_id,
                user_id,
                protocol,
                status,
                proxy_id,
                proxy_node_id,
                routing_source,
                client_ip,
                last_activity,
                session_metadata,
                started_at,
                ended_at
            )
            VALUES ($1, $2, $3, $4, $5::connection_status, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
        )
        .bind(session.id)
        .bind(session.target_id)
        .bind(session.user_id)
        .bind(session.protocol.as_str())
        .bind(session.status.as_str())
        .bind(session.proxy_id)
        .bind(session.proxy_node_id)
        .bind(session.routing_source.as_str())
        .bind(&session.client_ip)
        .bind(session.last_activity)
        .bind(&session.session_metadata)
        .bind(session.started_at)
        .bind(session.ended_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create session: {}", e)))?;
        Ok(())
    }

    async fn get_session(&self, session_id: Uuid) -> Result<Option<ConnectionSession>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id,
                   target_id,
                   user_id,
                   protocol,
                   status::text AS status,
                   proxy_id,
                   proxy_node_id,
                   routing_source,
                   client_ip,
                   last_activity,
                   session_metadata,
                   started_at,
                   ended_at
            FROM connections_session
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load connection session: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_session_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_sessions(
        &self,
        filter: ConnectionSessionFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ConnectionSession>, AppError> {
        let mut builder = QueryBuilder::new(
            r#"
            SELECT id,
                   target_id,
                   user_id,
                   protocol,
                   status::text AS status,
                   proxy_id,
                   proxy_node_id,
                   routing_source,
                   client_ip,
                   last_activity,
                   session_metadata,
                   started_at,
                   ended_at
            FROM connections_session
            "#,
        );
        let mut where_state = WhereState::new();
        Self::push_session_filter(&mut builder, &filter, &mut where_state);

        Self::push_order_by(
            &mut builder,
            filter.sort.as_ref(),
            CONNECTION_SESSION_COLUMNS,
            "started_at DESC, id",
        );
        builder.push(" LIMIT ");
        builder.push_bind(clamp_limit(limit) as i64);
        builder.push(" OFFSET ");
        builder.push_bind(clamp_offset(offset) as i64);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list connection sessions: {}", e))
        })?;
        rows.iter()
            .map(Self::parse_session_row)
            .collect::<Result<Vec<_>, _>>()
    }

    async fn count_sessions(&self, filter: ConnectionSessionFilter) -> Result<u64, AppError> {
        let mut builder = QueryBuilder::new(
            r#"
            SELECT COUNT(*) as count
            FROM connections_session
            "#,
        );
        let mut where_state = WhereState::new();
        Self::push_session_filter(&mut builder, &filter, &mut where_state);
        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count sessions: {}", e)))?;
        let count: i64 = row
            .try_get("count")
            .map_err(|e| AppError::Database(format!("Failed to read session count: {}", e)))?;
        Ok(count as u64)
    }

    async fn update_session(&self, session: &ConnectionSession) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE connections_session
            SET status = $2::connection_status,
                proxy_id = $3,
                proxy_node_id = $4,
                routing_source = $5,
                client_ip = $6,
                last_activity = $7,
                session_metadata = $8,
                started_at = $9,
                ended_at = $10
            WHERE id = $1
            "#,
        )
        .bind(session.id)
        .bind(session.status.as_str())
        .bind(session.proxy_id)
        .bind(session.proxy_node_id)
        .bind(session.routing_source.as_str())
        .bind(&session.client_ip)
        .bind(session.last_activity)
        .bind(&session.session_metadata)
        .bind(session.started_at)
        .bind(session.ended_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update session: {}", e)))?;
        Ok(())
    }

    async fn update_status(
        &self,
        session_id: Uuid,
        status: ConnectionStatus,
        ended_at: Option<DateTime<Utc>>,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE connections_session
            SET status = $2::connection_status, ended_at = $3
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .bind(status.as_str())
        .bind(ended_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update session status: {}", e)))?;
        Ok(())
    }

    async fn update_last_activity(
        &self,
        session_id: Uuid,
        last_activity: DateTime<Utc>,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE connections_session
            SET last_activity = $2
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .bind(last_activity)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to update session last_activity: {}", e))
        })?;
        Ok(())
    }

    async fn create_proxy_node(&self, proxy_node: &ProxyNode) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO connections_proxy_node (
                id, name, slug, base_url, enabled, secret_version, secret_rotated_at, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(proxy_node.id)
        .bind(&proxy_node.name)
        .bind(&proxy_node.slug)
        .bind(&proxy_node.base_url)
        .bind(proxy_node.enabled)
        .bind(proxy_node.secret_version)
        .bind(proxy_node.secret_rotated_at)
        .bind(proxy_node.created_at)
        .bind(proxy_node.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create proxy node: {}", e)))?;
        Ok(())
    }

    async fn get_proxy_node(&self, proxy_node_id: Uuid) -> Result<Option<ProxyNode>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, base_url, enabled, secret_version, secret_rotated_at, created_at, updated_at
            FROM connections_proxy_node
            WHERE id = $1
            "#,
        )
        .bind(proxy_node_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load proxy node: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_proxy_node_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn get_proxy_node_by_slug(&self, slug: &str) -> Result<Option<ProxyNode>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, slug, base_url, enabled, secret_version, secret_rotated_at, created_at, updated_at
            FROM connections_proxy_node
            WHERE slug = $1
            "#,
        )
        .bind(slug)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load proxy node by slug: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_proxy_node_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_proxy_nodes(
        &self,
        filter: ProxyNodeFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ProxyNode>, AppError> {
        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, name, slug, base_url, enabled, secret_version, secret_rotated_at, created_at, updated_at
            FROM connections_proxy_node
            "#,
        );
        let mut where_state = WhereState::new();
        Self::push_proxy_node_filter(&mut builder, &filter, &mut where_state);

        Self::push_order_by(
            &mut builder,
            filter.sort.as_ref(),
            PROXY_NODE_COLUMNS,
            "name ASC, id",
        );
        builder.push(" LIMIT ");
        builder.push_bind(clamp_limit(limit) as i64);
        builder.push(" OFFSET ");
        builder.push_bind(clamp_offset(offset) as i64);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list proxy nodes: {}", e)))?;
        rows.iter()
            .map(Self::parse_proxy_node_row)
            .collect::<Result<Vec<_>, _>>()
    }

    async fn count_proxy_nodes(&self, filter: ProxyNodeFilter) -> Result<u64, AppError> {
        let mut builder = QueryBuilder::new(
            r#"
            SELECT COUNT(*) as count
            FROM connections_proxy_node
            "#,
        );
        let mut where_state = WhereState::new();
        Self::push_proxy_node_filter(&mut builder, &filter, &mut where_state);
        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to count proxy nodes: {}", e)))?;
        let count: i64 = row
            .try_get("count")
            .map_err(|e| AppError::Database(format!("Failed to read proxy node count: {}", e)))?;
        Ok(count as u64)
    }

    async fn update_proxy_node(&self, proxy_node: &ProxyNode) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE connections_proxy_node
            SET name = $2,
                slug = $3,
                base_url = $4,
                enabled = $5,
                updated_at = $6
            WHERE id = $1
            "#,
        )
        .bind(proxy_node.id)
        .bind(&proxy_node.name)
        .bind(&proxy_node.slug)
        .bind(&proxy_node.base_url)
        .bind(proxy_node.enabled)
        .bind(proxy_node.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update proxy node: {}", e)))?;
        Ok(())
    }

    async fn mark_proxy_node_secret_rotated(
        &self,
        proxy_node_id: Uuid,
        rotated_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE connections_proxy_node
            SET secret_version = secret_version + 1,
                secret_rotated_at = $2,
                updated_at = $2
            WHERE id = $1
            "#,
        )
        .bind(proxy_node_id)
        .bind(rotated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to mark proxy node secret rotated: {}", e))
        })?;
        Ok(())
    }

    async fn delete_proxy_node(&self, proxy_node_id: Uuid) -> Result<bool, AppError> {
        let mut transaction = self.pool.begin().await?;
        if sqlx::query!(
            "SELECT id FROM connections_proxy_node WHERE id = $1 FOR UPDATE",
            proxy_node_id
        )
        .fetch_optional(&mut *transaction)
        .await?
        .is_none()
        {
            return Ok(false);
        }
        crate::contexts::secrets::infrastructure::persistence::repositories::secret_field_lifecycle::detach_consumer(
            &mut transaction, "proxy_node:credential", proxy_node_id, "proxy_node", proxy_node_id,
        ).await?;
        // `connections_proxy_assignment.proxy_node_id` is ON DELETE RESTRICT:
        // a node still carrying assignments must answer 409 DELETE_BLOCKED
        // naming them, not a generic FK 400. Sessions reference the node with
        // ON DELETE SET NULL and are deliberately not counted as blockers.
        let deleted = crate::contexts::shared::delete_guard::delete_record_guarded_in_transaction(
            &mut transaction,
            "connections_proxy_node",
            proxy_node_id,
            crate::contexts::shared::delete_guard::no_blocker_metadata,
        )
        .await?;
        transaction.commit().await?;
        Ok(deleted > 0)
    }

    async fn create_proxy_node_assignment(
        &self,
        input: &NewProxyNodeAssignment,
    ) -> Result<ProxyNodeAssignment, AppError> {
        let query = format!(
            "INSERT INTO connections_proxy_assignment \
             (proxy_node_id, scope_type, scope_id, weight, enabled) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING {PROXY_ASSIGNMENT_COLUMNS}"
        );
        let row = sqlx::query(&query)
            .bind(input.proxy_node_id)
            .bind(input.scope_type.as_str())
            .bind(input.scope_id)
            .bind(input.weight.unwrap_or(0))
            .bind(input.enabled.unwrap_or(true))
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to create proxy node assignment: {}", e))
            })?;
        Self::parse_proxy_node_assignment_row(&row)
    }

    async fn get_proxy_node_assignment(
        &self,
        assignment_id: Uuid,
    ) -> Result<Option<ProxyNodeAssignment>, AppError> {
        let query = format!(
            "SELECT {PROXY_ASSIGNMENT_COLUMNS} FROM connections_proxy_assignment WHERE id = $1"
        );
        let row = sqlx::query(&query)
            .bind(assignment_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to load proxy node assignment: {}", e))
            })?;
        row.as_ref()
            .map(Self::parse_proxy_node_assignment_row)
            .transpose()
    }

    async fn list_proxy_node_assignments(
        &self,
        filter: ProxyNodeAssignmentFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<ProxyNodeAssignment>, AppError> {
        let mut builder = QueryBuilder::new(format!(
            "SELECT {PROXY_ASSIGNMENT_COLUMNS} FROM connections_proxy_assignment"
        ));
        let mut where_state = WhereState::new();
        Self::push_proxy_node_assignment_filter(&mut builder, &filter, &mut where_state);
        Self::push_order_by(
            &mut builder,
            filter.sort.as_ref(),
            PROXY_NODE_ASSIGNMENT_COLUMNS,
            "scope_type ASC, weight DESC, id ASC",
        );
        builder.push(" LIMIT ");
        builder.push_bind(clamp_limit(limit) as i64);
        builder.push(" OFFSET ");
        builder.push_bind(clamp_offset(offset) as i64);

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list proxy node assignments: {}", e))
        })?;
        rows.iter()
            .map(Self::parse_proxy_node_assignment_row)
            .collect()
    }

    async fn count_proxy_node_assignments(
        &self,
        filter: ProxyNodeAssignmentFilter,
    ) -> Result<u64, AppError> {
        let mut builder =
            QueryBuilder::new("SELECT COUNT(*) AS count FROM connections_proxy_assignment");
        let mut where_state = WhereState::new();
        Self::push_proxy_node_assignment_filter(&mut builder, &filter, &mut where_state);
        let row = builder.build().fetch_one(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to count proxy node assignments: {}", e))
        })?;
        let count: i64 = row.try_get("count").map_err(|e| {
            AppError::Database(format!("Failed to read proxy node assignment count: {}", e))
        })?;
        Ok(count as u64)
    }

    async fn update_proxy_node_assignment(
        &self,
        assignment_id: Uuid,
        input: &UpdateProxyNodeAssignment,
    ) -> Result<Option<ProxyNodeAssignment>, AppError> {
        let new_scope_type = input.scope_type.map(|scope| scope.as_str().to_string());
        let query = format!(
            "UPDATE connections_proxy_assignment SET \
               proxy_node_id = COALESCE($2, proxy_node_id), \
               scope_type = COALESCE($3, scope_type), \
               scope_id = CASE WHEN $3 IS NULL THEN scope_id ELSE $4 END, \
               weight = COALESCE($5, weight), \
               enabled = COALESCE($6, enabled), \
               updated_at = now() \
             WHERE id = $1 \
             RETURNING {PROXY_ASSIGNMENT_COLUMNS}"
        );
        let row = sqlx::query(&query)
            .bind(assignment_id)
            .bind(input.proxy_node_id)
            .bind(new_scope_type)
            .bind(input.scope_id)
            .bind(input.weight)
            .bind(input.enabled)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to update proxy node assignment: {}", e))
            })?;
        row.as_ref()
            .map(Self::parse_proxy_node_assignment_row)
            .transpose()
    }

    async fn delete_proxy_node_assignment(&self, assignment_id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM connections_proxy_assignment WHERE id = $1")
            .bind(assignment_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!("Failed to delete proxy node assignment: {}", e))
            })?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_proxy_node_assignments_for_scopes(
        &self,
        scopes: &[(ConnConfScope, Option<Uuid>)],
    ) -> Result<Vec<ProxyNodeAssignment>, AppError> {
        let nil = "00000000-0000-0000-0000-000000000000";
        let mut params: Vec<(String, Uuid)> = Vec::with_capacity(scopes.len());
        for (scope_type, scope_id) in scopes {
            params.push((
                scope_type.as_str().to_string(),
                scope_id.unwrap_or_else(Uuid::nil),
            ));
        }
        if params.is_empty() {
            return Ok(Vec::new());
        }

        let mut clauses = Vec::with_capacity(params.len());
        for (i, _) in params.iter().enumerate() {
            let t = i * 2 + 1;
            let v = i * 2 + 2;
            clauses.push(format!(
                "(scope_type = ${t} AND COALESCE(scope_id, '{nil}') = ${v})"
            ));
        }
        let query = format!(
            "SELECT {PROXY_ASSIGNMENT_COLUMNS} FROM connections_proxy_assignment \
             WHERE enabled AND ({})",
            clauses.join(" OR ")
        );
        let mut q = sqlx::query(&query);
        for (scope_type, scope_id) in &params {
            q = q.bind(scope_type).bind(*scope_id);
        }
        let rows = q.fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!(
                "Failed to list scoped proxy node assignments: {}",
                e
            ))
        })?;
        rows.iter()
            .map(Self::parse_proxy_node_assignment_row)
            .collect()
    }

    async fn get_target_proxy_assignment(
        &self,
        target_id: Uuid,
    ) -> Result<Option<TargetProxyAssignment>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT
                $1::uuid AS target_id,
                a.proxy_node_id,
                a.updated_at
            FROM connections_proxy_assignment a
            WHERE a.scope_id = $1
              AND a.scope_type IN ('device', 'vm')
              AND a.enabled
            ORDER BY
                a.weight DESC,
                a.id DESC
            LIMIT 1
            "#,
        )
        .bind(target_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to load target proxy assignment: {}", e))
        })?;

        match row {
            Some(row) => Ok(Some(Self::parse_target_proxy_assignment_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn upsert_target_proxy_assignment(
        &self,
        assignment: &TargetProxyAssignment,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            WITH target_scope AS (
                SELECT 'device'::varchar AS scope_type
                FROM dcim_device
                WHERE id = $1
                UNION ALL
                SELECT 'vm'::varchar AS scope_type
                FROM virtualization_virtualmachine
                WHERE id = $1
                LIMIT 1
            ),
            cleared AS (
                DELETE FROM connections_proxy_assignment existing
                USING target_scope
                WHERE existing.scope_type = target_scope.scope_type
                  AND existing.scope_id = $1
            )
            INSERT INTO connections_proxy_assignment (
                proxy_node_id, scope_type, scope_id, weight, enabled, created_at, updated_at
            )
            SELECT $2, target_scope.scope_type, $1, 0, true, $3, $3
            FROM target_scope
            "#,
        )
        .bind(assignment.target_id)
        .bind(assignment.proxy_node_id)
        .bind(assignment.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to upsert target proxy assignment: {}", e))
        })?;
        Ok(())
    }

    async fn clear_target_proxy_assignment(&self, target_id: Uuid) -> Result<(), AppError> {
        sqlx::query(
            r#"
            DELETE FROM connections_proxy_assignment
            WHERE scope_id = $1
              AND scope_type IN ('device', 'vm')
            "#,
        )
        .bind(target_id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to clear target proxy assignment: {}", e))
        })?;
        Ok(())
    }

    async fn is_endpoint_bound_to_target(
        &self,
        target_id: Uuid,
        endpoint_host: &str,
    ) -> Result<bool, AppError> {
        let row = sqlx::query(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM ipam_ipaddress ip
                JOIN dcim_interface di ON di.id = ip.assigned_object_id
                WHERE di.device_id = $1
                  AND host(ip.address) = host($2::inet)
                UNION ALL
                SELECT 1
                FROM ipam_ipaddress ip
                JOIN virtualization_vminterface vi ON vi.id = ip.assigned_object_id
                WHERE vi.virtual_machine_id = $1
                  AND host(ip.address) = host($2::inet)
            ) AS is_bound
            "#,
        )
        .bind(target_id)
        .bind(endpoint_host)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            AppError::Database(format!(
                "Failed to verify endpoint binding for host {} and endpoint {}: {}",
                target_id, endpoint_host, e
            ))
        })?;
        row.try_get("is_bound")
            .map_err(|e| AppError::Database(format!("Failed to read endpoint binding flag: {}", e)))
    }

    async fn create_user_credential_set(&self, set: &UserCredentialSet) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO connections_user_credential_set (
                id, user_id, protocol, name, params, is_default, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(set.id)
        .bind(set.user_id)
        .bind(set.protocol.as_str())
        .bind(&set.name)
        .bind(&set.params)
        .bind(set.is_default)
        .bind(set.created_at)
        .bind(set.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create user credential set: {}", e)))?;
        Ok(())
    }

    async fn get_user_credential_set(
        &self,
        set_id: Uuid,
    ) -> Result<Option<UserCredentialSet>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, user_id, protocol, name, params, is_default, created_at, updated_at
            FROM connections_user_credential_set
            WHERE id = $1
            "#,
        )
        .bind(set_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load user credential set: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_user_credential_set_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_user_credential_sets(
        &self,
        filter: UserCredentialSetFilter,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<(Vec<UserCredentialSet>, i64), AppError> {
        if filter.user_id.is_empty() {
            return Err(AppError::Validation(
                "user_id filter is required for user credential sets".to_string(),
            ));
        }

        let mut builder = QueryBuilder::new(
            r#"
            SELECT id, user_id, protocol, name, params, is_default, created_at, updated_at,
                COUNT(*) OVER() AS total_count
            FROM connections_user_credential_set
            "#,
        );
        let mut where_state = WhereState::new();
        if let Some(sql) = Self::user_credential_set_filter_sql("user_id") {
            push_uuid_in(&mut builder, &mut where_state, sql, &filter.user_id);
        }
        if let Some(sql) = Self::user_credential_set_filter_sql("protocol") {
            let protocols: Vec<String> = filter
                .protocol
                .iter()
                .map(|protocol| protocol.as_str().to_string())
                .collect();
            push_string_in(&mut builder, &mut where_state, sql, &protocols);
        }
        if let Some(sql) = Self::user_credential_set_filter_sql("is_default") {
            push_string_in(&mut builder, &mut where_state, sql, &filter.is_default);
        }
        Self::push_order_by(
            &mut builder,
            filter.sort.as_ref(),
            USER_CREDENTIAL_SET_COLUMNS,
            "protocol ASC, is_default DESC, updated_at DESC, id",
        );
        if let Some(limit) = limit {
            builder.push(" LIMIT ");
            builder.push_bind(limit);
        }
        if let Some(offset) = offset {
            builder.push(" OFFSET ");
            builder.push_bind(offset);
        }

        let rows = builder.build().fetch_all(&self.pool).await.map_err(|e| {
            AppError::Database(format!("Failed to list user credential sets: {}", e))
        })?;
        // The window function repeats the unpaged match count on every row; an
        // empty page means there is nothing to count.
        let total = match rows.first() {
            Some(row) => row.try_get("total_count").map_err(|error| {
                AppError::Database(format!(
                    "Failed to read user credential set total_count: {error}"
                ))
            })?,
            None => 0,
        };
        let items = rows
            .iter()
            .map(Self::parse_user_credential_set_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((items, total))
    }

    async fn update_user_credential_set(&self, set: &UserCredentialSet) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE connections_user_credential_set
            SET name = $2, params = $3, is_default = $4, updated_at = $5
            WHERE id = $1
            "#,
        )
        .bind(set.id)
        .bind(&set.name)
        .bind(&set.params)
        .bind(set.is_default)
        .bind(set.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update user credential set: {}", e)))?;
        Ok(())
    }

    async fn delete_user_credential_set(&self, set_id: Uuid) -> Result<(), AppError> {
        let mut transaction = self.pool.begin().await?;
        let Some(set) = sqlx::query!(
            "SELECT user_id FROM connections_user_credential_set WHERE id = $1",
            set_id
        )
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(());
        };
        // Parent disappearance cascades this set; its following row lock
        // determines idempotent absence without a parent RowNotFound error.
        sqlx::query!(
            "SELECT id FROM identity_user WHERE id = $1 FOR UPDATE",
            set.user_id
        )
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(locked) = sqlx::query!(
            "SELECT user_id FROM connections_user_credential_set WHERE id = $1 FOR UPDATE",
            set_id
        )
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(());
        };
        if locked.user_id != set.user_id {
            return Err(AppError::Conflict(
                "Credential set ownership changed during deletion".into(),
            ));
        }
        crate::contexts::secrets::infrastructure::persistence::repositories::secret_field_lifecycle::detach_consumer(
            &mut transaction, "user_connection_credential_set", set_id, "user", locked.user_id,
        ).await?;
        sqlx::query(
            r#"
            DELETE FROM connections_user_credential_set
            WHERE id = $1
            "#,
        )
        .bind(set_id)
        .execute(&mut *transaction)
        .await
        .map_err(|e| AppError::Database(format!("Failed to delete user credential set: {}", e)))?;
        transaction.commit().await?;
        Ok(())
    }

    async fn set_default_user_credential_set(
        &self,
        user_id: Uuid,
        protocol: crate::contexts::connections::domain::ConnectionProtocol,
        set_id: Uuid,
        updated_at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Database(format!("Failed to start transaction: {}", e)))?;
        sqlx::query(
            r#"
            UPDATE connections_user_credential_set
            SET is_default = FALSE, updated_at = $3
            WHERE user_id = $1 AND protocol = $2 AND id <> $4
            "#,
        )
        .bind(user_id)
        .bind(protocol.as_str())
        .bind(updated_at)
        .bind(set_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            AppError::Database(format!("Failed to clear default credential set: {}", e))
        })?;
        let result = sqlx::query(
            r#"
            UPDATE connections_user_credential_set
            SET is_default = TRUE, updated_at = $4
            WHERE id = $1 AND user_id = $2 AND protocol = $3
            "#,
        )
        .bind(set_id)
        .bind(user_id)
        .bind(protocol.as_str())
        .bind(updated_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to set default credential set: {}", e)))?;
        if result.rows_affected() == 0 {
            return Err(AppError::NotFound("Credential set not found".to_string()));
        }
        tx.commit()
            .await
            .map_err(|e| AppError::Database(format!("Failed to commit transaction: {}", e)))?;
        Ok(())
    }

    async fn resolve_host_key_lines(&self, host: &str, port: u16) -> Result<Vec<String>, AppError> {
        crate::contexts::shared::host_key_pins::resolve_seed_lines(&self.pool, host, port).await
    }

    async fn persist_observed_host_key_lines(&self, lines: &[String]) -> Result<(), AppError> {
        crate::contexts::shared::host_key_pins::upsert_lines(&self.pool, lines).await
    }
}
