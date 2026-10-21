use super::row_mappers::read;
use super::sql_strings::TARGET_PREVIEW_BASE_SQL;
use crate::contexts::playbooks::domain::{PlaybookRun, PlaybookTargetSetPreview};
use crate::errors::{AppError, classify_sqlx_error_owned};
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder};
use std::collections::HashSet;
use uuid::Uuid;

pub(super) fn selector_uuid_list(selector: &Value, key: &str) -> Result<Vec<Uuid>, AppError> {
    let Some(value) = selector.get(key) else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(AppError::Validation(format!(
            "Target selector {key} must be an array of UUIDs"
        )));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .ok_or_else(|| {
                    AppError::Validation(format!("Target selector {key} must use UUID strings"))
                })
                .and_then(|raw| Uuid::parse_str(raw).map_err(AppError::ValidationUuid))
        })
        .collect()
}

pub(super) fn selector_string_list(selector: &Value, key: &str) -> Result<Vec<String>, AppError> {
    let Some(value) = selector.get(key) else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(AppError::Validation(format!(
            "Target selector {key} must be an array of strings"
        )));
    };
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let value = item
            .as_str()
            .ok_or_else(|| AppError::Validation(format!("Target selector {key} must use strings")))?
            .trim()
            .to_ascii_lowercase();
        if !value.is_empty() {
            result.push(value);
        }
    }
    Ok(result)
}

/// Reads a name-keyed facet (`region`/`site`/`tenant`/`role`/`platform`/`cluster`)
/// whose values are matched VERBATIM against the resolved `*_name` columns.
///
/// Unlike [`selector_string_list`], these values must NOT be case-folded: the
/// resolver compares them against the raw `dcim_*.name` display columns, and the
/// frontend writes the exact inventory display name read from those same columns.
/// Lower-casing here mismatched any name containing uppercase letters (e.g.
/// `"Germany"`), resolving the whole dynamic set to zero hosts. Non-ASCII names
/// (e.g. Cyrillic) survived only because `to_ascii_lowercase` left them untouched,
/// which is why the bug surfaced on ASCII-named regions/sites. We trim and drop
/// empty entries but otherwise preserve the value exactly.
pub(super) fn selector_name_list(selector: &Value, key: &str) -> Result<Vec<String>, AppError> {
    let Some(value) = selector.get(key) else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(AppError::Validation(format!(
            "Target selector {key} must be an array of strings"
        )));
    };
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let value = item
            .as_str()
            .ok_or_else(|| AppError::Validation(format!("Target selector {key} must use strings")))?
            .trim();
        if !value.is_empty() {
            result.push(value.to_string());
        }
    }
    Ok(result)
}

pub(super) fn selector_bool(selector: &Value, key: &str) -> Result<Option<bool>, AppError> {
    let Some(value) = selector.get(key) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| AppError::Validation(format!("Target selector {key} must be a boolean")))
}

pub(super) fn selector_connection_target_kinds(selector: &Value) -> Result<Vec<String>, AppError> {
    let target_kinds = selector_string_list(selector, "target_kind")?;
    if target_kinds
        .iter()
        .any(|target_kind| target_kind != "connection_target")
    {
        return Err(AppError::Validation(
            "Playbook target_kind selector supports connection_target only".to_string(),
        ));
    }
    Ok(target_kinds)
}

pub(super) fn selector_source_kinds(selector: &Value) -> Result<Vec<String>, AppError> {
    let source_kinds = selector_string_list(selector, "source_kind")?;
    if source_kinds
        .iter()
        .any(|kind| kind != "device" && kind != "virtual_machine")
    {
        return Err(AppError::Validation(
            "Target selector source_kind supports device and virtual_machine".to_string(),
        ));
    }
    Ok(source_kinds)
}

pub(super) fn selector_runtime_capabilities(selector: &Value) -> Result<Vec<String>, AppError> {
    let runtimes = selector_string_list(selector, "runtime")?;
    if runtimes.iter().any(|runtime| runtime != "ansible") {
        return Err(AppError::Validation(
            "Target selector runtime supports ansible only".to_string(),
        ));
    }
    Ok(runtimes)
}

