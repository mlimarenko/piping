import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Plus, Radar, Trash2 } from "lucide-react";

import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import EmptyState from "@/shared/ui-shared/EmptyState";
import ConfirmDeleteDialog from "@/shared/ui-shared/ConfirmDeleteDialog";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/shared/ui-primitives/select";
import { useApiMutation } from "@/shared/api/hooks";
import { notify } from "@/shared/lib/notifications";
import { usePermissions } from "@/shared/auth/permissionsContext";
import {
  createProfileAssignment,
  deleteProfileAssignment,
  getMonitoringCoverage,
  listAllProfiles,
  serviceDiscoveryCollectorUrl,
  type ProfileAssignmentRequest,
  type MonitoringProfile,
} from "./api";
import { COVERAGE_KEY, PROFILES_KEY, invalidateMonitoringProfileCaches } from "./keys";
import CollectorConfigPanel from "./CollectorConfigPanel";
import {
  assignmentsCoveringOtherScopes,
  assignmentsForScope,
  collectorTargetsOf,
  coverageScope,
  stateBadgeClass,
  type CoverageAssignmentSummary,
  type CoverageHost,
  type CoverageResponse,
  type MonitoringSubject,
} from "./coverage";

function summaryOf(response: CoverageResponse | undefined) {
  return (
    response?.totals ?? { hosts_total: 0, covered: 0, no_scrape: 0, duplicate: 0, filtered: 0 }
  );
}

const SummaryTile = ({ label, value }: { label: string; value: number }) => (
  <div className="rounded-md border border-border bg-background p-2">
    <p className="text-xs uppercase text-muted-foreground">{label}</p>
    <p className="mt-1 text-lg font-semibold tabular-nums">{value}</p>
  </div>
);

const HostRow = ({ host }: { host: CoverageHost }) => {
  const { t } = useTranslation();
  return (
    <div className="grid gap-1.5 p-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="break-all text-sm font-medium text-foreground">{host.host_name}</span>
        <Badge variant="outline" className={stateBadgeClass(host.state)}>
          {t(`observability.monitoring.state.${host.state}`)}
        </Badge>
        {host.status && <span className="text-xs text-muted-foreground">{host.status}</span>}
      </div>
      {host.reason && (
        <p className="text-xs text-muted-foreground">
          {t(`observability.monitoring.reason.${host.reason}`)}
        </p>
      )}
      {host.targets.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {host.targets.map((target) => (
            <Badge
              key={`${target.assignment_id}:${target.target}`}
              variant="secondary"
              className="max-w-full whitespace-normal break-all rounded-md text-xs"
            >
              {target.job ?? "—"} · {target.target} · {target.assignment_name ?? target.exporter_slug}
            </Badge>
          ))}
        </div>
      )}
    </div>
  );
};

const AssignmentRow = ({
  assignment,
  removable,
  onRemove,
}: {
  assignment: CoverageAssignmentSummary;
  removable: boolean;
  onRemove: (assignment: CoverageAssignmentSummary) => void;
}) => {
  const { t } = useTranslation();
  return (
    <div className="flex flex-wrap items-center justify-between gap-2 p-3">
      <div className="min-w-0">
        <p className="break-all text-sm font-medium text-foreground">
          {assignment.name ?? assignment.profile_slug}
        </p>
        <p className="text-xs text-muted-foreground">
          {assignment.scope_type} · {t("observability.monitoring.hosts")}: {assignment.matched_hosts} ·{" "}
          {t("observability.monitoring.targets")}: {assignment.targets}
          {assignment.duplicates > 0 ? ` · ${t("observability.monitoring.state.duplicate")}: ${assignment.duplicates}` : ""}
        </p>
      </div>
      {removable && (
        <Button
          variant="ghost"
          size="sm"
          className="gap-1.5 text-destructive hover:text-destructive"
          onClick={() => onRemove(assignment)}
        >
          <Trash2 className="size-3.5" />
          {t("delete")}
        </Button>
      )}
    </div>
  );
};

