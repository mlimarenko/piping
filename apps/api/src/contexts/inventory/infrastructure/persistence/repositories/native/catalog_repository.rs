//! Catalog persistence for application roles, applications, hierarchical flow
//! groups and reusable address groups.
//!
//! Runtime sqlx only (`sqlx::query` / `QueryBuilder` + `.bind`) so the crate
//! type-checks without a database. Hierarchy and application-consistency rules
//! are database-enforced; this module surfaces them through the shared SQLSTATE
//! classifier (`23514` -> validation, `23505`/`23001` -> conflict) and never
//! rewrites a related record to make a mutation succeed.
//!
//! Address-group members are selector rows (`side = 'member'`, `group_id = the
//! group`), ordered by `position`. Nesting is impossible by construction: the
//! member validator rejects `inventory.addressgroup` and this module never
//! writes `addressgroup_id` for a member.

use std::collections::{BTreeMap, BTreeSet};

use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use uuid::Uuid;

use crate::contexts::inventory::domain::native::catalog::{
    AddressGroup, AddressGroupMember, Application, ApplicationRole, DataflowGroup,
    parse_address_mode, parse_status, parse_stored_tag, selector_column, status_rank,
};
use crate::contexts::inventory::domain::native::dataflow::{
    DataflowEndpointInput, DataflowStatus, EndpointType,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::delete_guard::{
    delete_inventory_record, delete_inventory_record_in_transaction,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::{
    begin_native_write, lock_tag_catalog_for_usage,
};
use crate::contexts::shared::list_query::{
    WhereState, push_search_ilike, push_string_in, push_uuid_in,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

use super::dataflow_repository::validate_mutation_selectors;

const APPLICATION_ROLE_TABLE: &str = "inventory_application_role";
const APPLICATION_TABLE: &str = "inventory_application";
const DATAFLOW_GROUP_TABLE: &str = "inventory_dataflow_group";
const ADDRESS_GROUP_TABLE: &str = "inventory_address_group";

/// Bound of the ancestor walk. The schema enforces the same depth ceiling, so a
/// stored cycle cannot make this read unbounded.
const MAX_GROUP_DEPTH: i32 = 64;

const GROUP_SELECT: &str = "g.id, g.parent_id, g.application_id, g.name, g.description, \
     g.comments, g.status, g.owner_id, g.tenant_id, \
     COALESCE(g.tags, '{}'::text[]) AS tags, \
     COALESCE(g.custom_field_data, '{}'::jsonb) AS custom_field_data";

const ADDRESS_GROUP_SELECT: &str = "ag.id, ag.name, ag.description, ag.comments, ag.owner_id, \
     ag.tenant_id, COALESCE(ag.tags, '{}'::text[]) AS tags, \
     COALESCE(ag.custom_field_data, '{}'::jsonb) AS custom_field_data";

const GROUP_ANCESTORS_SQL: &str = r#"
WITH RECURSIVE chain AS (
    SELECT g.id AS root_id, p.id AS ancestor_id, p.parent_id, p.status,
           COALESCE(p.tags, '{}'::text[]) AS tags, 1::int AS depth
    FROM inventory_dataflow_group g
    JOIN inventory_dataflow_group p ON p.id = g.parent_id
    WHERE g.id = ANY($1::uuid[])
    UNION ALL
    SELECT c.root_id, p.id, p.parent_id, p.status,
           COALESCE(p.tags, '{}'::text[]), c.depth + 1
    FROM chain c
    JOIN inventory_dataflow_group p ON p.id = c.parent_id
    WHERE c.depth < $2
)
SELECT root_id, ancestor_id, status, tags
FROM chain
ORDER BY root_id, depth, ancestor_id
"#;

#[derive(Debug, sqlx::FromRow)]
struct ApplicationRoleRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    comments: Option<String>,
    owner_id: Option<Uuid>,
}

impl From<ApplicationRoleRow> for ApplicationRole {
    fn from(row: ApplicationRoleRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            description: row.description,
            comments: row.comments,
            owner_id: row.owner_id,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ApplicationRow {
    id: Uuid,
    name: String,
    role_id: Option<Uuid>,
    description: Option<String>,
    comments: Option<String>,
    owner_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    tags: Vec<String>,
    custom_field_data: serde_json::Value,
}

impl From<ApplicationRow> for Application {
    fn from(row: ApplicationRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            role_id: row.role_id,
            description: row.description,
            comments: row.comments,
            owner_id: row.owner_id,
            tenant_id: row.tenant_id,
            tags: row.tags,
            custom_field_data: row.custom_field_data,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct GroupRow {
    id: Uuid,
    parent_id: Option<Uuid>,
    application_id: Option<Uuid>,
    name: String,
    description: Option<String>,
    comments: Option<String>,
    status: String,
    owner_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    tags: Vec<String>,
    custom_field_data: serde_json::Value,
}

impl TryFrom<GroupRow> for DataflowGroup {
    type Error = AppError;

    fn try_from(row: GroupRow) -> Result<Self, AppError> {
        let status = parse_status(&row.status)?;
        Ok(Self {
            id: row.id,
            parent_id: row.parent_id,
            application_id: row.application_id,
            name: row.name,
            description: row.description,
            comments: row.comments,
            status,
            owner_id: row.owner_id,
            tenant_id: row.tenant_id,
            tags: row.tags,
            custom_field_data: row.custom_field_data,
            effective_status: status,
            blocked_by_group_id: None,
            inherited_tags: Vec::new(),
            effective_tags: Vec::new(),
        })
    }
}

#[derive(Debug, sqlx::FromRow)]
struct GroupAncestorRow {
    root_id: Uuid,
    ancestor_id: Uuid,
    status: String,
    tags: Vec<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct AddressGroupRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    comments: Option<String>,
    owner_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    tags: Vec<String>,
    custom_field_data: serde_json::Value,
}

impl From<AddressGroupRow> for AddressGroup {
    fn from(row: AddressGroupRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            description: row.description,
            comments: row.comments,
            owner_id: row.owner_id,
            tenant_id: row.tenant_id,
            tags: row.tags,
            custom_field_data: row.custom_field_data,
            members: Vec::new(),
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct MemberRow {
    group_id: Uuid,
    position: i16,
    object_type: String,
    address_mode: String,
    tag_object_type: Option<String>,
    object_id: Option<Uuid>,
    ipaddresses: Vec<Uuid>,
}

/// Role list filter. `search` is already an `ILIKE` pattern; `owner_id` is
/// matched with a parameterized `IN` predicate.
#[derive(Debug, Clone, Default)]
pub struct ApplicationRoleFilter {
    pub search: Option<String>,
    pub owner_id: Vec<Uuid>,
}

/// Application list filter.
#[derive(Debug, Clone, Default)]
pub struct ApplicationFilter {
    pub search: Option<String>,
    pub role_id: Vec<Uuid>,
    pub tenant_id: Vec<Uuid>,
    pub owner_id: Vec<Uuid>,
    /// Direct tag memberships; a row must carry all requested tag ids.
    pub tag_id: Vec<Uuid>,
}

/// Flow-group list filter. `parent_id` and `root` are distinct requests: an
/// omitted parent is no predicate, `root` selects rows with `parent_id IS NULL`.
#[derive(Debug, Clone, Default)]
pub struct DataflowGroupFilter {
    pub search: Option<String>,
    /// Stored status, not the ancestry-derived effective status.
    pub status: Vec<String>,
    pub application_id: Vec<Uuid>,
    pub parent_id: Option<Uuid>,
    pub root: bool,
    pub tenant_id: Vec<Uuid>,
    pub owner_id: Vec<Uuid>,
    pub tag_id: Vec<Uuid>,
}

/// Address-group list filter.
#[derive(Debug, Clone, Default)]
pub struct AddressGroupFilter {
    pub search: Option<String>,
    pub tenant_id: Vec<Uuid>,
    pub owner_id: Vec<Uuid>,
    pub tag_id: Vec<Uuid>,
}

/// Field-level patch. `None` preserves, `Some(None)` clears a nullable field.
#[derive(Debug, Clone, Default)]
pub struct ApplicationRoleChanges {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub comments: Option<Option<String>>,
    pub owner_id: Option<Option<Uuid>>,
}

impl ApplicationRoleChanges {
    fn apply(self, role: &mut ApplicationRole) {
        if let Some(name) = self.name {
            role.name = name;
        }
        if let Some(description) = self.description {
            role.description = description;
        }
        if let Some(comments) = self.comments {
            role.comments = comments;
        }
        if let Some(owner_id) = self.owner_id {
            role.owner_id = owner_id;
        }
    }
}

/// Field-level patch for an application.
#[derive(Debug, Clone, Default)]
pub struct ApplicationChanges {
    pub name: Option<String>,
    pub role_id: Option<Option<Uuid>>,
    pub description: Option<Option<String>>,
    pub comments: Option<Option<String>>,
    pub owner_id: Option<Option<Uuid>>,
    pub tenant_id: Option<Option<Uuid>>,
    pub tags: Option<Vec<String>>,
    pub custom_field_data: Option<serde_json::Value>,
}

impl ApplicationChanges {
    fn apply(self, application: &mut Application) {
        if let Some(name) = self.name {
            application.name = name;
        }
        if let Some(role_id) = self.role_id {
            application.role_id = role_id;
        }
        if let Some(description) = self.description {
            application.description = description;
        }
        if let Some(comments) = self.comments {
            application.comments = comments;
        }
        if let Some(owner_id) = self.owner_id {
            application.owner_id = owner_id;
        }
        if let Some(tenant_id) = self.tenant_id {
            application.tenant_id = tenant_id;
        }
        if let Some(tags) = self.tags {
            application.tags = tags;
        }
        if let Some(custom_field_data) = self.custom_field_data {
            application.custom_field_data = custom_field_data;
        }
    }
}

/// Field-level patch for a flow group.
#[derive(Debug, Clone, Default)]
pub struct DataflowGroupChanges {
    pub name: Option<String>,
    pub parent_id: Option<Option<Uuid>>,
    pub application_id: Option<Option<Uuid>>,
    pub description: Option<Option<String>>,
    pub comments: Option<Option<String>>,
    pub status: Option<DataflowStatus>,
    pub owner_id: Option<Option<Uuid>>,
    pub tenant_id: Option<Option<Uuid>>,
    pub tags: Option<Vec<String>>,
    pub custom_field_data: Option<serde_json::Value>,
}

impl DataflowGroupChanges {
    fn apply(self, group: &mut DataflowGroup) {
        if let Some(name) = self.name {
            group.name = name;
        }
        if let Some(parent_id) = self.parent_id {
            group.parent_id = parent_id;
        }
        if let Some(application_id) = self.application_id {
            group.application_id = application_id;
        }
        if let Some(description) = self.description {
            group.description = description;
        }
        if let Some(comments) = self.comments {
            group.comments = comments;
        }
        if let Some(status) = self.status {
            group.status = status;
        }
        if let Some(owner_id) = self.owner_id {
            group.owner_id = owner_id;
        }
        if let Some(tenant_id) = self.tenant_id {
            group.tenant_id = tenant_id;
        }
        if let Some(tags) = self.tags {
            group.tags = tags;
        }
        if let Some(custom_field_data) = self.custom_field_data {
            group.custom_field_data = custom_field_data;
        }
    }
}

/// Field-level patch for an address group. `members` replaces the whole ordered
/// member set when supplied.
#[derive(Debug, Clone, Default)]
pub struct AddressGroupChanges {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub comments: Option<Option<String>>,
    pub owner_id: Option<Option<Uuid>>,
    pub tenant_id: Option<Option<Uuid>>,
    pub tags: Option<Vec<String>>,
    pub custom_field_data: Option<serde_json::Value>,
    pub members: Option<Vec<AddressGroupMember>>,
}

impl AddressGroupChanges {
    fn apply(self, group: &mut AddressGroup) {
        if let Some(name) = self.name {
            group.name = name;
        }
        if let Some(description) = self.description {
            group.description = description;
        }
        if let Some(comments) = self.comments {
            group.comments = comments;
        }
        if let Some(owner_id) = self.owner_id {
            group.owner_id = owner_id;
        }
        if let Some(tenant_id) = self.tenant_id {
            group.tenant_id = tenant_id;
        }
        if let Some(tags) = self.tags {
            group.tags = tags;
        }
        if let Some(custom_field_data) = self.custom_field_data {
            group.custom_field_data = custom_field_data;
        }
        if let Some(members) = self.members {
            group.members = members;
        }
    }
}

#[derive(Clone)]
pub struct SqlxCatalogRepository {
    pool: PgPool,
}

impl SqlxCatalogRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ---- filters -----------------------------------------------------------

    fn push_tag_filter(
        builder: &mut QueryBuilder<Postgres>,
        state: &mut WhereState,
        column: &str,
        tag_id: &[Uuid],
    ) {
        if tag_id.is_empty() {
            return;
        }
        let tags: Vec<String> = tag_id.iter().map(Uuid::to_string).collect();
        state.and(builder);
        builder.push(column);
        builder.push(" @> ");
        builder.push_bind(tags);
        builder.push("::text[]");
    }

    fn push_role_filters(builder: &mut QueryBuilder<Postgres>, filter: &ApplicationRoleFilter) {
        let mut state = WhereState::new();
        push_search_ilike(
            builder,
            &mut state,
            &["r.name", "r.description"],
            filter.search.as_deref(),
        );
        push_uuid_in(builder, &mut state, "r.owner_id", &filter.owner_id);
    }

    fn push_application_filters(builder: &mut QueryBuilder<Postgres>, filter: &ApplicationFilter) {
        let mut state = WhereState::new();
        push_search_ilike(
            builder,
            &mut state,
            &["a.name", "a.description"],
            filter.search.as_deref(),
        );
        push_uuid_in(builder, &mut state, "a.role_id", &filter.role_id);
        push_uuid_in(builder, &mut state, "a.tenant_id", &filter.tenant_id);
        push_uuid_in(builder, &mut state, "a.owner_id", &filter.owner_id);
        Self::push_tag_filter(builder, &mut state, "a.tags", &filter.tag_id);
    }

    fn push_group_filters(builder: &mut QueryBuilder<Postgres>, filter: &DataflowGroupFilter) {
        let mut state = WhereState::new();
        push_search_ilike(
            builder,
            &mut state,
            &["g.name", "g.description"],
            filter.search.as_deref(),
        );
        push_string_in(builder, &mut state, "g.status", &filter.status);
        push_uuid_in(
            builder,
            &mut state,
            "g.application_id",
            &filter.application_id,
        );
        if let Some(parent_id) = filter.parent_id {
            state.and(builder);
            builder.push("g.parent_id = ");
            builder.push_bind(parent_id);
        } else if filter.root {
            state.and(builder);
            builder.push("g.parent_id IS NULL");
        }
        push_uuid_in(builder, &mut state, "g.tenant_id", &filter.tenant_id);
        push_uuid_in(builder, &mut state, "g.owner_id", &filter.owner_id);
        Self::push_tag_filter(builder, &mut state, "g.tags", &filter.tag_id);
    }

    fn push_address_group_filters(
        builder: &mut QueryBuilder<Postgres>,
        filter: &AddressGroupFilter,
    ) {
        let mut state = WhereState::new();
        push_search_ilike(
            builder,
            &mut state,
            &["ag.name", "ag.description"],
            filter.search.as_deref(),
        );
        push_uuid_in(builder, &mut state, "ag.tenant_id", &filter.tenant_id);
        push_uuid_in(builder, &mut state, "ag.owner_id", &filter.owner_id);
        Self::push_tag_filter(builder, &mut state, "ag.tags", &filter.tag_id);
    }

    // ---- group ancestry ----------------------------------------------------

    /// One bounded recursive ancestor walk for a whole page.
    async fn hydrate_groups(
        &self,
        mut groups: Vec<DataflowGroup>,
    ) -> Result<Vec<DataflowGroup>, AppError> {
        if groups.is_empty() {
            return Ok(groups);
        }
        let ids: Vec<Uuid> = groups.iter().map(|group| group.id).collect();
        let rows = sqlx::query_as::<_, GroupAncestorRow>(GROUP_ANCESTORS_SQL)
            .bind(&ids)
            .bind(MAX_GROUP_DEPTH)
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut chains: BTreeMap<Uuid, Vec<(DataflowStatus, Uuid, Vec<String>)>> = BTreeMap::new();
        for row in rows {
            chains.entry(row.root_id).or_default().push((
                parse_status(&row.status)?,
                row.ancestor_id,
                row.tags,
            ));
        }
        for group in groups.iter_mut() {
            let chain = chains.remove(&group.id).unwrap_or_default();
            let own_rank = status_rank(group.status);
            let mut inherited: Vec<Uuid> = Vec::new();
            let mut blocking: Option<(DataflowStatus, Uuid)> = None;
            for (status, ancestor_id, tags) in chain {
                // Rows arrive nearest-first, so the first ancestor reaching the
                // strictest rank is the nearest group that explains the state.
                if blocking.is_none_or(|(current, _)| status_rank(status) > status_rank(current)) {
                    blocking = Some((status, ancestor_id));
                }
                for tag in tags {
                    inherited.push(parse_stored_tag(&tag)?);
                }
            }
            match blocking {
                Some((status, ancestor_id)) if status_rank(status) > own_rank => {
                    group.effective_status = status;
                    group.blocked_by_group_id = Some(ancestor_id);
                }
                _ => {
                    group.effective_status = group.status;
                    group.blocked_by_group_id = None;
                }
            }
            inherited.sort_unstable();
            inherited.dedup();
            let mut effective: Vec<Uuid> = group
                .tags
                .iter()
                .map(|tag| parse_stored_tag(tag))
                .collect::<Result<Vec<Uuid>, AppError>>()?;
            effective.extend(inherited.iter().copied());
            effective.sort_unstable();
            effective.dedup();
            group.inherited_tags = inherited;
            group.effective_tags = effective;
        }
        Ok(groups)
    }

    // ---- address-group members --------------------------------------------

    async fn load_members(
        &self,
        group_ids: &[Uuid],
    ) -> Result<BTreeMap<Uuid, Vec<AddressGroupMember>>, AppError> {
        if group_ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let references = EndpointType::REFERENCES
            .iter()
            .map(|entry| format!("s.{}", entry.2))
            .collect::<Vec<_>>()
            .join(", ");
        let mut query = QueryBuilder::<Postgres>::new(format!(
            "SELECT s.group_id, s.position, s.object_type, s.address_mode, s.tag_object_type, \
             COALESCE({references}) AS object_id, \
             ARRAY(SELECT x.ipaddress_id FROM inventory_dataflow_selector_ip x \
             WHERE x.selector_id = s.id ORDER BY x.ipaddress_id) AS ipaddresses \
             FROM inventory_dataflow_selector s WHERE s.group_id = ANY("
        ));
        query.push_bind(group_ids);
        query.push("::uuid[]) AND s.side = 'member' ORDER BY s.group_id, s.position, s.id");
        let rows = query
            .build_query_as::<MemberRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut members: BTreeMap<Uuid, Vec<AddressGroupMember>> = BTreeMap::new();
        for row in rows {
            let group_id = row.group_id;
            members
                .entry(group_id)
                .or_default()
                .push(member_from_row(row)?);
        }
        let labels = self.member_labels(members.values().flatten()).await?;
        for member in members.values_mut().flatten() {
            if let Some(id) = member.selector.object_id {
                member.label = labels.get(&(member.selector.object_type, id)).cloned();
            }
        }
        Ok(members)
    }

    async fn member_labels(
        &self,
        members: impl Iterator<Item = &AddressGroupMember>,
    ) -> Result<BTreeMap<(EndpointType, Uuid), String>, AppError> {
        let mut demanded: BTreeMap<EndpointType, BTreeSet<Uuid>> = BTreeMap::new();
        for member in members {
            if let Some(id) = member.selector.object_id {
                demanded
                    .entry(member.selector.object_type)
                    .or_default()
                    .insert(id);
            }
        }
        let mut labels = BTreeMap::new();
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
            let mut query = QueryBuilder::<Postgres>::new(format!(
                "SELECT id, {column} FROM {table} WHERE id = ANY("
            ));
            query.push_bind(ids.iter().copied().collect::<Vec<_>>());
            query.push("::uuid[])");
            let rows = query
                .build_query_as::<(Uuid, Option<String>)>()
                .fetch_all(&self.pool)
                .await
                .map_err(classify_sqlx_error_owned)?;
            for (id, label) in rows {
                if let Some(label) = label {
                    labels.insert((kind, id), label);
                }
            }
        }
        Ok(labels)
    }

    /// Validate members with the same selector owner as flow create/update.
    async fn validate_members(
        transaction: &mut Transaction<'_, Postgres>,
        members: &[AddressGroupMember],
    ) -> Result<(), AppError> {
        let selectors: Vec<&DataflowEndpointInput> =
            members.iter().map(|member| &member.selector).collect();
        validate_mutation_selectors(&mut **transaction, &selectors).await
    }

    async fn replace_members(
        transaction: &mut Transaction<'_, Postgres>,
        group_id: Uuid,
        members: &[AddressGroupMember],
    ) -> Result<(), AppError> {
        sqlx::query(
            "DELETE FROM inventory_dataflow_selector_ip WHERE selector_id IN \
             (SELECT id FROM inventory_dataflow_selector WHERE group_id = $1)",
        )
        .bind(group_id)
        .execute(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
        sqlx::query("DELETE FROM inventory_dataflow_selector WHERE group_id = $1")
            .bind(group_id)
            .execute(&mut **transaction)
            .await
            .map_err(classify_sqlx_error_owned)?;
        for member in members {
            let selector_id = Uuid::now_v7();
            let column = selector_column(member.selector.object_type).ok_or_else(|| {
                AppError::Validation(
                    "Address-group members must use a typed endpoint reference".into(),
                )
            })?;
            // `column` comes from the closed endpoint registry, never a request.
            let mut query = QueryBuilder::<Postgres>::new(format!(
                "INSERT INTO inventory_dataflow_selector \
                 (id, dataflow_id, group_id, side, position, object_type, address_mode, \
                 tag_object_type, {column}) VALUES ("
            ));
            query
                .push_bind(selector_id)
                .push(", NULL, ")
                .push_bind(group_id);
            query.push(", 'member', ").push_bind(member.position);
            query
                .push(", ")
                .push_bind(member.selector.object_type.as_str());
            query
                .push(", ")
                .push_bind(member.selector.address_mode.as_str());
            query
                .push(", ")
                .push_bind(member.selector.tag_object_type.map(EndpointType::as_str));
            query
                .push(", ")
                .push_bind(member.selector.object_id)
                .push(")");
            query
                .build()
                .execute(&mut **transaction)
                .await
                .map_err(classify_sqlx_error_owned)?;
            for ip in &member.selector.ipaddresses {
                sqlx::query(
                    "INSERT INTO inventory_dataflow_selector_ip (selector_id, ipaddress_id) \
                     VALUES ($1, $2)",
                )
                .bind(selector_id)
                .bind(*ip)
                .execute(&mut **transaction)
                .await
                .map_err(classify_sqlx_error_owned)?;
            }
        }
        Ok(())
    }

    // ---- application roles -------------------------------------------------

    pub async fn list_application_roles(
        &self,
        filter: &ApplicationRoleFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ApplicationRole>, i64), AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT r.id, r.name, r.description, r.comments, r.owner_id \
             FROM inventory_application_role r",
        );
        Self::push_role_filters(&mut builder, filter);
        builder.push(" ORDER BY r.name, r.id LIMIT ");
        builder.push_bind(i64::from(clamp_limit(limit)));
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(clamp_offset(offset)));
        let rows = builder
            .build_query_as::<ApplicationRoleRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items: Vec<ApplicationRole> = rows.into_iter().map(ApplicationRole::from).collect();
        let total = self.count_application_roles(filter).await?;
        Ok((items, total))
    }

    async fn count_application_roles(
        &self,
        filter: &ApplicationRoleFilter,
    ) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM inventory_application_role r");
        Self::push_role_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    pub async fn get_application_role(
        &self,
        id: Uuid,
    ) -> Result<Option<ApplicationRole>, AppError> {
        let row = sqlx::query_as::<_, ApplicationRoleRow>(
            "SELECT r.id, r.name, r.description, r.comments, r.owner_id \
             FROM inventory_application_role r WHERE r.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        Ok(row.map(ApplicationRole::from))
    }

    pub async fn create_application_role(
        &self,
        role: &ApplicationRole,
    ) -> Result<ApplicationRole, AppError> {
        sqlx::query(
            "INSERT INTO inventory_application_role (id, name, description, comments, owner_id) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(role.id)
        .bind(role.name.as_str())
        .bind(role.description.as_deref().unwrap_or_default())
        .bind(role.comments.as_deref().unwrap_or_default())
        .bind(role.owner_id)
        .execute(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        self.get_application_role(role.id)
            .await?
            .ok_or_else(|| AppError::Database("Application role disappeared after insert".into()))
    }

    pub async fn update_application_role(
        &self,
        id: Uuid,
        changes: &ApplicationRoleChanges,
    ) -> Result<Option<ApplicationRole>, AppError> {
        let Some(existing) = self.get_application_role(id).await? else {
            return Ok(None);
        };
        let mut role = existing;
        changes.clone().apply(&mut role);
        role.normalize()?;
        sqlx::query(
            "UPDATE inventory_application_role \
             SET name = $2, description = $3, comments = $4, owner_id = $5 WHERE id = $1",
        )
        .bind(id)
        .bind(role.name.as_str())
        .bind(role.description.as_deref().unwrap_or_default())
        .bind(role.comments.as_deref().unwrap_or_default())
        .bind(role.owner_id)
        .execute(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        self.get_application_role(id).await
    }

    pub async fn delete_application_role(&self, id: Uuid) -> Result<(), AppError> {
        delete_existing_record(&self.pool, APPLICATION_ROLE_TABLE, id, "Application role").await
    }

    // ---- applications ------------------------------------------------------

    pub async fn list_applications(
        &self,
        filter: &ApplicationFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<Application>, i64), AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT a.id, a.name, a.role_id, a.description, a.comments, a.owner_id, a.tenant_id, \
             COALESCE(a.tags, '{}'::text[]) AS tags, \
             COALESCE(a.custom_field_data, '{}'::jsonb) AS custom_field_data \
             FROM inventory_application a",
        );
        Self::push_application_filters(&mut builder, filter);
        builder.push(" ORDER BY a.name, a.id LIMIT ");
        builder.push_bind(i64::from(clamp_limit(limit)));
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(clamp_offset(offset)));
        let rows = builder
            .build_query_as::<ApplicationRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let items: Vec<Application> = rows.into_iter().map(Application::from).collect();
        let total = self.count_applications(filter).await?;
        Ok((items, total))
    }

    async fn count_applications(&self, filter: &ApplicationFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM inventory_application a");
        Self::push_application_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    pub async fn get_application(&self, id: Uuid) -> Result<Option<Application>, AppError> {
        let row = sqlx::query_as::<_, ApplicationRow>(
            "SELECT a.id, a.name, a.role_id, a.description, a.comments, a.owner_id, a.tenant_id, \
             COALESCE(a.tags, '{}'::text[]) AS tags, \
             COALESCE(a.custom_field_data, '{}'::jsonb) AS custom_field_data \
             FROM inventory_application a WHERE a.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
        Ok(row.map(Application::from))
    }

    pub async fn create_application(
        &self,
        application: &Application,
    ) -> Result<Application, AppError> {
        let mut transaction = begin_native_write(&self.pool).await?;
        lock_tag_catalog_for_usage(&mut transaction).await?;
        sqlx::query(
            "INSERT INTO inventory_application \
             (id, name, role_id, description, comments, owner_id, tenant_id, tags, \
             custom_field_data) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(application.id)
        .bind(application.name.as_str())
        .bind(application.role_id)
        .bind(application.description.as_deref().unwrap_or_default())
        .bind(application.comments.as_deref().unwrap_or_default())
        .bind(application.owner_id)
        .bind(application.tenant_id)
        .bind(&application.tags)
        .bind(&application.custom_field_data)
        .execute(&mut *transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
        transaction
            .commit()
            .await
            .map_err(classify_sqlx_error_owned)?;
        self.get_application(application.id)
            .await?
            .ok_or_else(|| AppError::Database("Application disappeared after insert".into()))
    }

    pub async fn update_application(
        &self,
        id: Uuid,
        changes: &ApplicationChanges,
    ) -> Result<Option<Application>, AppError> {
        let Some(existing) = self.get_application(id).await? else {
            return Ok(None);
        };
        let mut application = existing;
        changes.clone().apply(&mut application);
        application.normalize()?;
        let mut transaction = begin_native_write(&self.pool).await?;
        lock_tag_catalog_for_usage(&mut transaction).await?;
        let updated = sqlx::query(
            "UPDATE inventory_application SET name = $2, role_id = $3, description = $4, \
             comments = $5, owner_id = $6, tenant_id = $7, tags = $8, custom_field_data = $9 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(application.name.as_str())
        .bind(application.role_id)
        .bind(application.description.as_deref().unwrap_or_default())
        .bind(application.comments.as_deref().unwrap_or_default())
        .bind(application.owner_id)
        .bind(application.tenant_id)
        .bind(&application.tags)
        .bind(&application.custom_field_data)
        .execute(&mut *transaction)
        .await
        .map_err(classify_sqlx_error_owned)?
        .rows_affected();
        if updated == 0 {
            transaction
                .rollback()
                .await
                .map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        }
        transaction
            .commit()
            .await
            .map_err(classify_sqlx_error_owned)?;
        self.get_application(id).await
    }

    pub async fn delete_application(&self, id: Uuid) -> Result<(), AppError> {
        delete_existing_record(&self.pool, APPLICATION_TABLE, id, "Application").await
    }

    // ---- flow groups -------------------------------------------------------

    pub async fn list_dataflow_groups(
        &self,
        filter: &DataflowGroupFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<DataflowGroup>, i64), AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(format!(
            "SELECT {GROUP_SELECT} FROM inventory_dataflow_group g"
        ));
        Self::push_group_filters(&mut builder, filter);
        builder.push(" ORDER BY g.name, g.id LIMIT ");
        builder.push_bind(i64::from(clamp_limit(limit)));
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(clamp_offset(offset)));
        let rows = builder
            .build_query_as::<GroupRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let groups = rows
            .into_iter()
            .map(DataflowGroup::try_from)
            .collect::<Result<Vec<DataflowGroup>, AppError>>()?;
        let total = self.count_dataflow_groups(filter).await?;
        Ok((self.hydrate_groups(groups).await?, total))
    }

    async fn count_dataflow_groups(&self, filter: &DataflowGroupFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM inventory_dataflow_group g");
        Self::push_group_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    pub async fn get_dataflow_group(&self, id: Uuid) -> Result<Option<DataflowGroup>, AppError> {
        let mut query = QueryBuilder::<Postgres>::new(format!(
            "SELECT {GROUP_SELECT} FROM inventory_dataflow_group g WHERE g.id = "
        ));
        query.push_bind(id);
        let row = query
            .build_query_as::<GroupRow>()
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut groups = self
            .hydrate_groups(vec![DataflowGroup::try_from(row)?])
            .await?;
        Ok(groups.pop())
    }

    /// Inclusive descendant set of a group, including the group itself.
    pub async fn descendant_group_ids(&self, group_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        sqlx::query_scalar::<_, Uuid>(
            "WITH RECURSIVE descendants AS ( \
                 SELECT g.id FROM inventory_dataflow_group g WHERE g.id = $1 \
                 UNION \
                 SELECT g.id FROM inventory_dataflow_group g \
                 JOIN descendants d ON g.parent_id = d.id \
             ) SELECT id FROM descendants ORDER BY id",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)
    }

    pub async fn create_dataflow_group(
        &self,
        group: &DataflowGroup,
    ) -> Result<DataflowGroup, AppError> {
        let mut transaction = begin_native_write(&self.pool).await?;
        lock_group_hierarchy(&mut transaction).await?;
        lock_tag_catalog_for_usage(&mut transaction).await?;
        sqlx::query(
            "INSERT INTO inventory_dataflow_group \
             (id, parent_id, application_id, name, description, comments, status, owner_id, \
             tenant_id, tags, custom_field_data) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(group.id)
        .bind(group.parent_id)
        .bind(group.application_id)
        .bind(group.name.as_str())
        .bind(group.description.as_deref().unwrap_or_default())
        .bind(group.comments.as_deref().unwrap_or_default())
        .bind(group.status.as_str())
        .bind(group.owner_id)
        .bind(group.tenant_id)
        .bind(&group.tags)
        .bind(&group.custom_field_data)
        .execute(&mut *transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
        transaction
            .commit()
            .await
            .map_err(classify_sqlx_error_owned)?;
        self.get_dataflow_group(group.id)
            .await?
            .ok_or_else(|| AppError::Database("Flow group disappeared after insert".into()))
    }

    pub async fn update_dataflow_group(
        &self,
        id: Uuid,
        changes: &DataflowGroupChanges,
    ) -> Result<Option<DataflowGroup>, AppError> {
        let mut transaction = begin_native_write(&self.pool).await?;
        lock_group_hierarchy(&mut transaction).await?;
        lock_tag_catalog_for_usage(&mut transaction).await?;
        let mut query = QueryBuilder::<Postgres>::new(format!(
            "SELECT {GROUP_SELECT} FROM inventory_dataflow_group g WHERE g.id = "
        ));
        query.push_bind(id).push(" FOR UPDATE");
        let row = query
            .build_query_as::<GroupRow>()
            .fetch_optional(&mut *transaction)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            transaction
                .rollback()
                .await
                .map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        };
        let mut group = DataflowGroup::try_from(row)?;
        changes.clone().apply(&mut group);
        group.normalize()?;
        group.reset_effective_projection();
        sqlx::query(
            "UPDATE inventory_dataflow_group SET parent_id = $2, application_id = $3, name = $4, \
             description = $5, comments = $6, status = $7, owner_id = $8, tenant_id = $9, \
             tags = $10, custom_field_data = $11 WHERE id = $1",
        )
        .bind(id)
        .bind(group.parent_id)
        .bind(group.application_id)
        .bind(group.name.as_str())
        .bind(group.description.as_deref().unwrap_or_default())
        .bind(group.comments.as_deref().unwrap_or_default())
        .bind(group.status.as_str())
        .bind(group.owner_id)
        .bind(group.tenant_id)
        .bind(&group.tags)
        .bind(&group.custom_field_data)
        .execute(&mut *transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
        transaction
            .commit()
            .await
            .map_err(classify_sqlx_error_owned)?;
        self.get_dataflow_group(id).await
    }

    pub async fn delete_dataflow_group(&self, id: Uuid) -> Result<(), AppError> {
        let mut transaction = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        lock_group_hierarchy(&mut transaction).await?;
        let deleted =
            delete_inventory_record_in_transaction(&mut transaction, DATAFLOW_GROUP_TABLE, id)
                .await?;
        if deleted == 0 {
            transaction
                .rollback()
                .await
                .map_err(classify_sqlx_error_owned)?;
            return Err(AppError::NotFound("Flow group not found".into()));
        }
        transaction
            .commit()
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok(())
    }

    // ---- address groups ----------------------------------------------------

    pub async fn list_address_groups(
        &self,
        filter: &AddressGroupFilter,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<AddressGroup>, i64), AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(format!(
            "SELECT {ADDRESS_GROUP_SELECT} FROM inventory_address_group ag"
        ));
        Self::push_address_group_filters(&mut builder, filter);
        builder.push(" ORDER BY ag.name, ag.id LIMIT ");
        builder.push_bind(i64::from(clamp_limit(limit)));
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(clamp_offset(offset)));
        let rows = builder
            .build_query_as::<AddressGroupRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut groups: Vec<AddressGroup> = rows.into_iter().map(AddressGroup::from).collect();
        let total = self.count_address_groups(filter).await?;
        let ids: Vec<Uuid> = groups.iter().map(|group| group.id).collect();
        let mut members = self.load_members(&ids).await?;
        for group in groups.iter_mut() {
            group.members = members.remove(&group.id).unwrap_or_default();
        }
        Ok((groups, total))
    }

    async fn count_address_groups(&self, filter: &AddressGroupFilter) -> Result<i64, AppError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM inventory_address_group ag");
        Self::push_address_group_filters(&mut builder, filter);
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)
    }

    pub async fn get_address_group(&self, id: Uuid) -> Result<Option<AddressGroup>, AppError> {
        let mut query = QueryBuilder::<Postgres>::new(format!(
            "SELECT {ADDRESS_GROUP_SELECT} FROM inventory_address_group ag WHERE ag.id = "
        ));
        query.push_bind(id);
        let row = query
            .build_query_as::<AddressGroupRow>()
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut group = AddressGroup::from(row);
        let mut members = self.load_members(&[group.id]).await?;
        group.members = members.remove(&group.id).unwrap_or_default();
        Ok(Some(group))
    }

    pub async fn create_address_group(
        &self,
        group: &AddressGroup,
    ) -> Result<AddressGroup, AppError> {
        let mut transaction = begin_native_write(&self.pool).await?;
        lock_tag_catalog_for_usage(&mut transaction).await?;
        Self::validate_members(&mut transaction, &group.members).await?;
        sqlx::query(
            "INSERT INTO inventory_address_group \
             (id, name, description, comments, owner_id, tenant_id, tags, custom_field_data) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(group.id)
        .bind(group.name.as_str())
        .bind(group.description.as_deref().unwrap_or_default())
        .bind(group.comments.as_deref().unwrap_or_default())
        .bind(group.owner_id)
        .bind(group.tenant_id)
        .bind(&group.tags)
        .bind(&group.custom_field_data)
        .execute(&mut *transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
        Self::replace_members(&mut transaction, group.id, &group.members).await?;
        transaction
            .commit()
            .await
            .map_err(classify_sqlx_error_owned)?;
        self.get_address_group(group.id)
            .await?
            .ok_or_else(|| AppError::Database("Address group disappeared after insert".into()))
    }

    pub async fn update_address_group(
        &self,
        id: Uuid,
        changes: &AddressGroupChanges,
    ) -> Result<Option<AddressGroup>, AppError> {
        let mut transaction = begin_native_write(&self.pool).await?;
        lock_tag_catalog_for_usage(&mut transaction).await?;
        let mut query = QueryBuilder::<Postgres>::new(format!(
            "SELECT {ADDRESS_GROUP_SELECT} FROM inventory_address_group ag WHERE ag.id = "
        ));
        query.push_bind(id).push(" FOR UPDATE");
        let row = query
            .build_query_as::<AddressGroupRow>()
            .fetch_optional(&mut *transaction)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            transaction
                .rollback()
                .await
                .map_err(classify_sqlx_error_owned)?;
            return Ok(None);
        };
        let replace_members = changes.members.is_some();
        let mut group = AddressGroup::from(row);
        changes.clone().apply(&mut group);
        if replace_members {
            group.normalize()?;
            Self::validate_members(&mut transaction, &group.members).await?;
        } else {
            group.normalize_metadata()?;
        }
        sqlx::query(
            "UPDATE inventory_address_group SET name = $2, description = $3, comments = $4, \
             owner_id = $5, tenant_id = $6, tags = $7, custom_field_data = $8 WHERE id = $1",
        )
        .bind(id)
        .bind(group.name.as_str())
        .bind(group.description.as_deref().unwrap_or_default())
        .bind(group.comments.as_deref().unwrap_or_default())
        .bind(group.owner_id)
        .bind(group.tenant_id)
        .bind(&group.tags)
        .bind(&group.custom_field_data)
        .execute(&mut *transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if replace_members {
            Self::replace_members(&mut transaction, id, &group.members).await?;
        }
        transaction
            .commit()
            .await
            .map_err(classify_sqlx_error_owned)?;
        self.get_address_group(id).await
    }

    pub async fn delete_address_group(&self, id: Uuid) -> Result<(), AppError> {
        let mut transaction = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        // Members belong to the group definition; flows referencing the group
        // through `addressgroup_id` still block the delete as a RESTRICT row.
        sqlx::query(
            "DELETE FROM inventory_dataflow_selector_ip WHERE selector_id IN \
             (SELECT id FROM inventory_dataflow_selector WHERE group_id = $1)",
        )
        .bind(id)
        .execute(&mut *transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
        sqlx::query("DELETE FROM inventory_dataflow_selector WHERE group_id = $1")
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let deleted =
            delete_inventory_record_in_transaction(&mut transaction, ADDRESS_GROUP_TABLE, id)
                .await?;
        if deleted == 0 {
            transaction
                .rollback()
                .await
                .map_err(classify_sqlx_error_owned)?;
            return Err(AppError::NotFound("Address group not found".into()));
        }
        transaction
            .commit()
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok(())
    }
}

/// Serialize all hierarchy mutations on one transaction-scoped advisory lock so
/// two concurrent moves cannot interleave their guard checks.
async fn lock_group_hierarchy(transaction: &mut Transaction<'_, Postgres>) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('inventory-dataflow-group', 0))")
        .execute(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

async fn delete_existing_record(
    pool: &PgPool,
    table: &'static str,
    id: Uuid,
    label: &str,
) -> Result<(), AppError> {
    match delete_inventory_record(pool, table, id).await? {
        0 => Err(AppError::NotFound(format!("{label} not found"))),
        _ => Ok(()),
    }
}

fn member_from_row(row: MemberRow) -> Result<AddressGroupMember, AppError> {
    let mut selector = DataflowEndpointInput {
        object_type: EndpointType::parse(&row.object_type)?,
        object_id: row.object_id,
        address_mode: parse_address_mode(&row.address_mode)?,
        ipaddresses: row.ipaddresses,
        tag_object_type: match row.tag_object_type.as_deref() {
            Some(raw) => Some(EndpointType::parse(raw)?),
            None => None,
        },
    };
    selector.normalize()?;
    Ok(AddressGroupMember {
        selector,
        position: row.position,
        label: None,
    })
}
