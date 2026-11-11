//! Persistence for source-owned exporter discovery assignments.
//!
//! Provenance is the discovery profile identity: one profile per run source
//! (`discovery-<source_kind>-<source_id>`), which owns its items and its
//! `connection_target` assignment. Manual profiles are never touched, so no new
//! parallel SD or side ledger is introduced.
//!
//! The advisory lock, the staleness decision, the absence window and every write
//! for one source share one transaction, so concurrent or replayed ingests
//! cannot regress assignments or interleave manual reads.

use std::collections::BTreeMap;

use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::errors::{AppError, classify_sqlx_error_owned};

use super::super::application::exporter_discovery_reconcile::{
    ConfirmedEndpoint, DiscoveryPolicy, EndpointKey, absent_endpoints, confirmed_endpoints,
    consecutive_absences, endpoint_key, is_stale_report,
};
use super::super::domain::exporter_discovery::{DiscoveryReport, EndpointIdentity};

pub const DISCOVERY_PROFILE_SLUG_PREFIX: &str = "discovery-";
pub const DISCOVERY_RECENT_REPORT_LIMIT: i64 = 60;

#[derive(Debug, Clone, Default)]
pub struct HostScope {
    pub site_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    /// `device` or `virtual_machine`; the host-scope predicates depend on it.
    pub kind: String,
}

pub struct DiscoveryApplyInput<'a> {
    pub host_id: Uuid,
    pub host_name: &'a str,
    pub source_kind: &'a str,
    pub source_id: Uuid,
    pub host_scope: HostScope,
    pub policy: DiscoveryPolicy,
    pub confirmed: &'a [ConfirmedEndpoint],
    pub absent: &'a [EndpointIdentity],
    pub absence_counts: &'a BTreeMap<EndpointKey, u32>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiscoveryApplyOutcome {
    pub created: usize,
    pub updated: usize,
    pub manual_preserved: usize,
    pub disabled: usize,
    pub skipped_unknown_type: usize,
}

/// The discovery profile slug is the canonical source provenance marker.
#[must_use]
pub fn discovery_profile_slug(source_kind: &str, source_id: Uuid) -> String {
    format!(
        "{DISCOVERY_PROFILE_SLUG_PREFIX}{source_kind}-{}",
        source_id.simple()
    )
}

/// Begin the one transaction that owns every read and write for a source.
pub async fn begin_source_tx<'a>(
    pool: &'a PgPool,
    source_id: Uuid,
) -> Result<Transaction<'a, Postgres>, AppError> {
    let mut tx = pool.begin().await.map_err(classify_sqlx_error_owned)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("monitoring-discovery:{source_id}"))
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(tx)
}

/// Newest persisted report finish time, ordered by the report's own instant so a
/// delayed replay cannot regress the baseline.
pub async fn latest_finished_at_tx(
    tx: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
) -> Result<Option<String>, AppError> {
    let value = sqlx::query_scalar::<_, Option<serde_json::Value>>(
        r#"
        SELECT content_json
        FROM playbook_artifact
        WHERE artifact_type = 'exporter-discovery-report'
          AND name = $1
        ORDER BY (content_json->>'finished_at') DESC NULLS LAST, created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(format!("exporter-discovery-report-{host_id}"))
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?
    .flatten();
    Ok(value.and_then(|value| value.get("finished_at")?.as_str().map(str::to_string)))
}

/// Recent report artifacts for one host, newest by their own finish time.
pub async fn load_recent_reports_tx(
    tx: &mut Transaction<'_, Postgres>,
    host_id: Uuid,
    limit: i64,
) -> Result<Vec<DiscoveryReport>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT content_json
        FROM playbook_artifact
        WHERE artifact_type = 'exporter-discovery-report'
          AND name = $1
        ORDER BY (content_json->>'finished_at') DESC NULLS LAST, created_at DESC, id DESC
        LIMIT $2
        "#,
    )
    .bind(format!("exporter-discovery-report-{host_id}"))
    .bind(limit.clamp(1, DISCOVERY_RECENT_REPORT_LIMIT))
    .fetch_all(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            row.try_get::<Option<serde_json::Value>, _>("content_json")
                .ok()
                .flatten()
        })
        .filter_map(|value| serde_json::from_value::<DiscoveryReport>(value).ok())
        .collect())
}

