import { useTranslation } from "react-i18next";
import { Link } from "react-router";
import { ArrowUpRight, CircleAlert } from "lucide-react";

import { ApiError } from "@/shared/api/client";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui-primitives/alert-dialog";
import type { PlaybookTargetSetUsage } from "@/features/playbooks/api";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";
import {
  jobTemplateEditorEditHref,
  scheduleEditorEditHref,
} from "@/features/playbooks/playbookEditorRoutes";

/** One object that keeps the set in use, with a way back to its editor. */
type BlockerItem = {
  key: string;
  label: string;
  href: string | null;
};

type BlockerGroup = {
  key: string;
  label: string;
  count: number;
  items: BlockerItem[];
};

const GROUP_LABEL_KEYS: Record<string, string> = {
  playbook_job_template: "playbooks.page.targetSets.blockers.jobTemplates",
  playbook_assignment: "playbooks.page.targetSets.blockers.assignments",
  playbook_schedule: "playbooks.page.targetSets.blockers.schedules",
};

function blockerHref(table: string, id: string | null): string | null {
  // Assignments have no per-row editor; the assignment list is their editor.
  if (table === "playbook_assignment") return PLAYBOOK_TAB_ROUTES.assignments;
  if (!id) return null;
  if (table === "playbook_job_template") return jobTemplateEditorEditHref(id);
  if (table === "playbook_schedule") return scheduleEditorEditHref(id);
  return null;
}

/** Blocker groups from a structured 409 `DELETE_BLOCKED` response. */
function parseBlockedDetails(error: unknown): BlockerGroup[] {
  if (!(error instanceof ApiError)) return [];
  if (error.code !== "DELETE_BLOCKED" && error.reasonCode !== "DELETE_BLOCKED") return [];
  const details = error.details as { blockers?: unknown } | undefined;
  const rawGroups = Array.isArray(details?.blockers) ? details.blockers : [];
  const groups: BlockerGroup[] = [];
  for (const raw of rawGroups) {
    if (!raw || typeof raw !== "object") continue;
    const group = raw as { table?: unknown; count?: unknown; examples?: unknown };
    if (typeof group.table !== "string" || typeof group.count !== "number") continue;
    const rawExamples = Array.isArray(group.examples) ? group.examples : [];
    const items: BlockerItem[] = [];
    for (const [index, rawExample] of rawExamples.entries()) {
      if (!rawExample || typeof rawExample !== "object") continue;
      const example = rawExample as { id?: unknown; label?: unknown };
      const id = typeof example.id === "string" ? example.id : null;
      const label =
        typeof example.label === "string" ? example.label : (id ?? group.table);
      items.push({
        key: `${group.table}:${id ?? index}`,
        label,
        href: blockerHref(group.table, id),
      });
    }
    groups.push({
      key: group.table,
      label: group.table,
      count: group.count,
      items,
    });
  }
  return groups;
}

/** Blocker groups from the list's preflight usage projection. */
function usageGroups(
  usage: PlaybookTargetSetUsage | undefined,
  labels: { jobTemplates: string; assignments: string; schedules: string },
): BlockerGroup[] {
  if (!usage) return [];
  const groups: BlockerGroup[] = [
    {
      key: "playbook_job_template",
      label: labels.jobTemplates,
      count: usage.job_templates.length,
      items: usage.job_templates.map((reference) => ({
        key: `job-${reference.id}`,
        label: reference.name ?? reference.id,
        href: jobTemplateEditorEditHref(reference.id),
      })),
    },
    {
      key: "playbook_assignment",
      label: labels.assignments,
      count: usage.assignments.length,
      items: usage.assignments.map((reference) => ({
        key: `assignment-${reference.id}`,
        label: reference.name ?? reference.id,
        href: PLAYBOOK_TAB_ROUTES.assignments,
      })),
    },
    {
      key: "playbook_schedule",
      label: labels.schedules,
      count: usage.schedules.length,
      items: usage.schedules.map((reference) => ({
        key: `schedule-${reference.id}`,
        label: reference.name ?? reference.id,
        href: scheduleEditorEditHref(reference.id),
      })),
    },
  ];
  return groups.filter((group) => group.items.length > 0);
}