const AddAssignmentDialog = ({
  open,
  onOpenChange,
  subject,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  subject: MonitoringSubject;
  onCreated: () => void;
}) => {
  const { t } = useTranslation();
  const { scopeType } = coverageScope(subject);
  const [profileId, setProfileId] = useState("");
  const profiles = useQuery({
    queryKey: [...PROFILES_KEY, "enabled"],
    queryFn: () => listAllProfiles({ enabled: "true" }),
    enabled: open,
  });
  const items: MonitoringProfile[] = profiles.data ?? [];
  const mutation = useApiMutation({
    mutationFn: (body: ProfileAssignmentRequest) => createProfileAssignment(body),
    onSuccess: () => {
      notify("success", t("observability.monitoring.added"));
      onCreated();
      onOpenChange(false);
    },
    onError: () => notify("error", t("observability.monitoring.addFailed")),
  });
  const submit = () => {
    const profile = items.find((item) => item.id === profileId);
    if (!profile) return;
    mutation.mutate({
      profile_id: profile.id,
      scope_type: scopeType,
      scope_id: subject.id,
      weight: 0,
      enabled: true,
      target: null,
    });
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("observability.monitoring.addTitle")}</DialogTitle>
          <DialogDescription>{t("observability.monitoring.addHint")}</DialogDescription>
        </DialogHeader>
        {profiles.isPending && <p role="status">{t("loading")}</p>}
        {profiles.isError && <div role="alert">{t("observability.entityLoadFailed")}<Button variant="outline" onClick={() => void profiles.refetch()}>{t("retry")}</Button></div>}
        {profiles.isSuccess && items.length === 0 && <p>{t("observability.profiles.empty")}</p>}
        <Select value={profileId} onValueChange={setProfileId}>
          <SelectTrigger aria-label={t("observability.profiles.profile")}>
            <SelectValue placeholder={t("observability.profiles.profile")} />
          </SelectTrigger>
          <SelectContent>
            {items.map((item) => (
              <SelectItem key={item.id} value={item.id}>
                {item.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("cancel")}
          </Button>
          <Button onClick={submit} disabled={!profileId || mutation.isPending || !profiles.isSuccess}>
            {t("observability.monitoring.add")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

interface MonitoringSectionProps {
  subject: MonitoringSubject;
}

/**
 * Monitoring properties of one inventory object: which scrapes are effective,
 * which assignment produced each target, and the collector artifact for them.
 */
const MonitoringSection = ({ subject }: MonitoringSectionProps) => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const scope = coverageScope(subject);
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<CoverageAssignmentSummary | null>(null);
  const query = useQuery({
    queryKey: [...COVERAGE_KEY, scope.scopeType, subject.id],
    queryFn: () => getMonitoringCoverage({ ...scope.params, limit: 200 }),
    retry: false,
  });
  const removeMutation = useApiMutation({
    mutationFn: (assignment: CoverageAssignmentSummary) => deleteProfileAssignment(assignment.assignment_id),
    onSuccess: () => {
      notify("success", t("observability.monitoring.removed"));
      setRemoving(null);
      void invalidateCoverage(qc);
    },
    onError: () => notify("error", t("observability.monitoring.removeFailed")),
  });

  const data = query.data;
  const totals = summaryOf(data);
  const hosts = data?.hosts ?? [];
  const ownAssignments = data ? assignmentsForScope(data, subject) : [];
  const otherAssignments = data ? assignmentsCoveringOtherScopes(data, subject) : [];
  const targets = data ? collectorTargetsOf(data) : [];
  const canCreate = can("CREATE", "MONITORING");
  const canDelete = can("DELETE", "MONITORING");

  return (
    <section className="flex flex-col gap-3" aria-label={t("observability.monitoring.title")}>
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0">
          <h2 className="flex items-center gap-1.5 text-base font-semibold">
            <Radar className="size-4" />
            {t("observability.monitoring.title")}
          </h2>
          <p className="text-xs text-muted-foreground">{t("observability.monitoring.hint")}</p>
        </div>
        {canCreate && (
          <Button variant="outline" size="sm" className="gap-1.5" onClick={() => setAdding(true)}>
            <Plus className="size-3.5" />
            {t("observability.monitoring.add")}
          </Button>
        )}
      </div>

      {query.isError ? (
        <SurfaceCard>
          <EmptyState
            icon={AlertTriangle}
            title={t("dataTable.errorTitle")}
            description={t("observability.entityLoadFailed")}
            actionLabel={t("retry")}
            onAction={() => void query.refetch()}
          />
        </SurfaceCard>
      ) : query.isPending ? (
        <SurfaceCard className="p-6 text-center text-sm text-muted-foreground">{t("loading")}</SurfaceCard>
      ) : (
        <>
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
            <SummaryTile label={t("observability.monitoring.state.covered")} value={totals.covered} />
            <SummaryTile label={t("observability.monitoring.state.no_scrape")} value={totals.no_scrape} />
            <SummaryTile label={t("observability.monitoring.state.duplicate")} value={totals.duplicate} />
            <SummaryTile label={t("observability.monitoring.state.filtered")} value={totals.filtered} />
          </div>

          <SurfaceCard className="overflow-hidden">
            <div className="border-b border-border px-3 py-2">
              <p className="text-sm font-semibold">{t("observability.monitoring.hosts")}</p>
            </div>
            {hosts.length === 0 ? (
              <div className="p-4">
                <EmptyState
                  icon={Radar}
                  title={t("observability.monitoring.empty")}
                  description={t("observability.monitoring.emptyHint")}
                />
              </div>
            ) : (
              <div className="divide-y divide-border">
                {hosts.map((host) => (
                  <HostRow key={host.host_id} host={host} />
                ))}
              </div>
            )}
          </SurfaceCard>

          <SurfaceCard className="overflow-hidden">
            <div className="border-b border-border px-3 py-2">
              <p className="text-sm font-semibold">{t("observability.monitoring.scrapes")}</p>
            </div>
            {ownAssignments.length === 0 && otherAssignments.length === 0 ? (
              <div className="p-4">
                <EmptyState
                  icon={Radar}
                  title={t("observability.monitoring.empty")}
                  description={t("observability.monitoring.emptyHint")}
                />
              </div>
            ) : (
              <div className="divide-y divide-border">
                {ownAssignments.map((assignment) => (
                  <AssignmentRow
                    key={assignment.assignment_id}
                    assignment={assignment}
                    removable={canDelete}
                    onRemove={setRemoving}
                  />
                ))}
                {otherAssignments.map((assignment) => (
                  <AssignmentRow
                    key={assignment.assignment_id}
                    assignment={assignment}
                    removable={false}
                    onRemove={setRemoving}
                  />
                ))}
              </div>
            )}
            {targets.length > 0 && (
              <div className="px-3 pb-3">
                <CollectorConfigPanel
                  collectorUrl={serviceDiscoveryCollectorUrl()}
                  targets={targets}
                  subject={subject.label}
                />
              </div>
            )}
          </SurfaceCard>
        </>
      )}

      <AddAssignmentDialog
        open={adding}
        onOpenChange={setAdding}
        subject={subject}
        onCreated={() => void invalidateCoverage(qc)}
      />
      <ConfirmDeleteDialog
        open={removing !== null}
        onOpenChange={(open) => {
          if (!open) setRemoving(null);
        }}
        title={t("observability.monitoring.removeConfirmTitle")}
        description={t("observability.monitoring.removeConfirmDescription")}
        cancelLabel={t("cancel")}
        confirmLabel={t("delete")}
        confirmProps={{
          disabled: removeMutation.isPending,
          onClick: () => {
            if (removing) removeMutation.mutate(removing);
          },
        }}
      />
    </section>
  );
};

async function invalidateCoverage(qc: ReturnType<typeof useQueryClient>): Promise<void> {
  await invalidateMonitoringProfileCaches(qc);
}

export default MonitoringSection;
