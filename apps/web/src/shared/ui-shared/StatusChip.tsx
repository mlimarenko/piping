import { cn } from "@/shared/lib/utils";

interface StatusChipProps {
  status: "info" | "positive" | "warning" | "negative" | "neutral";
  label: string;
  dot?: boolean;
  className?: string;
}

const StatusChip = ({ status, label, dot = true, className }: StatusChipProps) => {
  const statusClass = {
    info: "status-chip status-info",
    positive: "status-chip status-positive",
    warning: "status-chip status-warning",
    negative: "status-chip status-negative",
    neutral: "status-chip surface-chip text-muted-foreground",
  }[status];

  const dotColor = {
    info: "bg-status-info",
    positive: "bg-status-positive",
    warning: "bg-status-warning",
    negative: "bg-status-negative",
    neutral: "bg-muted-foreground",
  }[status];

  return (
    <span className={cn(statusClass, className)}>
      {dot && <span className={cn("w-1.5 h-1.5 rounded-full", dotColor)} />}
      {label}
    </span>
  );
};

export default StatusChip;