pub(super) fn selector_connection_protocols(selector: &Value) -> Result<Vec<String>, AppError> {
    let protocols = selector_string_list(selector, "connection_protocol")?
        .into_iter()
        .map(|protocol| protocol.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if protocols.iter().any(|protocol| protocol != "ssh") {
        return Err(AppError::Validation(
            "Target selector connection_protocol supports ssh only".to_string(),
        ));
    }
    Ok(protocols)
}

pub(super) fn push_connection_protocol_predicate(
    builder: &mut QueryBuilder<'_, Postgres>,
    protocols: Vec<String>,
) {
    builder.push(" AND EXISTS (");
    builder.push("SELECT 1 FROM public.connconf_assignment ca ");
    builder.push("JOIN public.connconf_record cr ON cr.id = ca.record_id ");
    builder.push("WHERE ca.enabled = TRUE AND cr.enabled = TRUE AND cr.protocol = ANY(");
    builder.push_bind(protocols);
    builder.push(") AND (");
    builder.push("(ca.scope_type = 'global' AND ca.scope_id IS NULL)");
    builder.push(" OR (ca.scope_type = 'region' AND ca.scope_id = connection_targets.region_id)");
    builder.push(
        " OR (ca.scope_type = 'site_group' AND ca.scope_id = connection_targets.site_group_id)",
    );
    builder.push(" OR (ca.scope_type = 'site' AND ca.scope_id = connection_targets.site_id)");
    builder
        .push(" OR (ca.scope_type = 'location' AND ca.scope_id = connection_targets.location_id)");
    builder.push(" OR (ca.scope_type = 'rack' AND ca.scope_id = connection_targets.rack_id)");
    builder.push(" OR (ca.scope_type = 'tenant' AND ca.scope_id = connection_targets.tenant_id)");
    builder.push(" OR (ca.scope_type = 'cluster' AND ca.scope_id = connection_targets.cluster_id)");
    builder.push(" OR (ca.scope_type = 'role' AND ca.scope_id = connection_targets.role_id)");
    builder.push(
        " OR (ca.scope_type = 'device_type' AND ca.scope_id = connection_targets.device_type_id)",
    );
    builder
        .push(" OR (ca.scope_type = 'platform' AND ca.scope_id = connection_targets.platform_id)");
    builder.push(" OR (ca.scope_type = 'device' AND connection_targets.source_kind = 'device' AND ca.scope_id = connection_targets.source_id)");
    builder.push(" OR (ca.scope_type = 'vm' AND connection_targets.source_kind = 'virtual_machine' AND ca.scope_id = connection_targets.source_id)");
    builder.push("))");
}

pub(super) async fn target_ids_with_connection_protocol(
    pool: &PgPool,
    target_ids: &[Uuid],
    protocol: &str,
) -> Result<HashSet<Uuid>, AppError> {
    if target_ids.is_empty() {
        return Ok(HashSet::new());
    }

    let mut builder = QueryBuilder::<Postgres>::new("SELECT connection_targets.target_id FROM (");
    builder.push(TARGET_PREVIEW_BASE_SQL);
    builder.push(") AS connection_targets WHERE connection_targets.target_id = ANY(");
    builder.push_bind(target_ids.to_vec());
    builder.push(") AND connection_targets.address IS NOT NULL");
    builder.push(" AND connection_targets.address_bound = TRUE");
    push_connection_protocol_predicate(&mut builder, vec![protocol.to_string()]);

    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
    rows.iter()
        .map(|row| read(row, "target_id"))
        .collect::<Result<HashSet<_>, _>>()
}

pub(super) async fn filter_preview_by_connection_protocol(
    pool: &PgPool,
    mut preview: PlaybookTargetSetPreview,
    protocol: &str,
) -> Result<PlaybookTargetSetPreview, AppError> {
    let target_ids = preview
        .items
        .iter()
        .map(|item| item.target_id)
        .collect::<Vec<_>>();
    let eligible_target_ids =
        target_ids_with_connection_protocol(pool, &target_ids, protocol).await?;
    preview
        .items
        .retain(|item| eligible_target_ids.contains(&item.target_id));
    preview.total = i64::try_from(preview.items.len()).unwrap_or(i64::MAX);
    preview.truncated = false;
    Ok(preview)
}

pub(super) fn run_event_host(run: &PlaybookRun) -> Option<String> {
    run.target_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn selector_text(selector: &Value, key: &str) -> Result<Option<String>, AppError> {
    let Some(value) = selector.get(key) else {
        return Ok(None);
    };
    let Some(value) = value.as_str() else {
        return Err(AppError::Validation(format!(
            "Target selector {key} must be a string"
        )));
    };
    let value = value.trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

pub(super) fn selector_sort(selector: &Value) -> Result<(&'static str, &'static str), AppError> {
    let sort = selector_text(selector, "sort")?
        .unwrap_or_else(|| "name".to_string())
        .to_ascii_lowercase();
    let column = match sort.as_str() {
        "name" => "name",
        "address" => "address",
        "status" => "status",
        "site" => "site_name",
        "region" => "region_name",
        "tenant" => "tenant_name",
        "role" => "role_name",
        "platform" => "platform_name",
        "cluster" => "cluster_name",
        "source_kind" => "source_kind",
        _ => {
            return Err(AppError::Validation(
                "Target selector sort must be one of name, address, status, site, region, tenant, role, platform, cluster, source_kind".to_string(),
            ));
        }
    };
    let direction = selector_text(selector, "sort_direction")?
        .unwrap_or_else(|| "asc".to_string())
        .to_ascii_lowercase();
    let direction = match direction.as_str() {
        "asc" => "ASC",
        "desc" => "DESC",
        _ => {
            return Err(AppError::Validation(
                "Target selector sort_direction must be asc or desc".to_string(),
            ));
        }
    };
    Ok((column, direction))
}

pub(super) fn push_dynamic_target_order(
    builder: &mut QueryBuilder<'_, Postgres>,
    selector: &Value,
) -> Result<(), AppError> {
    let (column, direction) = selector_sort(selector)?;
    builder.push(" ORDER BY connection_targets.");
    builder.push(column);
    builder.push(" ");
    builder.push(direction);
    builder.push(" NULLS LAST, connection_targets.name ASC, connection_targets.target_id ASC");
    Ok(())
}

fn push_uuid_selector_predicates(
    builder: &mut QueryBuilder<'_, Postgres>,
    selector: &Value,
) -> Result<(), AppError> {
    for (key, column) in [
        ("site_ids", "site_id"),
        ("region_ids", "region_id"),
        ("tenant_ids", "tenant_id"),
        ("role_ids", "role_id"),
        ("platform_ids", "platform_id"),
        ("cluster_ids", "cluster_id"),
        ("device_type_ids", "device_type_id"),
        ("include_target_ids", "target_id"),
        ("include_source_ids", "source_id"),
    ] {
        let values = selector_uuid_list(selector, key)?;
        if !values.is_empty() {
            builder.push(" AND connection_targets.");
            builder.push(column);
            builder.push(" = ANY(");
            builder.push_bind(values);
            builder.push(")");
        }
    }
    Ok(())
}

fn push_name_selector_predicates(
    builder: &mut QueryBuilder<'_, Postgres>,
    selector: &Value,
) -> Result<(), AppError> {
    for (key, column) in [
        ("site", "site_name"),
        ("region", "region_name"),
        ("tenant", "tenant_name"),
        ("role", "role_name"),
        ("platform", "platform_name"),
        ("cluster", "cluster_name"),
    ] {
        let names = selector_name_list(selector, key)?;
        if !names.is_empty() {
            builder.push(" AND connection_targets.");
            builder.push(column);
            builder.push(" = ANY(");
            builder.push_bind(names);
            builder.push(")");
        }
    }
    Ok(())
}

fn push_exclusion_predicate(
    builder: &mut QueryBuilder<'_, Postgres>,
    selector: &Value,
    key: &str,
    column: &str,
) -> Result<(), AppError> {
    let values = selector_uuid_list(selector, key)?;
    if !values.is_empty() {
        builder.push(" AND NOT (connection_targets.");
        builder.push(column);
        builder.push(" = ANY(");
        builder.push_bind(values);
        builder.push("))");
    }
    Ok(())
}

fn push_selector_values(builder: &mut QueryBuilder<'_, Postgres>, sql: &str, values: Vec<String>) {
    if values.is_empty() {
        return;
    }
    builder.push(sql);
    builder.push_bind(values);
    builder.push(")");
}

fn push_kind_selector_predicates(
    builder: &mut QueryBuilder<'_, Postgres>,
    selector: &Value,
) -> Result<(), AppError> {
    push_selector_values(
        builder,
        " AND connection_targets.target_kind = ANY(",
        selector_connection_target_kinds(selector)?,
    );
    push_selector_values(
        builder,
        " AND connection_targets.source_kind = ANY(",
        selector_source_kinds(selector)?,
    );
    Ok(())
}

fn push_runtime_selector_predicates(
    builder: &mut QueryBuilder<'_, Postgres>,
    selector: &Value,
) -> Result<(), AppError> {
    let runtime_requires_connection = selector_runtime_capabilities(selector)?
        .iter()
        .any(|runtime| runtime == "ansible");
    let mut protocols = selector_connection_protocols(selector)?;
    if runtime_requires_connection && protocols.is_empty() {
        protocols.push("ssh".to_string());
    }
    if runtime_requires_connection || !protocols.is_empty() {
        builder.push(
            " AND connection_targets.address IS NOT NULL AND connection_targets.address_bound = TRUE",
        );
    }
    if !protocols.is_empty() {
        push_connection_protocol_predicate(builder, protocols);
    }
    Ok(())
}

fn push_optional_selector_predicates(
    builder: &mut QueryBuilder<'_, Postgres>,
    selector: &Value,
) -> Result<(), AppError> {
    push_selector_values(
        builder,
        " AND LOWER(connection_targets.status) = ANY(",
        selector_string_list(selector, "status")?,
    );
    let tags = selector_string_list(selector, "tag_slugs")?;
    if !tags.is_empty() {
        builder.push(" AND connection_targets.tags @> ");
        builder.push_bind(tags);
    }
    if let Some(address_bound) = selector_bool(selector, "address_bound")? {
        builder.push(" AND connection_targets.address_bound = ");
        builder.push_bind(address_bound);
    }
    push_address_presence_predicate(builder, selector_bool(selector, "has_address")?);
    push_text_selector_predicates(builder, selector)?;
    Ok(())
}

fn push_address_presence_predicate(
    builder: &mut QueryBuilder<'_, Postgres>,
    has_address: Option<bool>,
) {
    match has_address {
        Some(true) => builder.push(" AND connection_targets.address IS NOT NULL"),
        Some(false) => builder.push(" AND connection_targets.address IS NULL"),
        None => builder,
    };
}

fn push_text_selector_predicates(
    builder: &mut QueryBuilder<'_, Postgres>,
    selector: &Value,
) -> Result<(), AppError> {
    if let Some(name) = selector_text(selector, "name")? {
        builder.push(" AND connection_targets.name ILIKE ");
        builder.push_bind(format!("%{name}%"));
    }
    if let Some(search) = selector_text(selector, "search")? {
        let pattern = format!("%{search}%");
        builder.push(" AND (connection_targets.name ILIKE ");
        builder.push_bind(pattern.clone());
        builder.push(" OR connection_targets.address ILIKE ");
        builder.push_bind(pattern);
        builder.push(")");
    }
    Ok(())
}

pub(super) fn push_target_selector_predicates(
    builder: &mut QueryBuilder<'_, Postgres>,
    selector: &Value,
) -> Result<(), AppError> {
    builder.push(" WHERE TRUE");
    push_kind_selector_predicates(builder, selector)?;
    push_runtime_selector_predicates(builder, selector)?;
    push_uuid_selector_predicates(builder, selector)?;

    // Name-keyed facets: the structured selector builder filters by display name
    // (site/region/tenant/role/platform/cluster), not UUID, so match the *_name
    // columns. Without this, a builder-produced dynamic set silently dropped every
    // facet except `status` and resolved to nearly all hosts. Mirrors the *_ids
    // loop above; both the inventory builder (names) and API/power callers (ids)
    // now filter. Preview and run resolution share this builder, so they agree.
    //
    // These values are matched VERBATIM via `selector_name_list` (no case-folding):
    // the `*_name` columns are not lower-cased, and the frontend writes the exact
    // display name. Lower-casing the bound values silently broke any ASCII name
    // with uppercase letters (e.g. "Germany"), resolving the set to zero hosts.
    push_name_selector_predicates(builder, selector)?;
    push_exclusion_predicate(builder, selector, "exclude_target_ids", "target_id")?;
    push_exclusion_predicate(builder, selector, "exclude_source_ids", "source_id")?;
    push_optional_selector_predicates(builder, selector)?;
    Ok(())
}

pub(super) fn prune_role_from_selector(selector: &Value, role_id: Uuid, role_name: &str) -> Value {
    let Value::Object(mut object) = selector.clone() else {
        return selector.clone();
    };
    remove_matching_string(&mut object, "role", |value| value == role_name);
    let role_id = role_id.to_string();
    remove_matching_string(&mut object, "role_ids", |value| {
        value.eq_ignore_ascii_case(&role_id)
    });
    Value::Object(object)
}

pub(super) fn remove_matching_string<F>(
    object: &mut serde_json::Map<String, Value>,
    key: &str,
    matches_removed_value: F,
) where
    F: Fn(&str) -> bool,
{
    let Some(value) = object.get(key) else {
        return;
    };
    match value {
        Value::String(value) if matches_removed_value(value) => {
            object.remove(key);
        }
        Value::Array(values) => {
            let filtered = values
                .iter()
                .filter(|item| {
                    item.as_str()
                        .is_none_or(|value| !matches_removed_value(value))
                })
                .cloned()
                .collect::<Vec<_>>();
            if filtered.is_empty() {
                object.remove(key);
            } else if filtered.len() != values.len() {
                object.insert(key.to_string(), Value::Array(filtered));
            }
        }
        _ => {}
    }
}
