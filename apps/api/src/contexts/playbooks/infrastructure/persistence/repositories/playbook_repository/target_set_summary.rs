//! List-facing target-set facts: resolved host counts and usage.
//!
//! A target-set list must answer "how many hosts" and "what uses it" without a
//! preview request per row. Counts run as one grouped statement for static sets
//! and one `UNION ALL` statement for dynamic sets, both reusing the exact
//! preview predicate builder so a list count always equals the preview count.
//! Usage is three grouped selects over the identity references that exist:
//! managed job-template inventory, assignments and schedule target selections.
//!
//! Counts are cached for a short period keyed by `(id, updated_at)`: any write
//! to the set changes `updated_at`, so a stale entry can never be served for a
//! modified set. A count failure or timeout yields `None` ("unknown"), never 0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use super::selector::push_target_selector_predicates;
use super::sql_strings::TARGET_PREVIEW_BASE_SQL;
use crate::contexts::playbooks::domain::{
    PlaybookTargetSet, PlaybookTargetSetMode, PlaybookTargetSetUsage, PlaybookTargetSetUsageRef,
};
use crate::errors::{AppError, classify_sqlx_error_owned};

/// How long a resolved count may be reused for an unchanged set.
const COUNT_CACHE_TTL: Duration = Duration::from_secs(30);
/// Upper bound for one list's count work before counts become "unknown".
const COUNT_QUERY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy)]
struct CachedCount {
    count: i64,
    expires: Instant,
}

/// Short-lived per-set count cache shared by every request of this repository.
#[derive(Debug, Clone, Default)]
pub(super) struct TargetSetCountCache {
    entries: Arc<Mutex<HashMap<(Uuid, DateTime<Utc>), CachedCount>>>,
}

impl TargetSetCountCache {
    fn get(&self, target_set: &PlaybookTargetSet) -> Option<i64> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (target_set.id, target_set.updated_at);
        let entry = *entries.get(&key)?;
        if entry.expires <= Instant::now() {
            entries.remove(&key);
            return None;
        }
        Some(entry.count)
    }

    fn insert(&self, target_set: &PlaybookTargetSet, count: i64) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        entries.retain(|_, entry| entry.expires > now);
        entries.insert(
            (target_set.id, target_set.updated_at),
            CachedCount {
                count,
                expires: now + COUNT_CACHE_TTL,
            },
        );
    }

    /// Drop every cached count of one set after a write.
    pub(super) fn invalidate(&self, id: Uuid) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|(cached_id, _), _| *cached_id != id);
    }
}

/// Resolved host counts for one page. A set missing from the result map is
/// unknown, not zero.
pub(super) async fn resolved_counts(
    pool: &PgPool,
    cache: &TargetSetCountCache,
    target_sets: &[PlaybookTargetSet],
) -> HashMap<Uuid, Option<i64>> {
    let mut counts = HashMap::with_capacity(target_sets.len());
    let mut missing = Vec::new();
    for target_set in target_sets {
        match cache.get(target_set) {
            Some(count) => {
                counts.insert(target_set.id, Some(count));
            }
            None => missing.push(target_set.clone()),
        }
    }
    if missing.is_empty() {
        return counts;
    }

    // Static and dynamic counts are independent statements. An uncomputable
    // selector must leave its own sets unknown without discarding counts that
    // another set already answered.
    let (static_sets, dynamic_sets): (Vec<_>, Vec<_>) = missing
        .into_iter()
        .partition(|target_set| target_set.mode == PlaybookTargetSetMode::Static);
    let mut computed: HashMap<Uuid, i64> =
        HashMap::with_capacity(static_sets.len() + dynamic_sets.len());
    for (mode, outcome) in [
        (
            "static",
            tokio::time::timeout(COUNT_QUERY_TIMEOUT, static_counts(pool, &static_sets)).await,
        ),
        (
            "dynamic",
            tokio::time::timeout(COUNT_QUERY_TIMEOUT, dynamic_counts(pool, &dynamic_sets)).await,
        ),
    ] {
        match outcome {
            Ok(Ok(values)) => computed.extend(values),
            Ok(Err(error)) => tracing::warn!(
                error = %error,
                mode,
                "target set counts failed; reporting unknown counts"
            ),
            Err(_) => tracing::warn!(
                mode,
                "target set counts timed out; reporting unknown counts"
            ),
        }
    }

    for target_set in static_sets.iter().chain(dynamic_sets.iter()) {
        match computed.get(&target_set.id) {
            Some(count) => {
                cache.insert(target_set, *count);
                counts.insert(target_set.id, Some(*count));
            }
            None => {
                counts.insert(target_set.id, None);
            }
        }
    }
    counts
}

