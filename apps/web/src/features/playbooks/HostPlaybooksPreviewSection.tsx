import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { AlertTriangle, ClipboardList, Loader2, Play } from "lucide-react";

import {
  getTargetPlaybookAssignments,
  TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT,
  type PlaybookTargetAssignment,
  type PlaybookTargetAssignments,
} from "@/features/playbooks/api";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { Button } from "@/shared/ui-primitives/button";
import { cn } from "@/shared/lib/utils";

interface HostRef {
  id: string;
}

interface HostPlaybooksPreviewSectionProps {
  host: HostRef;
  visible?: boolean;
  enabled?: boolean;
  layout?: "cards" | "buttons";
  onAvailabilityChange?: (available: boolean) => void;
  /** Called when a runnable assignment is selected. The parent owns confirmation and run creation. */
  onLaunch?: (item: PlaybookTargetAssignment) => void;
}

const RISK_STYLE: Record<string, string> = {
  low: "border-status-positive/30 bg-status-positive/10 text-status-positive",
  medium: "border-status-warning/30 bg-status-warning/10 text-status-warning",
  high: "border-status-warning/40 bg-status-warning/15 text-status-warning",
  critical: "border-status-negative/35 bg-status-negative/10 text-status-negative",
};

const RISK_BUTTON_STYLE: Record<string, string> = {
  low: "border-status-positive/35 text-status-positive hover:bg-status-positive/10",
  medium: "border-status-warning/35 text-status-warning hover:bg-status-warning/10",
  high: "border-status-warning/40 text-status-warning hover:bg-status-warning/10",
  critical: "border-status-negative/40 text-status-negative hover:bg-status-negative/10",
};

function playbookTitle(item: PlaybookTargetAssignment): string {
  return item.assignment.name || item.playbook.playbook.name;
}

function scopeLabel(item: PlaybookTargetAssignment, t: TFunction): string {
  return t(`playbooks.scope.${item.assignment.scope_kind}`, {
    defaultValue: item.assignment.scope_kind,
  });
}

const HostPlaybooksPreviewSection = ({
  host,
  visible = true,
  enabled = true,
  layout = "cards",
  onAvailabilityChange,
  onLaunch,
}: HostPlaybooksPreviewSectionProps) => {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const query = useQuery({
    queryKey: ["playbooks", "target-assignments", host.id],
    queryFn: () => getTargetPlaybookAssignments(host.id, TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT),
    enabled,
    staleTime: 15_000,
  });
  const data: PlaybookTargetAssignments | undefined = query.data;
  const items = data?.items ?? [];

  useEffect(() => {
    if (!enabled || !onAvailabilityChange) return;
    if (query.isError) {
      onAvailabilityChange(false);
      return;
    }
    if (query.isSuccess) {
      onAvailabilityChange(items.length > 0);
    }
  }, [enabled, items.length, onAvailabilityChange, query.isError, query.isSuccess]);

  if (!visible || !enabled) return null;

  if (query.isPending) {
    return (
      <div className="flex items-center gap-2 py-1 text-xs text-muted-foreground">
        <Loader2 className="h-3 w-3 animate-spin" />
        <span>{t("loading")}</span>
      </div>
    );
  }

  if (query.isError) {
    return (
      <div className="flex items-start gap-2 py-1 text-xs text-muted-foreground">
        <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0 text-status-warning" />
        <span>{query.error.message}</span>
      </div>
    );
  }

  if (items.length === 0) return null;

  if (layout === "buttons") {
    return (
      <div className="flex flex-wrap gap-1">
        {items.map((item) => {
          const risk = item.playbook.playbook.risk;
          const style = RISK_BUTTON_STYLE[risk] ?? RISK_BUTTON_STYLE.low;
          const itemTitle = playbookTitle(item);
          const runnable =
            typeof onLaunch === "function" &&
            can("EXECUTE", "PLAYBOOK") &&
            item.assignment.executable &&
            item.assignment.enabled &&
            item.playbook.playbook.status === "active" &&
            Boolean(item.playbook.current_version);
          const title = [
            itemTitle,
            t(`playbooks.risk.${risk}`, { defaultValue: risk }),
            scopeLabel(item, t),
            item.playbook.playbook.description,
          ].filter(Boolean).join(" - ");
          return (
            <span key={item.assignment.id} className="inline-flex max-w-full min-w-0" title={title}>
              <Button
                size="sm"
                variant="outline"
                className={cn(
                  "min-w-0 max-w-full gap-2 border-l-2 px-3 transition-colors",
                  style,
                )}
                disabled={!runnable}
                onClick={() => onLaunch?.(item)}
              >
                <ClipboardList className="h-3 w-3 shrink-0" />
                <span className="min-w-0 max-w-[12rem] truncate">{itemTitle}</span>
              </Button>
            </span>
          );
        })}
      </div>
    );
  }

  return (
    <div className="space-y-1.5">
      {items.map((item) => {
        const risk = item.playbook.playbook.risk;
        const style = RISK_STYLE[risk] ?? RISK_STYLE.low;
        const runnable =
          typeof onLaunch === "function" &&
          can("EXECUTE", "PLAYBOOK") &&
          item.assignment.executable &&
          item.assignment.enabled &&
          item.playbook.playbook.status === "active" &&
          Boolean(item.playbook.current_version);
        return (
          <div
            key={item.assignment.id}
            className="rounded-md border border-border bg-card px-3 py-2"
          >
            <div className="flex items-start gap-2">
              <div className="mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-md bg-muted">
                <ClipboardList className="h-3 w-3 text-primary" />
              </div>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-1.5">
                  <p className="truncate text-xs font-medium text-foreground">
                    {playbookTitle(item)}
                  </p>
                  <span
                    className={cn(
                      "shrink-0 rounded-md border px-2 py-1 text-xs font-medium",
                      style,
                    )}
                  >
                    {t(`playbooks.risk.${risk}`, { defaultValue: risk })}
                  </span>
                </div>
                {item.playbook.playbook.description && (
                  <p className="mt-0.5 line-clamp-2 text-xs leading-snug text-muted-foreground">
                    {item.playbook.playbook.description}
                  </p>
                )}
                <p className="mt-1 text-xs text-muted-foreground">{scopeLabel(item, t)}</p>
              </div>
              <Button
                size="sm"
                variant="outline"
                className="shrink-0 gap-2 px-3"
                disabled={!runnable}
                onClick={() => onLaunch?.(item)}
              >
                <Play className="h-3 w-3" />
                {t("playbooks.inspector.run")}
              </Button>
            </div>
          </div>
        );
      })}
    </div>
  );
};

export default HostPlaybooksPreviewSection;
