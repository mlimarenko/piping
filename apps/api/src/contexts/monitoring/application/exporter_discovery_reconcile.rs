//! Provenance reconciliation for discovered exporters.
//!
//! The monitoring owner applies only source-owned assignments. Manual profiles
//! and manually edited labels are never touched, a repeated or out-of-order
//! report is idempotent, and history comes from the run artifacts rather than a
//! second ledger.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use sqlx::PgPool;

use crate::errors::AppError;

use super::super::domain::exporter_discovery::{
    DiscoveryReport, DiscoveryStatus, EndpointIdentity, EndpointObservation,
};

/// Absence policy mode. `manual` is the default for new and existing settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbsenceMode {
    Manual,
    Automatic,
}

impl AbsenceMode {
    pub fn parse(value: &str) -> Result<Self, AppError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "manual" => Ok(Self::Manual),
            "automatic" => Ok(Self::Automatic),
            _ => Err(AppError::Validation(
                "discovery absence policy must be `manual` or `automatic`".to_string(),
            )),
        }
    }
}

pub const DISCOVERY_DEFAULT_CONFIRM_RUNS: u32 = 3;
pub const DISCOVERY_MAX_CONFIRM_RUNS: u32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryPolicy {
    pub mode: AbsenceMode,
    pub confirm_runs: u32,
    pub restore_on_reappearance: bool,
}

impl Default for DiscoveryPolicy {
    fn default() -> Self {
        Self {
            mode: AbsenceMode::Manual,
            confirm_runs: DISCOVERY_DEFAULT_CONFIRM_RUNS,
            restore_on_reappearance: true,
        }
    }
}

impl DiscoveryPolicy {
    pub fn validated(mode: &str, confirm_runs: u32, restore: bool) -> Result<Self, AppError> {
        let mode = AbsenceMode::parse(mode)?;
        if !(1..=DISCOVERY_MAX_CONFIRM_RUNS).contains(&confirm_runs) {
            return Err(AppError::Validation(
                "discovery absence confirmation window is out of bounds".to_string(),
            ));
        }
        Ok(Self {
            mode,
            confirm_runs,
            restore_on_reappearance: restore,
        })
    }

    /// Automatic exclusion requires an explicitly enabled mode and a complete
    /// confirmed absence in `confirm_runs` consecutive complete runs.
    #[must_use]
    pub fn may_exclude(&self, consecutive_absences: u32) -> bool {
        self.mode == AbsenceMode::Automatic && consecutive_absences >= self.confirm_runs
    }
}

/// One confirmed endpoint that may produce a source-owned assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedEndpoint {
    pub identity: EndpointIdentity,
    pub exporter_type: String,
    pub labels: BTreeMap<String, String>,
}

/// Stable key used for history comparison.
pub type EndpointKey = (Uuid, String, u16, String, String);

pub fn endpoint_key(endpoint: &EndpointIdentity) -> EndpointKey {
    endpoint.sort_key()
}

/// Confirmed discoveries from either source, deduplicated by endpoint identity.
/// Unverified, unreachable, error, skipped and absent observations never
/// produce an assignment.
#[must_use]
pub fn confirmed_endpoints(observations: &[EndpointObservation]) -> Vec<ConfirmedEndpoint> {
    let mut confirmed = BTreeMap::<(Uuid, String, u16, String, String), ConfirmedEndpoint>::new();
    for observation in observations {
        if observation.status != DiscoveryStatus::ConfirmedDiscovered {
            continue;
        }
        let Some(exporter_type) = observation
            .exporter_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        confirmed
            .entry(endpoint_key(&observation.endpoint))
            .or_insert_with(|| ConfirmedEndpoint {
                identity: observation.endpoint.clone(),
                exporter_type: exporter_type.to_string(),
                labels: observation.labels.clone(),
            });
    }
    confirmed.into_values().collect()
}