async fn static_counts(
    pool: &PgPool,
    target_sets: &[PlaybookTargetSet],
) -> Result<HashMap<Uuid, i64>, AppError> {
    let mut counts = HashMap::with_capacity(target_sets.len());

    let static_ids: Vec<Uuid> = target_sets.iter().map(|target_set| target_set.id).collect();
    if !static_ids.is_empty() {
        let mut query = QueryBuilder::<Postgres>::new(
            "WITH selected_members AS (\
                 SELECT target_set_id, target_kind, target_id, source_kind, source_id \
                 FROM playbook_target_set_member WHERE target_set_id = ANY(",
        );
        query.push_bind(static_ids.clone());
        query.push(")), targets AS (");
        query.push(TARGET_PREVIEW_BASE_SQL);
        query.push(
            ") SELECT selected_members.target_set_id AS id, COUNT(*) AS total \
             FROM selected_members \
             JOIN targets \
               ON targets.target_kind = selected_members.target_kind \
              AND targets.target_id = selected_members.target_id \
              AND targets.source_kind = selected_members.source_kind \
              AND targets.source_id = selected_members.source_id \
             GROUP BY selected_members.target_set_id",
        );
        let rows = query
            .build()
            .fetch_all(pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
            let total: i64 = row.try_get("total").map_err(classify_sqlx_error_owned)?;
            counts.insert(id, total);
        }
        // A static set whose members no longer resolve has a known count of 0.
        for id in static_ids {
            counts.entry(id).or_insert(0);
        }
    }

    Ok(counts)
}

