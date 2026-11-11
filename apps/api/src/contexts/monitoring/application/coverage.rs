//! Read-only exporter-assignment coverage calculation.
//!
//! The HTTP layer fetches the scoped hosts, their candidate assignments and
//! the rendered targets; this module is the pure part: it classifies every
//! host, detects `job`+`target` collisions across assignments, and aggregates
//! the fleet totals. Keeping it free of database and axum types makes the
//! classification the single testable owner of the coverage vocabulary.
//!
//! The enabled assignment selection and target rendering themselves live in
//! the read model and service-discovery render path, so the preview cannot
//! disagree with the fleet view by using a second resolution path.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

/// One effective target with the assignment that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageTarget {
    pub assignment_id: Uuid,
    pub assignment_name: Option<String>,
    pub exporter_slug: String,
    pub assignment_scope_type: String,
    pub assignment_scope_id: Option<Uuid>,
    pub job: Option<String>,
    pub target: String,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageState {
    Covered,
    NoScrape,
    Duplicate,
    Filtered,
}

impl CoverageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Covered => "covered",
            Self::NoScrape => "no_scrape",
            Self::Duplicate => "duplicate",
            Self::Filtered => "filtered",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageReason {
    NoAssignment,
    DisabledAssignment,
    StatusFiltered,
    SiteStatusFiltered,
    NoTarget,
}

impl CoverageReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoAssignment => "no_assignment",
            Self::DisabledAssignment => "disabled_assignment",
            Self::StatusFiltered => "status_filtered",
            Self::SiteStatusFiltered => "site_status_filtered",
            Self::NoTarget => "no_target",
        }
    }
}

/// Everything known about one scoped host before classification.
#[derive(Debug, Clone)]
pub struct CoverageHostEvidence {
    pub host_id: Uuid,
    pub host_name: String,
    /// Inventory kind of the host (`device` / `virtual_machine`).
    pub kind: String,
    pub status: Option<String>,
    /// Set when a status allow-list (from the SD label settings) excluded the
    /// host from the rendered fleet.
    pub filtered_reason: Option<CoverageReason>,
    /// Assignments whose scope matches this host, disabled ones included.
    pub matching_assignment_ids: Vec<Uuid>,
    /// Matching assignments that are enabled together with their exporter.
    pub enabled_assignment_ids: Vec<Uuid>,
    pub targets: Vec<CoverageTarget>,
}