/// Apply confirmed and absent observations inside the caller's locked
/// transaction.
pub async fn apply_discovery_tx(
    tx: &mut Transaction<'_, Postgres>,
    input: &DiscoveryApplyInput<'_>,
) -> Result<DiscoveryApplyOutcome, AppError> {
    let mut outcome = DiscoveryApplyOutcome::default();
    let mut applicable = Vec::<(Uuid, &ConfirmedEndpoint)>::new();
    for endpoint in input.confirmed {
        let Some(exporter_id) = resolve_exporter_id(tx, &endpoint.exporter_type).await? else {
            outcome.skipped_unknown_type += 1;
            continue;
        };
        if manual_coverage_exists(tx, input, exporter_id, &endpoint.identity).await? {
            outcome.manual_preserved += 1;
            continue;
        }
        applicable.push((exporter_id, endpoint));
    }

    let slug = discovery_profile_slug(input.source_kind, input.source_id);
    // A run that only sees manual coverage must not create an empty profile.
    let profile_id = if applicable.is_empty() {
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM monitoring_profile WHERE slug = $1")
            .bind(&slug)
            .fetch_optional(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?
    } else {
        let profile_name = format!("Discovered exporters {}", input.host_name);
        sqlx::query(
            "INSERT INTO monitoring_profile (name, slug, enabled) VALUES ($1, $2, true) \
             ON CONFLICT (slug) DO NOTHING",
        )
        .bind(&profile_name)
        .bind(&slug)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        Some(
            sqlx::query_scalar::<_, Uuid>("SELECT id FROM monitoring_profile WHERE slug = $1")
                .bind(&slug)
                .fetch_one(&mut **tx)
                .await
                .map_err(classify_sqlx_error_owned)?,
        )
    };
    let Some(profile_id) = profile_id else {
        return Ok(outcome);
    };

    let restore = input.policy.mode
        == super::super::application::exporter_discovery_reconcile::AbsenceMode::Automatic
        && input.policy.restore_on_reappearance;
    if !applicable.is_empty() {
        sqlx::query(
            r#"
            INSERT INTO monitoring_profile_assignment
                (profile_id, scope_type, scope_id, weight, enabled, target)
            VALUES ($1, 'connection_target', $2, 0, true, NULL)
            ON CONFLICT (profile_id, scope_type, COALESCE(scope_id, '00000000-0000-0000-0000-000000000000'::uuid))
            DO UPDATE SET
                enabled = CASE WHEN $3 THEN true ELSE monitoring_profile_assignment.enabled END,
                updated_at = now()
            "#,
        )
        .bind(profile_id)
        .bind(input.host_id)
        .bind(restore)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }

    for (exporter_id, endpoint) in applicable {
        let job = format!("{}-{}", endpoint.exporter_type, endpoint.identity.port);
        let inserted =
            upsert_discovery_item(tx, profile_id, exporter_id, &job, endpoint, restore).await?;
        if inserted {
            outcome.created += 1;
        } else {
            outcome.updated += 1;
        }
    }

    for endpoint in input.absent {
        let count = input
            .absence_counts
            .get(&endpoint_key(endpoint))
            .copied()
            .unwrap_or(0);
        if !input.policy.may_exclude(count) {
            continue;
        }
        let disabled = sqlx::query(
            r#"
            UPDATE monitoring_profile_item
            SET enabled = false, updated_at = now()
            WHERE profile_id = $1
              AND enabled
              AND COALESCE(port, -1) = $2
              AND COALESCE(metrics_path, '') = $3
              AND COALESCE(scheme, '') = $4
            "#,
        )
        .bind(profile_id)
        .bind(i32::from(endpoint.port))
        .bind(&endpoint.path)
        .bind(&endpoint.scheme)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
        outcome.disabled += usize::try_from(disabled.rows_affected()).unwrap_or(0);
    }

    Ok(outcome)
}

/// Transaction-owning wrapper used by focused tests and simple callers.
pub async fn apply_discovery(
    pool: &PgPool,
    input: &DiscoveryApplyInput<'_>,
) -> Result<DiscoveryApplyOutcome, AppError> {
    let mut tx = begin_source_tx(pool, input.source_id).await?;
    let outcome = apply_discovery_tx(&mut tx, input).await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok(outcome)
}

async fn resolve_exporter_id(
    tx: &mut Transaction<'_, Postgres>,
    exporter_type: &str,
) -> Result<Option<Uuid>, AppError> {
    if exporter_type.is_empty()
        || !exporter_type
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
    {
        return Ok(None);
    }
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM monitoring_exporter \
         WHERE exporter_type = $1 OR slug = $1 \
         ORDER BY (exporter_type = $1) DESC, enabled DESC, id \
         LIMIT 1",
    )
    .bind(exporter_type)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)
}