async fn dynamic_counts(
    pool: &PgPool,
    dynamic_sets: &[PlaybookTargetSet],
) -> Result<HashMap<Uuid, i64>, AppError> {
    let mut counts = HashMap::with_capacity(dynamic_sets.len());
    if !dynamic_sets.is_empty() {
        let mut query = QueryBuilder::<Postgres>::new("SELECT id, COUNT(*) AS total FROM (");
        for (position, target_set) in dynamic_sets.iter().enumerate() {
            if position > 0 {
                query.push(" UNION ALL ");
            }
            query.push("SELECT ");
            query.push_bind(target_set.id);
            query.push(" AS id, selected_targets.* FROM (");
            query.push(TARGET_PREVIEW_BASE_SQL);
            push_target_selector_predicates(&mut query, &target_set.selector)?;
            query.push(") AS selected_targets");
        }
        query.push(") AS counted GROUP BY id");
        let rows = query
            .build()
            .fetch_all(pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        for row in rows {
            let id: Uuid = row.try_get("id").map_err(classify_sqlx_error_owned)?;
            let total: i64 = row.try_get("total").map_err(classify_sqlx_error_owned)?;
            counts.insert(id, total);
        }
    }

    Ok(counts)
}

/// Usage buckets for one page, keyed by target-set id.
pub(super) async fn usage_by_target_set(
    pool: &PgPool,
    target_set_ids: &[Uuid],
) -> Result<HashMap<Uuid, PlaybookTargetSetUsage>, AppError> {
    let mut connection = pool.acquire().await.map_err(classify_sqlx_error_owned)?;
    usage_by_target_set_on(&mut connection, target_set_ids).await
}

/// Usage read inside the caller's transaction. The delete path calls this only
/// after taking its `FOR UPDATE` lock, so the snapshot includes every reference
/// whose writer released its `FOR SHARE` lock.
pub(super) async fn usage_by_target_set_tx(
    tx: &mut Transaction<'_, Postgres>,
    target_set_ids: &[Uuid],
) -> Result<HashMap<Uuid, PlaybookTargetSetUsage>, AppError> {
    usage_by_target_set_on(&mut **tx, target_set_ids).await
}

async fn usage_by_target_set_on(
    connection: &mut PgConnection,
    target_set_ids: &[Uuid],
) -> Result<HashMap<Uuid, PlaybookTargetSetUsage>, AppError> {
    let mut usage: HashMap<Uuid, PlaybookTargetSetUsage> = target_set_ids
        .iter()
        .map(|id| (*id, PlaybookTargetSetUsage::default()))
        .collect();
    if target_set_ids.is_empty() {
        return Ok(usage);
    }
    let id_text: Vec<String> = target_set_ids.iter().map(Uuid::to_string).collect();

    let rows = sqlx::query(
        "SELECT id, name, inventory->>'target_set_id' AS target_set_id \
         FROM playbook_job_template \
         WHERE inventory->>'kind' = 'managed' \
           AND inventory->>'target_set_id' = ANY($1) \
         ORDER BY name, id",
    )
    .bind(&id_text)
    .fetch_all(&mut *connection)
    .await
    .map_err(classify_sqlx_error_owned)?;
    for row in rows {
        let Some(id) = parse_reference(&row)? else {
            continue;
        };
        push_usage(
            &mut usage,
            id,
            PlaybookTargetSetUsageRef {
                id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
                name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
            },
            |usage| &mut usage.job_templates,
        );
    }

    let rows = sqlx::query(
        "SELECT id, name, target_set_id FROM playbook_assignment \
         WHERE target_set_id = ANY($1) ORDER BY name NULLS LAST, id",
    )
    .bind(target_set_ids)
    .fetch_all(&mut *connection)
    .await
    .map_err(classify_sqlx_error_owned)?;
    for row in rows {
        let id: Uuid = row
            .try_get("target_set_id")
            .map_err(classify_sqlx_error_owned)?;
        push_usage(
            &mut usage,
            id,
            PlaybookTargetSetUsageRef {
                id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
                name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
            },
            |usage| &mut usage.assignments,
        );
    }

    let rows = sqlx::query(
        "SELECT id, name, targets->>'target_set_id' AS target_set_id \
         FROM playbook_schedule \
         WHERE targets->>'target_set_id' = ANY($1) \
         ORDER BY name, id",
    )
    .bind(&id_text)
    .fetch_all(&mut *connection)
    .await
    .map_err(classify_sqlx_error_owned)?;
    for row in rows {
        let Some(id) = parse_reference(&row)? else {
            continue;
        };
        push_usage(
            &mut usage,
            id,
            PlaybookTargetSetUsageRef {
                id: row.try_get("id").map_err(classify_sqlx_error_owned)?,
                name: row.try_get("name").map_err(classify_sqlx_error_owned)?,
            },
            |usage| &mut usage.schedules,
        );
    }

    Ok(usage)
}

fn parse_reference(row: &sqlx::postgres::PgRow) -> Result<Option<Uuid>, AppError> {
    let Some(reference): Option<String> = row
        .try_get("target_set_id")
        .map_err(classify_sqlx_error_owned)?
    else {
        return Ok(None);
    };
    Uuid::parse_str(&reference)
        .map(Some)
        .map_err(|error| AppError::Database(format!("Invalid target set reference: {error}")))
}

fn push_usage(
    usage: &mut HashMap<Uuid, PlaybookTargetSetUsage>,
    target_set_id: Uuid,
    reference: PlaybookTargetSetUsageRef,
    bucket: impl FnOnce(&mut PlaybookTargetSetUsage) -> &mut Vec<PlaybookTargetSetUsageRef>,
) {
    if let Some(entry) = usage.get_mut(&target_set_id) {
        bucket(entry).push(reference);
    }
}

/// Usernames for the list's owner columns.
pub(super) async fn usernames_by_id(
    pool: &PgPool,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, String>, AppError> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query("SELECT id, username FROM identity_user WHERE id = ANY($1)")
        .bind(user_ids)
        .fetch_all(pool)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let mut usernames = HashMap::with_capacity(rows.len());
    for row in rows {
        usernames.insert(
            row.try_get("id").map_err(classify_sqlx_error_owned)?,
            row.try_get("username").map_err(classify_sqlx_error_owned)?,
        );
    }
    Ok(usernames)
}