/// Distinct absent endpoint identities from complete host inspection only.
#[must_use]
pub fn absent_endpoints(observations: &[EndpointObservation]) -> Vec<EndpointIdentity> {
    let mut absent = BTreeSet::new();
    let mut result = Vec::new();
    for observation in observations {
        if observation.status == DiscoveryStatus::ConfirmedAbsent {
            let key = endpoint_key(&observation.endpoint);
            if absent.insert(key) {
                result.push(observation.endpoint.clone());
            }
        }
    }
    result
}

/// Consecutive confirmed-absence counts from the newest report backwards.
///
/// A report counts only when it has a `confirmed_absent` observation for the
/// identity; any other status for that identity stops the streak. The current
/// report is included once; duplicate reports for the same run are ignored.
#[must_use]
pub fn consecutive_absences(
    reports_newest_first: &[DiscoveryReport],
    identities: &[EndpointIdentity],
) -> BTreeMap<EndpointKey, u32> {
    let mut counts = identities
        .iter()
        .map(|identity| (endpoint_key(identity), 0_u32))
        .collect::<BTreeMap<_, _>>();
    let mut finished = BTreeSet::<Uuid>::new();
    let mut stopped = BTreeSet::<EndpointKey>::new();
    for report in reports_newest_first {
        if !finished.insert(report.run_id) {
            continue;
        }
        for identity in identities {
            let key = endpoint_key(identity);
            if stopped.contains(&key) {
                continue;
            }
            let observation = report
                .endpoints
                .iter()
                .find(|item| item.endpoint == *identity);
            match observation.map(|item| item.status) {
                Some(DiscoveryStatus::ConfirmedAbsent) => {
                    *counts.entry(key).or_insert(0) += 1;
                }
                Some(_) | None => {
                    // A report without the identity or with any other status
                    // ends the streak; later reports are older.
                    stopped.insert(key);
                }
            }
        }
    }
    counts
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ReconcileOutcome {
    pub stale: bool,
    pub confirmed: usize,
    pub absent: usize,
    pub created: usize,
    pub updated: usize,
    pub manual_preserved: usize,
    pub disabled: usize,
    pub skipped_unknown_type: usize,
}

/// Apply one resolved report to the source-owned assignments under the
/// per-source lock. All reads that gate the decision happen inside the same
/// transaction as the writes.
pub async fn reconcile_report(
    pool: &PgPool,
    host_id: Uuid,
    host_name: &str,
    source_kind: &str,
    host_scope: super::super::infrastructure::discovery_store::HostScope,
    policy: DiscoveryPolicy,
    report: &DiscoveryReport,
) -> Result<ReconcileOutcome, AppError> {
    let (stale, confirmed, absent, apply) =
        super::super::infrastructure::discovery_store::reconcile_report_locked(
            pool,
            host_id,
            host_name,
            source_kind,
            host_scope,
            policy,
            report,
        )
        .await?;
    Ok(ReconcileOutcome {
        stale,
        confirmed,
        absent,
        created: apply.created,
        updated: apply.updated,
        manual_preserved: apply.manual_preserved,
        disabled: apply.disabled,
        skipped_unknown_type: apply.skipped_unknown_type,
    })
}

/// Whether a report applies to the host and source and is not older than the
/// latest applied report. Out-of-order reports are stored as history but do not
/// rewrite assignments.
#[must_use]
pub fn is_stale_report(report_finished_at: &str, latest_finished_at: Option<&str>) -> bool {
    let Some(latest) = latest_finished_at else {
        return false;
    };
    let Ok(candidate) = chrono::DateTime::parse_from_rfc3339(report_finished_at) else {
        return true;
    };
    let Ok(latest) = chrono::DateTime::parse_from_rfc3339(latest) else {
        return false;
    };
    candidate < latest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::monitoring::domain::exporter_discovery::{
        DiscoveryProof, ObservationProvenance, reason,
    };

    fn identity(port: u16) -> EndpointIdentity {
        EndpointIdentity::validated(Uuid::from_u128(1), "10.0.0.5", port, "http", "/metrics")
            .unwrap()
    }

    fn observation(
        port: u16,
        status: DiscoveryStatus,
        exporter_type: Option<&str>,
    ) -> EndpointObservation {
        EndpointObservation {
            endpoint: identity(port),
            source: crate::contexts::monitoring::domain::exporter_discovery::DiscoverySource::HostInspection,
            platform: Some(crate::contexts::monitoring::domain::exporter_discovery::DiscoveryPlatform::Linux),
            observed_at: "2026-11-10T00:00:00Z".to_string(),
            provenance: ObservationProvenance {
                run_id: Uuid::from_u128(2),
                run_group_id: Uuid::from_u128(3),
                connection_target_id: Uuid::from_u128(1),
                source_kind: "virtual_machine".to_string(),
                source_id: Uuid::from_u128(1),
                observed_from: "runner".to_string(),
            },
            proof: DiscoveryProof::default(),
            status,
            reason: if status == DiscoveryStatus::ConfirmedAbsent {
                reason::NO_LISTENER.to_string()
            } else {
                reason::TYPED_LISTENER_OWNER.to_string()
            },
            exporter_type: exporter_type.map(str::to_string),
            labels: BTreeMap::new(),
        }
    }

    fn report(run: u128, observations: Vec<EndpointObservation>) -> DiscoveryReport {
        DiscoveryReport {
            schema_version: 1,
            run_id: Uuid::from_u128(run),
            run_group_id: Uuid::from_u128(3),
            started_at: "2026-11-10T00:00:00Z".to_string(),
            finished_at: "2026-11-10T00:00:05Z".to_string(),
            sources: BTreeMap::new(),
            endpoints: observations,
            groups: Vec::new(),
        }
    }

    #[test]
    fn only_confirmed_typed_observations_plan_assignments() {
        let confirmed = confirmed_endpoints(&[
            observation(9100, DiscoveryStatus::ConfirmedDiscovered, Some("node")),
            observation(9200, DiscoveryStatus::Unverified, Some("node")),
            observation(9300, DiscoveryStatus::ConfirmedDiscovered, None),
            observation(9400, DiscoveryStatus::Unreachable, Some("node")),
        ]);
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].identity.port, 9100);
    }

    #[test]
    fn absence_streak_stops_at_any_other_status_and_dedupes_replay() {
        let reports = vec![
            report(
                10,
                vec![observation(9100, DiscoveryStatus::ConfirmedAbsent, None)],
            ),
            report(
                9,
                vec![observation(9100, DiscoveryStatus::ConfirmedAbsent, None)],
            ),
            report(
                9,
                vec![observation(9100, DiscoveryStatus::ConfirmedAbsent, None)],
            ),
            report(
                8,
                vec![observation(9100, DiscoveryStatus::Unreachable, None)],
            ),
            report(
                7,
                vec![observation(9100, DiscoveryStatus::ConfirmedAbsent, None)],
            ),
        ];
        let counts = consecutive_absences(&reports, &[identity(9100)]);
        assert_eq!(counts.values().next().copied(), Some(2));
    }

    #[test]
    fn manual_policy_never_excludes_and_automatic_needs_the_window() {
        let manual = DiscoveryPolicy::default();
        assert!(!manual.may_exclude(10));
        let automatic = DiscoveryPolicy {
            mode: AbsenceMode::Automatic,
            confirm_runs: 3,
            restore_on_reappearance: true,
        };
        assert!(!automatic.may_exclude(2));
        assert!(automatic.may_exclude(3));
    }

    #[test]
    fn stale_reports_are_detected_by_finish_time() {
        assert!(!is_stale_report("2026-11-10T00:00:05Z", None));
        assert!(is_stale_report(
            "2026-11-09T00:00:05Z",
            Some("2026-11-10T00:00:05Z")
        ));
        assert!(!is_stale_report(
            "2026-11-10T00:00:06Z",
            Some("2026-11-10T00:00:05Z")
        ));
    }
}
