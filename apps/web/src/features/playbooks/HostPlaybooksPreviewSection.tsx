import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getTargetPlaybookAssignments, TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT, type PlaybookTargetAssignment } from "./api";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { Button } from "@/shared/ui-primitives/button";

interface Props {
  host: { id: string };
  visible?: boolean;
  enabled?: boolean;
  layout?: "cards" | "buttons";
  onAvailabilityChange?: (available: boolean) => void;
  onLaunch?: (item: PlaybookTargetAssignment) => void;
}

export default function HostPlaybooksPreviewSection({ host, visible = true, enabled = true, onAvailabilityChange, onLaunch }: Props) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const query = useQuery({
    queryKey: ["playbooks", "target-assignments", host.id],
    queryFn: ({ signal }) => getTargetPlaybookAssignments(host.id, TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT, signal),
    enabled: enabled && Boolean(onLaunch),
    staleTime: 15_000,
  });
  const items = query.data?.items ?? [];
  useEffect(() => {
    if (enabled && onAvailabilityChange) onAvailabilityChange(Boolean(onLaunch) && query.isSuccess && items.length > 0);
  }, [enabled, onAvailabilityChange, onLaunch, query.isSuccess, items.length]);
  if (!visible || !enabled) return null;
  if (!onLaunch) return <p className="text-xs text-muted-foreground">{t("playbooks.hostJobs.pending", { defaultValue: "Host job assignments are not available until the job assignment API is connected." })}</p>;
  if (query.isPending) return <p className="text-xs text-muted-foreground">{t("loading")}</p>;
  if (query.isError) return <p role="alert" className="text-xs text-muted-foreground">{t("unknownError")}</p>;
  return <div className="flex flex-wrap gap-1">{items.map(item => {
    const runnable = can("EXECUTE", "PLAYBOOK_JOB_TEMPLATE") && can("EXECUTE_NATIVE", "PLAYBOOK_JOB_TEMPLATE") && item.assignment.enabled && item.assignment.executable && item.job_template.inventory.kind === "managed";
    return <Button key={item.assignment.id} size="sm" variant="outline" disabled={!runnable} title={item.job_template.inventory.kind === "native" ? t("playbooks.launchDraft.fixedTargetsConstraint", { defaultValue: "Selected hosts must belong to the job's saved managed inventory. Native inventory does not support host selection." }) : undefined} onClick={() => onLaunch(item)}>{item.assignment.name || item.job_template.name}</Button>;
  })}</div>;
}
