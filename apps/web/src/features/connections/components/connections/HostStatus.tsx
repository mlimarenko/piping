import StatusChip from "@/shared/ui-shared/StatusChip";
import { cn } from "@/shared/lib/utils";
import type { ResourceStatus } from "@/shared/api/types";
import { hostStatusMap, UNAVAILABLE } from "./hostPresentation";

export function HostStatusValue({
  status,
  label,
}: {
  status?: ResourceStatus | null;
  label: string;
}) {
  if (!status) return <span className="text-xs text-muted-foreground">{UNAVAILABLE}</span>;
  return <StatusChip status={hostStatusMap[status]} label={label} />;
}

export function HostStatusDot({ status }: { status?: ResourceStatus | null }) {
  if (!status) return null;
  return (
    <div
      className={cn(
        "w-1.5 h-1.5 rounded-full shrink-0",
        status === "active" && "bg-status-positive",
        status === "planned" && "bg-status-info",
        status === "staged" && "bg-status-warning",
        status === "decommissioning" && "bg-status-negative",
        status === "offline" && "bg-muted-foreground",
      )}
    />
  );
}