/// A manual assignment of any effective scope that already covers this endpoint
/// makes the discovery item unnecessary. The scope predicate mirrors
/// `assignment_matches_host`.
async fn manual_coverage_exists(
    tx: &mut Transaction<'_, Postgres>,
    input: &DiscoveryApplyInput<'_>,
    exporter_id: Uuid,
    endpoint: &EndpointIdentity,
) -> Result<bool, AppError> {
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM monitoring_profile_item i
            JOIN monitoring_profile p ON p.id = i.profile_id
            JOIN monitoring_profile_assignment a ON a.profile_id = p.id
            JOIN monitoring_exporter e ON e.id = i.exporter_id
            WHERE p.slug NOT LIKE 'discovery-%'
              AND p.enabled AND i.enabled AND a.enabled AND e.enabled
              AND i.exporter_id = $2
              AND COALESCE(i.port, e.default_port) = $3
              AND COALESCE(i.metrics_path, e.default_metrics_path) = $4
              AND (
                  a.scope_type = 'global'
                  OR (a.scope_type = 'site' AND a.scope_id = $5)
                  OR (a.scope_type = 'tenant' AND a.scope_id = $6)
                  OR (a.scope_type = 'role' AND a.scope_id = $7)
                  OR (a.scope_type = 'connection_target' AND a.scope_id = $8)
                  OR (a.scope_type = 'vm' AND a.scope_id = $8 AND $9 = 'virtual_machine')
                  OR (a.scope_type = 'device' AND a.scope_id = $8 AND $9 = 'device')
                  OR (a.scope_type = 'virtual_machine' AND a.scope_id = $8
                      AND $9 = 'virtual_machine')
              )
        )
        "#,
    )
    .bind(input.host_id)
    .bind(exporter_id)
    .bind(i32::from(endpoint.port))
    .bind(&endpoint.path)
    .bind(input.host_scope.site_id)
    .bind(input.host_scope.tenant_id)
    .bind(input.host_scope.role_id)
    .bind(input.host_id)
    .bind(&input.host_scope.kind)
    .fetch_one(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)
}

