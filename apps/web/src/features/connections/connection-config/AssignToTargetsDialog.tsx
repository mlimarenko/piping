import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  CircleAlert,
  Loader2,
  Search,
  ServerCog,
  X,
} from "lucide-react";

import {
  CONNECTION_TARGETS_QUERY_KEY,
  listConnectionTargets,
} from "@/features/connections/api";
import { ApiError } from "@/shared/api/client";
import type { ConnectionTarget } from "@/shared/api/types";
import type { Uuid } from "@/shared/lib/ids";
import { useApiMutation } from "@/shared/api/hooks";
import { cn } from "@/shared/lib/utils";
import { Button } from "@/shared/ui-primitives/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import { Input } from "@/shared/ui-primitives/input";
import FormField from "@/shared/ui-shared/FormField";
import {
  bulkAssignRecord,
  listAssignments,
  listRecords,
  type BulkAssignRecordResponse,
  type ConnConfAssignment,
} from "./api";

const MAX_BULK_ASSIGN_TARGETS = 1_000;
const VISIBLE_TARGET_LIMIT = 100;

interface AssignToTargetsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  recordId: string;
}

interface CompetingAssignment {
  assignment: ConnConfAssignment;
  recordName: string;
}

function targetSubtitle(target: ConnectionTarget): string {
  return [target.targetKind === "virtual_machine" ? "VM" : null, target.site, target.role]
    .filter(Boolean)
    .join(" · ");
}

