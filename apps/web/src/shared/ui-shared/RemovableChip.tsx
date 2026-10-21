import type { ReactNode } from "react";
import { X } from "lucide-react";
import { cn } from "@/shared/lib/utils";

export interface RemovableChipProps {
  label: ReactNode;
  /** Leading icon node (already sized by the caller, typically `h-2.5 w-2.5`). */
  icon?: ReactNode;
  /** Small dimmed prefix rendered as `sublabel:` before the label. */
  sublabel?: ReactNode;
  /** When provided, renders a trailing remove (×) button. */
  onRemove?: () => void;
  removeLabel?: string;
  /** Renders the label in a monospace face (action keys, scope literals). */
  mono?: boolean;
  title?: string;
  className?: string;
  labelClassName?: string;
}

/**
 * The shared "selected value" chip with an optional leading icon,
 * an optional dimmed `sublabel:` prefix, a truncating label and an optional
 * trailing remove button. Unifies the per-form `RuleChip` / `SelectedChips`
 * variants used by the policy builder, token scopes, filters and custom fields.
 */
const RemovableChip = ({
  label,
  icon,
  sublabel,
  onRemove,
  removeLabel,
  mono,
  title,
  className,
  labelClassName,
}: RemovableChipProps) => (
  <span
    className={cn(
      "surface-chip inline-flex max-w-full items-center gap-1 rounded-md px-2 py-1 text-xs font-medium",
      className,
    )}
    title={title}
  >
    {icon ? <span className="shrink-0">{icon}</span> : null}
    {sublabel ? <span className="shrink-0 text-xs text-muted-foreground">{sublabel}:</span> : null}
    <span className={cn("min-w-0 max-w-48 truncate", mono && "font-mono", labelClassName)}>{label}</span>
    {onRemove ? (
      <button
        type="button"
        onClick={onRemove}
        aria-label={removeLabel ?? "Remove"}
        className="flex size-6 shrink-0 items-center justify-center rounded-sm text-muted-foreground transition-colors hover:bg-accent hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        <X className="size-3.5" />
      </button>
    ) : null}
  </span>
);

export default RemovableChip;