export function DeleteTargetSetDialog({
  open,
  name,
  usage,
  error,
  pending,
  onOpenChange,
  onConfirm,
}: Readonly<{
  open: boolean;
  name: string;
  usage?: PlaybookTargetSetUsage;
  error?: unknown;
  pending: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}>) {
  const { t } = useTranslation();
  const blockedGroups = parseBlockedDetails(error);
  const preflightGroups = usageGroups(usage, {
    jobTemplates: t("playbooks.page.targetSets.blockers.jobTemplates"),
    assignments: t("playbooks.page.targetSets.blockers.assignments"),
    schedules: t("playbooks.page.targetSets.blockers.schedules"),
  });
  const groups = blockedGroups.length > 0 ? blockedGroups : preflightGroups;
  const blocked = groups.length > 0;
  const total = groups.reduce((sum, group) => sum + group.count, 0);

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-lg">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("playbooks.page.targetSets.deleteTitle")}</AlertDialogTitle>
          <AlertDialogDescription>
            {t("playbooks.page.targetSets.deleteDescription", { name })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {blocked ? (
          <div className="min-w-0 space-y-3">
            <div className="flex items-start gap-2 rounded-lg border border-warning/30 bg-warning/10 p-3 text-sm text-foreground">
              <CircleAlert className="mt-0.5 h-4 w-4 shrink-0 text-warning" />
              <div className="space-y-1">
                <p className="font-medium">{t("playbooks.page.targetSets.deleteBlockedTitle")}</p>
                <p className="text-xs text-muted-foreground">
                  {t("playbooks.page.targetSets.deleteBlockedSummary", { count: total })}
                </p>
                <p className="text-xs text-muted-foreground">
                  {t("playbooks.page.targetSets.deleteBlockedHint")}
                </p>
              </div>
            </div>
            <div className="min-w-0 space-y-2">
              {groups.map((group) => {
                const labelKey = GROUP_LABEL_KEYS[group.key];
                return (
                <div
                  key={group.key}
                  className="min-w-0 rounded-lg border border-border bg-accent/10 p-3"
                >
                  <div className="flex items-center justify-between gap-3">
                    <p className="min-w-0 truncate text-sm font-medium">
                      {labelKey ? t(labelKey) : group.label}
                    </p>
                    <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
                      {group.count}
                    </span>
                  </div>
                  {group.items.length > 0 ? (
                    <ul className="mt-2 space-y-1">
                      {group.items.map((item) => (
                        <li
                          key={item.key}
                          className="flex flex-col gap-1 rounded-md border border-border bg-background px-2.5 py-2 text-sm sm:flex-row sm:items-center sm:justify-between sm:gap-2"
                        >
                          <span className="min-w-0 truncate">{item.label}</span>
                          {item.href ? (
                            <Link
                              to={item.href}
                              onClick={() => onOpenChange(false)}
                              className="inline-flex h-9 shrink-0 items-center gap-1 rounded px-2 text-sm underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                            >
                              <ArrowUpRight className="h-3.5 w-3.5" />
                              {t("playbooks.page.targetSets.reassign")}
                            </Link>
                          ) : null}
                        </li>
                      ))}
                    </ul>
                  ) : null}
                  {group.count > group.items.length ? (
                    <p className="mt-2 px-1 text-xs text-muted-foreground">
                      {t("playbooks.page.targetSets.deleteBlockedRemaining", {
                        count: group.count - group.items.length,
                      })}
                    </p>
                  ) : null}
                </div>
                );
              })}
            </div>
          </div>
        ) : (
          <p className="text-xs leading-relaxed text-muted-foreground">
            {t("playbooks.page.targetSets.deleteImpact")}
          </p>
        )}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>
            {blocked ? t("close") : t("cancel")}
          </AlertDialogCancel>
          {!blocked ? (
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              disabled={pending}
              onClick={(event) => {
                event.preventDefault();
                onConfirm();
              }}
            >
              {t("delete")}
            </AlertDialogAction>
          ) : null}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