export default function AssignToTargetsDialog({
  open,
  onOpenChange,
  recordId,
}: AssignToTargetsDialogProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [selectedIds, setSelectedIds] = useState<Uuid[]>([]);
  const [search, setSearch] = useState("");
  const [weight, setWeight] = useState(0);
  const [result, setResult] = useState<BulkAssignRecordResponse | null>(null);

  const sentinel = open ? recordId : "__closed__";
  const [previousSentinel, setPreviousSentinel] = useState(sentinel);
  if (sentinel !== previousSentinel) {
    setPreviousSentinel(sentinel);
    setSelectedIds([]);
    setSearch("");
    setWeight(0);
    setResult(null);
  }

  const targetsQuery = useQuery({
    queryKey: CONNECTION_TARGETS_QUERY_KEY,
    queryFn: () => listConnectionTargets(),
    enabled: open,
    staleTime: 60_000,
  });
  const recordsQuery = useQuery({
    queryKey: ["connection-config", "records"],
    queryFn: () => listRecords(),
    enabled: open,
    staleTime: 60_000,
  });
  const assignmentsQuery = useQuery({
    queryKey: ["connection-config", "assignments", "bulk-competition"],
    queryFn: () => listAssignments(),
    enabled: open,
  });

  const targets = useMemo(() => targetsQuery.data?.items ?? [], [targetsQuery.data?.items]);
  const targetById = useMemo(
    () => new Map<string, ConnectionTarget>(targets.map((target) => [target.id, target])),
    [targets],
  );
  const records = useMemo(() => recordsQuery.data ?? [], [recordsQuery.data]);
  const recordById = useMemo(
    () => new Map(records.map((record) => [record.id, record])),
    [records],
  );
  const currentProtocol = recordById.get(recordId)?.protocol;
  const selectedIdSet = useMemo(() => new Set(selectedIds), [selectedIds]);
  const filteredTargets = useMemo(() => {
    const needle = search.trim().toLocaleLowerCase();
    const matches = needle
      ? targets.filter((target) =>
          [target.name, target.site, target.role, target.primaryIp]
            .filter(Boolean)
            .some((value) => value?.toLocaleLowerCase().includes(needle)),
        )
      : targets;
    return matches.slice(0, VISIBLE_TARGET_LIMIT);
  }, [search, targets]);

  const competingByTarget = useMemo(() => {
    const map = new Map<string, CompetingAssignment[]>();
    for (const assignment of assignmentsQuery.data ?? []) {
      if (
        !assignment.enabled ||
        assignment.record_id === recordId ||
        (assignment.scope_type !== "device" && assignment.scope_type !== "vm") ||
        !assignment.scope_id
      ) {
        continue;
      }
      const competingRecord = recordById.get(assignment.record_id);
      if (currentProtocol && competingRecord?.protocol !== currentProtocol) continue;
      const competing = {
        assignment,
        recordName: competingRecord?.name ?? assignment.record_id,
      };
      map.set(assignment.scope_id, [...(map.get(assignment.scope_id) ?? []), competing]);
    }
    return map;
  }, [assignmentsQuery.data, currentProtocol, recordById, recordId]);
  const selectedCompeting = useMemo(
    () =>
      selectedIds.flatMap((targetId) =>
        (competingByTarget.get(targetId) ?? []).map((competing) => ({
          targetId,
          targetName: targetById.get(targetId)?.name ?? targetId,
          ...competing,
        })),
      ),
    [competingByTarget, selectedIds, targetById],
  );

  const mutation = useApiMutation({
    mutationFn: () => bulkAssignRecord(recordId, selectedIds, weight),
    onSuccess: (response) => {
      setResult(response);
      void queryClient.invalidateQueries({
        queryKey: ["connection-config", "assignments"],
      });
    },
  });

  const toggleTarget = (targetId: Uuid) => {
    setResult(null);
    setSelectedIds((current) =>
      current.includes(targetId)
        ? current.filter((id) => id !== targetId)
        : current.length < MAX_BULK_ASSIGN_TARGETS
          ? [...current, targetId]
          : current,
    );
  };

  const handleOpenChange = (nextOpen: boolean) => {
    if (!mutation.isPending) onOpenChange(nextOpen);
  };

  const isLoading = targetsQuery.isPending || assignmentsQuery.isPending || recordsQuery.isPending;
  const loadError = targetsQuery.error ?? assignmentsQuery.error ?? recordsQuery.error;
  const canSubmit = selectedIds.length > 0 && !isLoading && !loadError && !mutation.isPending;

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-[960px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-xl">
            <ServerCog className="h-4 w-4 text-primary" aria-hidden />
            {t("connectionConfig.bulkAssign.title")}
          </DialogTitle>
          <DialogDescription className="text-xs">
            {t("connectionConfig.bulkAssign.description")}
          </DialogDescription>
        </DialogHeader>

        <fieldset disabled={mutation.isPending} inert={mutation.isPending} className="flex min-w-0 flex-col gap-4" aria-busy={mutation.isPending}>
          <div className="flex flex-wrap items-end gap-4">
            <FormField
              className="min-w-0 flex-1"
              label={t("connectionConfig.bulkAssign.targets")}
              htmlFor="bulk-target-search"
            >
              <div className="relative">
                <Search className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
                <Input
                  id="bulk-target-search"
                  aria-label={t("connectionConfig.bulkAssign.search")}
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder={t("connectionConfig.bulkAssign.search")}
                  className="h-9 pl-8 text-sm"
                />
                {search ? (
                  <button
                    type="button"
                    className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
                    aria-label={t("connectionConfig.bulkAssign.clearSearch")}
                    onClick={() => setSearch("")}
                  >
                    <X className="h-3.5 w-3.5" />
                  </button>
                ) : null}
              </div>
            </FormField>
            <FormField
              className="w-24"
              label={t("connectionConfig.assignments.weight")}
              htmlFor="bulk-assignment-weight"
            >
              <Input
                id="bulk-assignment-weight"
                type="number"
                aria-label={t("connectionConfig.assignments.weight")}
                className="h-9 text-center font-mono text-sm"
                value={weight}
                onChange={(event) => {
                  setResult(null);
                  setWeight(Number(event.target.value) || 0);
                }}
              />
            </FormField>
          </div>

          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>{t("connectionConfig.bulkAssign.selected", { count: selectedIds.length })}</span>
            {selectedIds.length > 0 ? (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-9 px-3 text-sm"
                onClick={() => {
                  setSelectedIds([]);
                  setResult(null);
                }}
              >
                {t("connectionConfig.bulkAssign.clearAll")}
              </Button>
            ) : null}
          </div>

          <div className="max-h-60 space-y-1 overflow-y-auto overscroll-contain rounded-lg border border-border/40 bg-background p-1.5">
            {isLoading ? (
              <div className="flex items-center justify-center gap-2 px-3 py-8 text-xs text-muted-foreground">
                <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
                {t("loading")}
              </div>
            ) : loadError ? (
              <p className="px-3 py-8 text-center text-xs text-destructive">
                {t("connectionConfig.bulkAssign.loadFailed")}
              </p>
            ) : filteredTargets.length === 0 ? (
              <p className="px-3 py-8 text-center text-xs text-muted-foreground">
                {t("connectionConfig.bulkAssign.noTargets")}
              </p>
            ) : (
              filteredTargets.map((target) => {
                const isSelected = selectedIdSet.has(target.id);
                const competitionCount = competingByTarget.get(target.id)?.length ?? 0;
                return (
                  <button
                    key={target.id}
                    type="button"
                    aria-pressed={isSelected}
                    onClick={() => toggleTarget(target.id)}
                    className={cn(
                      "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors",
                      isSelected ? "bg-primary/10" : "hover:bg-accent",
                    )}
                  >
                    <span
                      className={cn(
                        "flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-[4px] border",
                        isSelected
                          ? "border-primary bg-primary text-primary-foreground"
                          : "border-input/30",
                      )}
                    >
                      {isSelected ? <Check className="h-2.5 w-2.5" /> : null}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-xs font-medium text-foreground">
                        {target.name}
                      </span>
                      <span className="block truncate text-xs text-muted-foreground">
                        {targetSubtitle(target) || "—"}
                      </span>
                    </span>
                    {competitionCount > 0 ? (
                      <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-status-warning" aria-hidden />
                    ) : null}
                  </button>
                );
              })
            )}
          </div>

          {selectedCompeting.length > 0 ? (
            <section
              aria-label={t("connectionConfig.bulkAssign.competingTitle")}
              className="rounded-lg border border-status-warning/35 bg-status-warning/10 p-3"
            >
              <div className="flex items-start gap-2">
                <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-status-warning" aria-hidden />
                <div className="min-w-0">
                  <p className="text-xs font-semibold text-foreground">
                    {t("connectionConfig.bulkAssign.competingTitle")}
                  </p>
                  <p className="mt-0.5 text-xs leading-relaxed text-muted-foreground">
                    {t("connectionConfig.bulkAssign.competingHint")}
                  </p>
                  <ul className="mt-2 space-y-1 text-xs text-foreground">
                    {selectedCompeting.map(({ targetId, targetName, assignment, recordName }) => (
                      <li key={`${targetId}:${assignment.id}`}>
                        <span className="font-medium">{targetName}</span>
                        {" — "}
                        {recordName} ({t("connectionConfig.bulkAssign.weightValue", { weight: assignment.weight })})
                      </li>
                    ))}
                  </ul>
                </div>
              </div>
            </section>
          ) : null}

          {mutation.error ? (
            <p role="alert" className="text-xs text-destructive">
              {mutation.error instanceof ApiError
                ? mutation.error.message
                : t("connectionConfig.bulkAssign.failed")}
            </p>
          ) : null}

          {result ? (
            <section aria-label={t("connectionConfig.bulkAssign.resultsTitle")} className="space-y-2">
              <div className="flex flex-wrap gap-2 text-xs">
                <span className="rounded bg-status-positive/10 px-2 py-1 text-status-positive">
                  {t("connectionConfig.bulkAssign.assignedCount", { count: result.assigned_count })}
                </span>
                <span className="rounded bg-muted px-2 py-1 text-muted-foreground">
                  {t("connectionConfig.bulkAssign.alreadyCount", { count: result.already_assigned_count })}
                </span>
                <span className="rounded bg-destructive/10 px-2 py-1 text-destructive">
                  {t("connectionConfig.bulkAssign.failedCount", { count: result.failed_count })}
                </span>
              </div>
              <ul className="max-h-44 space-y-1 overflow-y-auto rounded-lg border border-border/40 p-1.5">
                {result.items.map((item) => {
                  const targetName = targetById.get(item.target_id)?.name ?? item.target_id;
                  const isAssigned = item.status === "assigned";
                  const isAlreadyAssigned = item.status === "already_assigned";
                  return (
                    <li key={item.target_id} className="flex items-start gap-2 rounded-md px-2 py-1.5 text-xs">
                      {isAssigned ? (
                        <CheckCircle2 className="mt-0.5 h-3.5 w-3.5 shrink-0 text-status-positive" aria-hidden />
                      ) : isAlreadyAssigned ? (
                        <Check className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" aria-hidden />
                      ) : (
                        <CircleAlert className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" aria-hidden />
                      )}
                      <span className="min-w-0 flex-1">
                        <span className="font-medium text-foreground">{targetName}</span>
                        <span className="ml-2 text-muted-foreground">
                          {t(`connectionConfig.bulkAssign.status.${isAssigned ? "assigned" : isAlreadyAssigned ? "alreadyAssigned" : "failed"}`)}
                        </span>
                        {item.error ? (
                          <span className="mt-0.5 block break-words text-xs text-destructive">
                            {item.error}
                          </span>
                        ) : null}
                      </span>
                    </li>
                  );
                })}
              </ul>
            </section>
          ) : null}
        </fieldset>

        <DialogFooter>
          <Button variant="outline" size="sm" disabled={mutation.isPending} onClick={() => handleOpenChange(false)}>
            {result ? t("close") : t("cancel")}
          </Button>
          {!result ? (
            <Button
              size="sm"
              className="gap-1.5"
              disabled={!canSubmit}
              onClick={() => mutation.mutate()}
            >
              {mutation.isPending ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden />
              ) : (
                <ServerCog className="h-3.5 w-3.5" aria-hidden />
              )}
              {t("connectionConfig.bulkAssign.submit", { count: selectedIds.length })}
            </Button>
          ) : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
