import type { MonitoringCoverageResponse } from "./api";
import type { CollectorTarget } from "./collectorConfig";

/**
 * Coverage payload produced by `GET /api/v1/monitoring/coverage`.
 *
 * The backend computes it from the same discovery read model as
 * `/api/v1/monitoring/sd/preview`, so every target here also appears in the
 * discovery output with the same job and labels.
 */
export type CoverageResponse = MonitoringCoverageResponse;
export type CoverageHost = CoverageResponse["hosts"][number];
export type CoverageTarget = CoverageHost["targets"][number];
export type CoverageAssignmentSummary = CoverageResponse["assignments"][number];
export type CoverageTotals = CoverageResponse["totals"];

export type CoverageState = "covered" | "no_scrape" | "duplicate" | "filtered";

/** Badge styling shared by the fleet coverage table and the per-object section. */
const STATE_BADGE_CLASS: Record<CoverageState, string> = {
  covered: "border-status-positive/40 bg-status-positive/10 text-status-positive",
  no_scrape: "border-border bg-accent text-muted-foreground",
  duplicate: "border-status-warning/40 bg-status-warning/10 text-status-warning",
  filtered: "border-border bg-accent text-muted-foreground",
};

/** Badge class for a coverage state; unknown states fall back to the neutral one. */
export function stateBadgeClass(state: string): string {
  return STATE_BADGE_CLASS[state as CoverageState] ?? STATE_BADGE_CLASS.no_scrape;
}

/** Inventory detail route for a coverage host; `kind` comes from the backend. */
export function coverageHostHref(host: CoverageHost): string {
  const entity = host.kind === "virtual_machine" ? "Virtual Machines" : "Devices";
  return `/inventory/resources/${encodeURIComponent(entity)}/${encodeURIComponent(host.host_id)}`;
}

export type CoverageReason =
  | "no_assignment"
  | "disabled_assignment"
  | "status_filtered"
  | "site_status_filtered"
  | "no_target";

export type MonitoringSubjectKind = "host" | "role" | "site";

export interface MonitoringSubject {
  kind: MonitoringSubjectKind;
  id: string;
  label: string;
}

export interface CoverageScope {
  scopeType: string;
  params: { host_id?: string; role_id?: string; site_id?: string };
}

/** The inventory entity whose monitoring the section shows. */
export function coverageScope(subject: MonitoringSubject): CoverageScope {
  switch (subject.kind) {
    case "host":
      return { scopeType: "connection_target", params: { host_id: subject.id } };
    case "role":
      return { scopeType: "role", params: { role_id: subject.id } };
    case "site":
      return { scopeType: "site", params: { site_id: subject.id } };
  }
}

/** Assignments bound to exactly this entity; other scopes are shown read-only. */
export function assignmentsForScope(
  response: CoverageResponse,
  subject: MonitoringSubject,
): CoverageAssignmentSummary[] {
  const { scopeType } = coverageScope(subject);
  return response.assignments.filter(
    (assignment) => assignment.scope_type === scopeType && assignment.scope_id === subject.id,
  );
}

export function assignmentsCoveringOtherScopes(
  response: CoverageResponse,
  subject: MonitoringSubject,
): CoverageAssignmentSummary[] {
  const own = new Set(assignmentsForScope(response, subject).map((item) => item.assignment_id));
  return response.assignments.filter((assignment) => !own.has(assignment.assignment_id));
}

/** Every resolved target, deduplicated, in collector-generator form. */
export function collectorTargetsOf(response: CoverageResponse): CollectorTarget[] {
  const seen = new Set<string>();
  const targets: CollectorTarget[] = [];
  for (const host of response.hosts) {
    for (const target of host.targets) {
      const key = `${target.assignment_id}\u0000${target.target}`;
      if (seen.has(key)) continue;
      seen.add(key);
      targets.push({
        assignmentId: target.assignment_id,
        exporterSlug: target.exporter_slug,
        job: target.job ?? null,
        target: target.target,
        labels: target.labels,
      });
    }
  }
  return targets.sort((left, right) =>
    `${left.job ?? ""}\u0000${left.target}`.localeCompare(`${right.job ?? ""}\u0000${right.target}`),
  );
}