async fn upsert_discovery_item(
    tx: &mut Transaction<'_, Postgres>,
    profile_id: Uuid,
    exporter_id: Uuid,
    job: &str,
    endpoint: &ConfirmedEndpoint,
    restore: bool,
) -> Result<bool, AppError> {
    let labels = serde_json::to_value(&endpoint.labels)
        .map_err(|error| AppError::Internal(format!("discovery labels are not JSON: {error}")))?;
    let row = sqlx::query(
        r#"
        INSERT INTO monitoring_profile_item
            (profile_id, exporter_id, job, scheme, port, metrics_path, labels,
             enabled, sort_order)
        VALUES ($1, $2, $3, $4, $5, $6, $7, true, 0)
        ON CONFLICT (profile_id, exporter_id, COALESCE(job, ''))
        DO UPDATE SET
            scheme = EXCLUDED.scheme,
            port = EXCLUDED.port,
            metrics_path = EXCLUDED.metrics_path,
            enabled = CASE WHEN $8 THEN true ELSE monitoring_profile_item.enabled END,
            updated_at = now()
        RETURNING (xmax = 0) AS inserted
        "#,
    )
    .bind(profile_id)
    .bind(exporter_id)
    .bind(job)
    .bind(&endpoint.identity.scheme)
    .bind(i32::from(endpoint.identity.port))
    .bind(&endpoint.identity.path)
    .bind(labels)
    .bind(restore)
    .fetch_one(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    row.try_get::<bool, _>("inserted")
        .map_err(classify_sqlx_error_owned)
}

/// Resolve one report against persisted history inside the source lock.
pub async fn reconcile_report_locked(
    pool: &PgPool,
    host_id: Uuid,
    host_name: &str,
    source_kind: &str,
    host_scope: HostScope,
    policy: DiscoveryPolicy,
    report: &DiscoveryReport,
) -> Result<(bool, usize, usize, DiscoveryApplyOutcome), AppError> {
    let mut tx = begin_source_tx(pool, host_id).await?;
    let latest = latest_finished_at_tx(&mut tx, host_id).await?;
    let confirmed = confirmed_endpoints(&report.endpoints);
    let confirmed_keys = confirmed
        .iter()
        .map(|endpoint| endpoint_key(&endpoint.identity))
        .collect::<std::collections::BTreeSet<_>>();
    let absent = absent_endpoints(&report.endpoints)
        .into_iter()
        .filter(|endpoint| !confirmed_keys.contains(&endpoint_key(endpoint)))
        .collect::<Vec<_>>();
    if is_stale_report(&report.finished_at, latest.as_deref()) {
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        return Ok((
            true,
            confirmed.len(),
            absent.len(),
            DiscoveryApplyOutcome::default(),
        ));
    }
    let reports =
        load_recent_reports_tx(&mut tx, host_id, i64::from(policy.confirm_runs) + 1).await?;
    let absence_counts = consecutive_absences(&reports, &absent);
    let outcome = apply_discovery_tx(
        &mut tx,
        &DiscoveryApplyInput {
            host_id,
            host_name,
            source_kind,
            source_id: host_id,
            host_scope,
            policy,
            confirmed: &confirmed,
            absent: &absent,
            absence_counts: &absence_counts,
        },
    )
    .await?;
    tx.commit().await.map_err(classify_sqlx_error_owned)?;
    Ok((false, confirmed.len(), absent.len(), outcome))
}

#[cfg(test)]
mod db_tests {
    use std::collections::BTreeMap;

    use super::super::super::application::exporter_discovery_reconcile::{
        AbsenceMode, ConfirmedEndpoint, DiscoveryPolicy, endpoint_key,
    };
    use super::super::super::domain::exporter_discovery::EndpointIdentity;
    use super::*;
    use sqlx::migrate::MigrateDatabase;

    const HOST: Uuid = Uuid::from_u128(0x5660_0000_0000_7000_8000_0000_0000_0001);
    const SITE: Uuid = Uuid::from_u128(0x5660_0000_0000_7000_8000_0000_0000_0002);

    fn identity(port: u16) -> EndpointIdentity {
        EndpointIdentity::validated(HOST, "10.0.0.5", port, "http", "/metrics").unwrap()
    }

    fn confirmed(port: u16) -> ConfirmedEndpoint {
        ConfirmedEndpoint {
            identity: identity(port),
            exporter_type: "node".to_string(),
            labels: BTreeMap::new(),
        }
    }

    async fn item_enabled(pool: &PgPool, discovery: bool, job: &str) -> bool {
        let sql = if discovery {
            "SELECT i.enabled FROM monitoring_profile_item i \
             JOIN monitoring_profile p ON p.id = i.profile_id \
             WHERE p.slug LIKE 'discovery-%' AND i.job = $1"
        } else {
            "SELECT i.enabled FROM monitoring_profile_item i \
             JOIN monitoring_profile p ON p.id = i.profile_id \
             WHERE p.slug NOT LIKE 'discovery-%' AND i.job = $1"
        };
        sqlx::query_scalar::<_, bool>(sql)
            .bind(job)
            .fetch_one(pool)
            .await
            .expect("item enabled")
    }

    async fn item_count(pool: &PgPool, job: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM monitoring_profile_item i \
             JOIN monitoring_profile p ON p.id = i.profile_id \
             WHERE p.slug LIKE 'discovery-%' AND i.job = $1",
        )
        .bind(job)
        .fetch_one(pool)
        .await
        .expect("item count")
    }

    type Counts = BTreeMap<(Uuid, String, u16, String, String), u32>;

    async fn run_apply(
        pool: &PgPool,
        confirmed: &[ConfirmedEndpoint],
        absent: &[EndpointIdentity],
        counts: &Counts,
        policy: DiscoveryPolicy,
    ) -> Result<DiscoveryApplyOutcome, AppError> {
        apply_discovery(
            pool,
            &DiscoveryApplyInput {
                host_id: HOST,
                host_name: "db-01",
                source_kind: "virtual_machine",
                source_id: HOST,
                host_scope: HostScope {
                    site_id: Some(SITE),
                    tenant_id: None,
                    role_id: None,
                    kind: "virtual_machine".to_string(),
                },
                policy,
                confirmed,
                absent,
                absence_counts: counts,
            },
        )
        .await
    }

    async fn manual_profile(
        pool: &PgPool,
        slug: &str,
        scope_type: &str,
        scope_id: Option<Uuid>,
        node_id: Uuid,
        port: i32,
        job: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let profile: Uuid = sqlx::query_scalar(
            "INSERT INTO monitoring_profile (name, slug) VALUES ($1, $1) RETURNING id",
        )
        .bind(slug)
        .fetch_one(pool)
        .await?;
        sqlx::query(
            "INSERT INTO monitoring_profile_item \
             (profile_id, exporter_id, job, scheme, port, metrics_path, labels, enabled, sort_order) \
             VALUES ($1, $2, $3, 'http', $4, '/metrics', '{}', true, 0)",
        )
        .bind(profile)
        .bind(node_id)
        .bind(job)
        .bind(port)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO monitoring_profile_assignment \
             (profile_id, scope_type, scope_id, weight, enabled, target) \
             VALUES ($1, $2, $3, 0, true, NULL)",
        )
        .bind(profile)
        .bind(scope_type)
        .bind(scope_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires PIPING_DATABASE__URL with CREATEDB"]
    async fn reconciliation_is_idempotent_and_protects_manual_data() {
        let base = std::env::var("PIPING_DATABASE__URL").expect("PIPING_DATABASE__URL");
        let mut url = url::Url::parse(&base).expect("database url");
        url.set_path(&format!("/expdisc_{}", Uuid::now_v7().simple()));
        let url = url.to_string();
        <Postgres as MigrateDatabase>::create_database(&url)
            .await
            .expect("create database");
        let result = run(&url).await;
        let _ = <Postgres as MigrateDatabase>::force_drop_database(&url).await;
        result.expect("scenario");
    }

    async fn run(url: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let pool = PgPool::connect(url).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let node_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM monitoring_exporter WHERE exporter_type = 'node' LIMIT 1",
        )
        .fetch_one(&pool)
        .await?;
        manual_profile(
            &pool,
            "manual-node",
            "connection_target",
            Some(HOST),
            node_id,
            9100,
            "manual-node",
        )
        .await?;
        manual_profile(
            &pool,
            "site-node",
            "site",
            Some(SITE),
            node_id,
            9300,
            "site-node",
        )
        .await?;
        manual_profile(&pool, "vm-node", "vm", Some(HOST), node_id, 9400, "vm-node").await?;

        let manual = DiscoveryPolicy::default();
        let empty_absent: Vec<EndpointIdentity> = Vec::new();
        let empty_counts = BTreeMap::new();

        // Manual coverage wins for both a host-scoped and a site-scoped manual
        // assignment; no duplicate source-owned item is created.
        let outcome = run_apply(
            &pool,
            &[confirmed(9100), confirmed(9300), confirmed(9400)],
            &empty_absent,
            &empty_counts,
            manual,
        )
        .await?;
        assert_eq!(outcome.manual_preserved, 3);
        assert_eq!(item_count(&pool, "node-9100").await, 0);
        assert_eq!(item_count(&pool, "node-9300").await, 0);
        assert_eq!(item_count(&pool, "node-9400").await, 0);

        // A new endpoint becomes a source-owned item; a replay is idempotent.
        let first = run_apply(
            &pool,
            &[confirmed(9200)],
            &empty_absent,
            &empty_counts,
            manual,
        )
        .await?;
        assert_eq!(first.created, 1);
        assert_eq!(first.updated, 0);
        let replay = run_apply(
            &pool,
            &[confirmed(9200)],
            &empty_absent,
            &empty_counts,
            manual,
        )
        .await?;
        assert_eq!(replay.created, 0);
        assert_eq!(replay.updated, 1);
        assert_eq!(item_count(&pool, "node-9200").await, 1);
        assert!(item_enabled(&pool, false, "manual-node").await);

        // Manual policy keeps the target enabled on confirmed absence.
        let absent = vec![identity(9200)];
        let counts = BTreeMap::from([(endpoint_key(&identity(9200)), 3)]);
        let outcome = run_apply(&pool, &[], &absent, &counts, manual).await?;
        assert_eq!(outcome.disabled, 0);
        assert!(item_enabled(&pool, true, "node-9200").await);

        // Automatic mode disables only the source-owned item after the window.
        let automatic = DiscoveryPolicy {
            mode: AbsenceMode::Automatic,
            confirm_runs: 3,
            restore_on_reappearance: true,
        };
        let outcome = run_apply(&pool, &[], &absent, &counts, automatic).await?;
        assert_eq!(outcome.disabled, 1);
        assert!(!item_enabled(&pool, true, "node-9200").await);
        assert!(item_enabled(&pool, false, "manual-node").await);

        // Manual mode must not re-enable what automatic mode disabled.
        run_apply(
            &pool,
            &[confirmed(9200)],
            &empty_absent,
            &empty_counts,
            manual,
        )
        .await?;
        assert!(!item_enabled(&pool, true, "node-9200").await);

        // Reappearance under the explicit automatic policy restores it.
        run_apply(
            &pool,
            &[confirmed(9200)],
            &empty_absent,
            &empty_counts,
            automatic,
        )
        .await?;
        assert!(item_enabled(&pool, true, "node-9200").await);

        let slug = discovery_profile_slug("virtual_machine", HOST);
        let profiles: i64 =
            sqlx::query_scalar("SELECT count(*) FROM monitoring_profile WHERE slug = $1")
                .bind(&slug)
                .fetch_one(&pool)
                .await?;
        assert_eq!(profiles, 1);
        let _ = pool.close().await;
        Ok(())
    }
}