#[derive(Debug, Clone)]
pub struct CoverageAssignmentInfo {
    pub assignment_id: Uuid,
    pub name: Option<String>,
    pub profile_slug: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct CoverageHostResult {
    pub host_id: Uuid,
    pub host_name: String,
    /// Inventory kind of the host (`device` / `virtual_machine`); the UI links
    /// to the matching inventory detail page.
    pub kind: String,
    pub status: Option<String>,
    pub state: CoverageState,
    pub reason: Option<CoverageReason>,
    pub scrapes: usize,
    pub targets: Vec<CoverageTarget>,
}

#[derive(Debug, Clone)]
pub struct CoverageDuplicateOccurrence {
    pub assignment_id: Uuid,
    pub exporter_slug: String,
}

#[derive(Debug, Clone)]
pub struct CoverageDuplicate {
    pub job: Option<String>,
    pub target: String,
    pub assignments: Vec<CoverageDuplicateOccurrence>,
}

#[derive(Debug, Clone, Default)]
pub struct CoverageTotals {
    pub hosts_total: usize,
    pub covered: usize,
    pub no_scrape: usize,
    pub duplicate: usize,
    pub filtered: usize,
}

#[derive(Debug, Clone)]
pub struct CoverageAssignmentSummary {
    pub assignment_id: Uuid,
    pub name: Option<String>,
    pub profile_slug: String,
    pub scope_type: String,
    pub scope_id: Option<Uuid>,
    pub matched_hosts: usize,
    pub targets: usize,
    pub duplicates: usize,
}

#[derive(Debug, Clone)]
pub struct CoverageResult {
    pub hosts: Vec<CoverageHostResult>,
    pub duplicates: Vec<CoverageDuplicate>,
    pub assignments: Vec<CoverageAssignmentSummary>,
    pub totals: CoverageTotals,
}

/// Classify the scoped fleet and aggregate per-assignment counters.
///
/// Duplicates are keyed by the rendered target string plus the resolved `job`
/// label, and require at least two distinct assignments: `/sd` groups by label
/// set, so a collision is only visible across groups, and a single assignment
/// emitting the same address twice is not two conflicting assignments.
pub fn build_coverage(
    hosts: &[CoverageHostEvidence],
    assignments: &[CoverageAssignmentInfo],
) -> CoverageResult {
    let mut by_key: HashMap<(String, Option<String>), Vec<Uuid>> = HashMap::new();
    for host in hosts.iter().filter(|host| host.filtered_reason.is_none()) {
        for target in &host.targets {
            let occurrences = by_key
                .entry((target.target.clone(), target.job.clone()))
                .or_default();
            if occurrences.contains(&target.assignment_id) {
                continue;
            }
            occurrences.push(target.assignment_id);
        }
    }

    let mut exporter_by_target = HashMap::new();
    for host in hosts.iter().filter(|host| host.filtered_reason.is_none()) {
        for target in &host.targets {
            exporter_by_target.insert(
                (
                    target.assignment_id,
                    target.target.clone(),
                    target.job.clone(),
                ),
                target.exporter_slug.clone(),
            );
        }
    }

    let mut duplicates = by_key
        .into_iter()
        .filter(|(_, occurrences)| occurrences.len() > 1)
        .map(|((target, job), occurrences)| {
            let mut occurrences = occurrences
                .into_iter()
                .map(|assignment_id| CoverageDuplicateOccurrence {
                    exporter_slug: exporter_by_target
                        .get(&(assignment_id, target.clone(), job.clone()))
                        .cloned()
                        .unwrap_or_default(),
                    assignment_id,
                })
                .collect::<Vec<_>>();
            occurrences.sort_by_key(|occurrence| occurrence.assignment_id);
            CoverageDuplicate {
                job,
                target,
                assignments: occurrences,
            }
        })
        .collect::<Vec<_>>();
    duplicates.sort_by(|left, right| {
        left.target
            .cmp(&right.target)
            .then_with(|| left.job.cmp(&right.job))
    });

    let duplicate_keys = duplicates
        .iter()
        .map(|duplicate| (duplicate.target.clone(), duplicate.job.clone()))
        .collect::<HashSet<_>>();

    let mut totals = CoverageTotals {
        hosts_total: hosts.len(),
        ..CoverageTotals::default()
    };
    let mut host_results = Vec::with_capacity(hosts.len());
    for host in hosts {
        let (state, reason) = classify_host(host, &duplicate_keys);
        match state {
            CoverageState::Covered => totals.covered += 1,
            CoverageState::NoScrape => totals.no_scrape += 1,
            CoverageState::Duplicate => totals.duplicate += 1,
            CoverageState::Filtered => totals.filtered += 1,
        }
        host_results.push(CoverageHostResult {
            host_id: host.host_id,
            host_name: host.host_name.clone(),
            kind: host.kind.clone(),
            status: host.status.clone(),
            state,
            reason,
            scrapes: if host.filtered_reason.is_none() {
                // One scrape per profile item; the old model had exactly one
                // target per assignment, so target count is the honest number.
                host.targets.len()
            } else {
                0
            },
            targets: host.targets.clone(),
        });
    }

    let mut seen_assignments = HashSet::new();
    let mut assignment_summaries = assignments
        .iter()
        .filter(|assignment| seen_assignments.insert(assignment.assignment_id))
        .map(|assignment| {
            let matches = hosts
                .iter()
                .filter(|host| {
                    host.filtered_reason.is_none()
                        && host
                            .matching_assignment_ids
                            .contains(&assignment.assignment_id)
                })
                .collect::<Vec<_>>();
            CoverageAssignmentSummary {
                assignment_id: assignment.assignment_id,
                name: assignment.name.clone(),
                profile_slug: assignment.profile_slug.clone(),
                scope_type: assignment.scope_type.clone(),
                scope_id: assignment.scope_id,
                matched_hosts: matches.len(),
                targets: matches
                    .iter()
                    .flat_map(|host| host.targets.iter())
                    .filter(|target| target.assignment_id == assignment.assignment_id)
                    .count(),
                duplicates: duplicates
                    .iter()
                    .filter(|duplicate| {
                        duplicate
                            .assignments
                            .iter()
                            .any(|occurrence| occurrence.assignment_id == assignment.assignment_id)
                    })
                    .count(),
            }
        })
        .collect::<Vec<_>>();
    assignment_summaries.sort_by(|left, right| {
        left.profile_slug
            .cmp(&right.profile_slug)
            .then_with(|| left.assignment_id.cmp(&right.assignment_id))
    });

    CoverageResult {
        hosts: host_results,
        duplicates,
        assignments: assignment_summaries,
        totals,
    }
}

fn classify_host(
    host: &CoverageHostEvidence,
    duplicate_keys: &HashSet<(String, Option<String>)>,
) -> (CoverageState, Option<CoverageReason>) {
    if let Some(reason) = host.filtered_reason {
        return (CoverageState::Filtered, Some(reason));
    }
    if !host.targets.is_empty() {
        let duplicated = host
            .targets
            .iter()
            .any(|target| duplicate_keys.contains(&(target.target.clone(), target.job.clone())));
        return if duplicated {
            (CoverageState::Duplicate, None)
        } else {
            (CoverageState::Covered, None)
        };
    }
    if host.matching_assignment_ids.is_empty() {
        return (CoverageState::NoScrape, Some(CoverageReason::NoAssignment));
    }
    if host.enabled_assignment_ids.is_empty() {
        return (
            CoverageState::NoScrape,
            Some(CoverageReason::DisabledAssignment),
        );
    }
    (CoverageState::NoScrape, Some(CoverageReason::NoTarget))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(host_id: u128) -> CoverageHostEvidence {
        CoverageHostEvidence {
            host_id: Uuid::from_u128(host_id),
            host_name: format!("host-{host_id}"),
            kind: "device".to_string(),
            status: Some("active".to_string()),
            filtered_reason: None,
            matching_assignment_ids: Vec::new(),
            enabled_assignment_ids: Vec::new(),
            targets: Vec::new(),
        }
    }

    fn target(host: &CoverageHostEvidence, assignment_id: u128, job: &str) -> CoverageTarget {
        CoverageTarget {
            assignment_id: Uuid::from_u128(assignment_id),
            assignment_name: Some("preset node".to_string()),
            exporter_slug: "node".to_string(),
            assignment_scope_type: "connection_target".to_string(),
            assignment_scope_id: Some(host.host_id),
            job: Some(job.to_string()),
            target: "10.0.0.1:9100".to_string(),
            labels: HashMap::new(),
        }
    }

    fn assignment(id: u128, slug: &str) -> CoverageAssignmentInfo {
        CoverageAssignmentInfo {
            assignment_id: Uuid::from_u128(id),
            name: Some(format!("{slug} assignment")),
            profile_slug: slug.to_string(),
            scope_type: "connection_target".to_string(),
            scope_id: None,
        }
    }

    #[test]
    fn empty_fleet_yields_zero_totals() {
        let result = build_coverage(&[], &[]);

        assert!(result.hosts.is_empty());
        assert!(result.assignments.is_empty());
        assert!(result.duplicates.is_empty());
        assert_eq!(result.totals.hosts_total, 0);
        assert_eq!(result.totals.covered, 0);
        assert_eq!(result.totals.no_scrape, 0);
    }

    #[test]
    fn host_without_any_assignment_reports_no_assignment() {
        let result = build_coverage(&[evidence(1)], &[]);

        assert_eq!(result.hosts[0].state, CoverageState::NoScrape);
        assert_eq!(result.hosts[0].reason, Some(CoverageReason::NoAssignment));
        assert_eq!(result.totals.no_scrape, 1);
    }

    #[test]
    fn status_filtered_host_is_reported_but_not_scraped_or_matched() {
        let mut host = evidence(1);
        host.filtered_reason = Some(CoverageReason::StatusFiltered);
        host.matching_assignment_ids = vec![Uuid::from_u128(10)];
        host.enabled_assignment_ids = vec![Uuid::from_u128(10)];

        let result = build_coverage(&[host], &[assignment(10, "node")]);

        assert_eq!(result.hosts[0].state, CoverageState::Filtered);
        assert_eq!(result.hosts[0].reason, Some(CoverageReason::StatusFiltered));
        assert_eq!(result.hosts[0].scrapes, 0);
        assert_eq!(result.totals.filtered, 1);
        assert_eq!(result.assignments[0].matched_hosts, 0);
        assert_eq!(result.assignments[0].targets, 0);
    }

    #[test]
    fn same_target_with_a_different_job_is_not_a_duplicate() {
        let mut left = evidence(1);
        left.matching_assignment_ids = vec![Uuid::from_u128(10)];
        left.enabled_assignment_ids = vec![Uuid::from_u128(10)];
        left.targets = vec![target(&left, 10, "node")];

        let mut right = evidence(2);
        right.matching_assignment_ids = vec![Uuid::from_u128(11)];
        right.enabled_assignment_ids = vec![Uuid::from_u128(11)];
        right.targets = vec![target(&right, 11, "windows")];

        let result = build_coverage(&[left, right], &[]);

        assert_eq!(result.totals.duplicate, 0);
        assert!(result.duplicates.is_empty());
    }

    #[test]
    fn no_matching_assignment_reports_no_assignment() {
        let host = evidence(1);

        let result = build_coverage(&[host], &[]);

        assert_eq!(result.hosts[0].state, CoverageState::NoScrape);
        assert_eq!(result.hosts[0].reason, Some(CoverageReason::NoAssignment));
    }

    #[test]
    fn disabled_assignment_is_not_reported_as_no_assignment() {
        let mut host = evidence(1);
        host.matching_assignment_ids = vec![Uuid::from_u128(10)];

        let result = build_coverage(&[host], &[]);

        assert_eq!(result.hosts[0].state, CoverageState::NoScrape);
        assert_eq!(
            result.hosts[0].reason,
            Some(CoverageReason::DisabledAssignment)
        );
    }

    #[test]
    fn duplicate_pair_is_detected_across_assignments_only() {
        let mut left = evidence(1);
        left.matching_assignment_ids = vec![Uuid::from_u128(10)];
        left.enabled_assignment_ids = vec![Uuid::from_u128(10)];
        left.targets = vec![target(&left, 10, "node")];

        let mut right = evidence(2);
        right.matching_assignment_ids = vec![Uuid::from_u128(11)];
        right.enabled_assignment_ids = vec![Uuid::from_u128(11)];
        right.targets = vec![target(&right, 11, "node")];

        let result = build_coverage(&[left, right], &[]);

        assert_eq!(result.totals.duplicate, 2);
        assert_eq!(result.duplicates.len(), 1);
        assert_eq!(result.duplicates[0].assignments.len(), 2);
    }

    #[test]
    fn a_single_assignment_emitting_one_pair_is_not_a_duplicate() {
        let mut host = evidence(1);
        host.matching_assignment_ids = vec![Uuid::from_u128(10)];
        host.enabled_assignment_ids = vec![Uuid::from_u128(10)];
        host.targets = vec![target(&host, 10, "node"), target(&host, 10, "node")];

        let result = build_coverage(&[host], &[]);

        assert_eq!(result.hosts[0].state, CoverageState::Covered);
        assert!(result.duplicates.is_empty());
    }
}
