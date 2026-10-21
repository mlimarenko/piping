import type { ReactNode } from "react";
import { Check } from "lucide-react";
import { cn } from "@/shared/lib/utils";

export interface TogglePillProps {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
  /** Leading icon node (sized by caller). */
  icon?: ReactNode;
  /** Leading check-circle (the bundle/pack affordance). */
  showCheck?: boolean;
  /** Trailing tabular-nums count. */
  count?: number;
  /** Pill shape: `full` (packs/presets) or `md` (segmented choices). */
  shape?: "full" | "md";
  /** Overrides the active palette (e.g. a destructive/deny choice). */
  activeClassName?: string;
  className?: string;
  disabled?: boolean;
}

/**
 * A single selectable pill button with the shared active/idle palette. The atom
 * behind every "choose from a row" control — expiry presets, scope packs,
 * allow/deny effect, scope-type segments — so they all toggle and read the same
 * way. Selection logic stays with the caller (single-select, bundle subset…).
 */
export const TogglePill = ({
  active,
  onClick,
  children,
  icon,
  showCheck,
  count,
  shape = "full",
  activeClassName,
  className,
  disabled,
}: TogglePillProps) => (
  <button
    type="button"
    aria-pressed={active}
    disabled={disabled}
    onClick={onClick}
    className={cn(
      "inline-flex min-h-9 items-center gap-2 border px-3 py-1.5 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50",
      shape === "full" ? "rounded-full" : "rounded-md",
      active
        ? (activeClassName ?? "border-primary bg-primary/10 text-primary")
        : "border-input/30 bg-card text-muted-foreground hover:bg-accent hover:text-accent-foreground",
      className,
    )}
  >
    {showCheck && (
      <span
        className={cn(
          "flex h-3.5 w-3.5 items-center justify-center rounded-full border transition-colors",
          active ? "border-primary bg-primary text-primary-foreground" : "border-input/30",
        )}
      >
        {active && <Check className="h-2.5 w-2.5" />}
      </span>
    )}
    {icon ? <span className="shrink-0">{icon}</span> : null}
    {children}
    {count !== undefined && <span className="tabular-nums opacity-60">{count}</span>}
  </button>
);

export interface TogglePillOption<TValue extends string> {
  value: TValue;
  label: ReactNode;
  icon?: ReactNode;
  count?: number;
  activeClassName?: string;
  disabled?: boolean;
}

export interface TogglePillGroupProps<TValue extends string> {
  options: readonly TogglePillOption<TValue>[];
  value: TValue;
  onValueChange: (value: TValue) => void;
  shape?: "full" | "md";
  className?: string;
}

/**
 * Single-select row of {@link TogglePill}s (segmented control / preset row).
 * For multi-select "bundle" rows where active state is derived (e.g. a pack is
 * active when its whole subset is selected), render {@link TogglePill} directly.
 */
export const TogglePillGroup = <TValue extends string>({
  options,
  value,
  onValueChange,
  shape = "md",
  className,
}: TogglePillGroupProps<TValue>) => (
  <div className={cn("flex flex-wrap gap-1.5", className)}>
    {options.map((option) => (
      <TogglePill
        key={option.value}
        active={option.value === value}
        onClick={() => onValueChange(option.value)}
        icon={option.icon}
        count={option.count}
        shape={shape}
        activeClassName={option.activeClassName}
        disabled={option.disabled}
      >
        {option.label}
      </TogglePill>
    ))}
  </div>
);
