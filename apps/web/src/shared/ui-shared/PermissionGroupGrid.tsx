import type { ReactNode } from "react";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { cn } from "@/shared/lib/utils";

export interface PermissionGroupItem {
  value: string;
  label?: string;
  title?: string;
  /** Optional raw/technical token (e.g. the wire scope key `read`) shown as a
   *  small muted monospace suffix after the friendly `label`, so the picker
   *  reads plainly while still exposing the exact identifier a script/MCP
   *  config needs. */
  hint?: string;
  /** Optional human description, surfaced as the option's hover tooltip. Takes
   *  precedence over `title` for enabled items so callers can pass a rich
   *  "Label — what granting this permits" string without changing the label. */
  description?: string;
  disabled?: boolean;
  disabledReason?: string;
}

export interface PermissionGroup {
  key: string;
  title: string;
  subtitle?: string;
  /** Optional domain section this group belongs to (e.g. "Connections"). When
   *  any group carries a section, the grid renders section bands with a
   *  "select the whole section" checkbox; when none do, it stays a flat grid. */
  section?: string;
  items: PermissionGroupItem[];
}

export interface PermissionGroupState {
  selectedCount: number;
  totalCount: number;
  allSelected: boolean;
}

interface PermissionGroupGridProps {
  groups: PermissionGroup[];
  selectedValues: ReadonlySet<string>;
  onToggleValue: (value: string) => void;
  /** Batch toggle for the section "select all" checkbox. When omitted, the grid
   *  falls back to calling `onToggleValue` per value that needs flipping. */
  onToggleValues?: (values: string[], selected: boolean) => void;
  renderHeaderActions?: (group: PermissionGroup, state: PermissionGroupState) => ReactNode;
  emptyLabel: string;
  className?: string;
  groupClassName?: string;
  itemGridClassName?: string;
  itemLabelClassName?: string;
  monoItems?: boolean;
}

const PermissionGroupGrid = ({
  groups,
  selectedValues,
  onToggleValue,
  onToggleValues,
  renderHeaderActions,
  emptyLabel,
  className,
  groupClassName,
  itemGridClassName,
  itemLabelClassName,
  monoItems,
}: PermissionGroupGridProps) => {
  if (groups.length === 0) {
    return (
      <div className="rounded-lg border border-border/35 bg-background/25 p-6 text-center text-xs text-muted-foreground">
        {emptyLabel}
      </div>
    );
  }

  // Both consumers live inside ~768px dialogs, so the defaults are sized for
  // that CONTAINER: at most two group cards per row, and one full-width option
  // row per item. Viewport breakpoints wider than `sm` are deliberately not
  // used — a fixed-width dialog does not get wider on a bigger monitor, and
  // viewport-driven 3-column layouts inside it were exactly what truncated
  // labels down to unreadable stubs.
  const gridClass = className ?? "grid grid-cols-1 gap-2 sm:grid-cols-2";

  const renderGroupCard = (group: PermissionGroup) => {
    const selectedCount = group.items.filter((item) => selectedValues.has(item.value)).length;
    const state: PermissionGroupState = {
      selectedCount,
      totalCount: group.items.length,
      allSelected: selectedCount === group.items.length && group.items.length > 0,
    };

    return (
      <div
        key={group.key}
        className={cn(
          "rounded-lg border p-2 transition-colors",
          selectedCount > 0 ? "border-primary/35 bg-primary/5" : "border-border/35 bg-background/25",
          groupClassName,
        )}
      >
        <div className="mb-1.5 flex items-start justify-between gap-2">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <span
                className={cn(
                  "flex h-5 w-5 shrink-0 items-center justify-center rounded-full border text-xs font-semibold",
                  state.allSelected
                    ? "border-primary bg-primary text-primary-foreground"
                    : selectedCount > 0
                      ? "border-primary/60 text-primary"
                      : "border-border/50 text-muted-foreground",
                )}
              >
                {selectedCount}
              </span>
              <p className="truncate text-xs font-semibold text-foreground">{group.title}</p>
            </div>
            {group.subtitle && (
              <p className="mt-0.5 text-xs text-muted-foreground">{group.subtitle}</p>
            )}
          </div>
          {renderHeaderActions && (
            <div className="flex shrink-0 items-center gap-1">{renderHeaderActions(group, state)}</div>
          )}
        </div>
        <div className={cn("grid grid-cols-1 gap-1", itemGridClassName)}>
          {group.items.map((item) => {
            const checked = selectedValues.has(item.value);
            const disabled = item.disabled === true;
            const inactiveClass = disabled
              ? "border-border/20 bg-muted/20 text-muted-foreground"
              : "border-input/30 bg-card/25 text-muted-foreground hover:border-input/60 hover:text-foreground";
            const stateClass =
              checked && !disabled ? "border-primary/45 bg-primary/10 text-primary" : inactiveClass;
            return (
              <label
                key={item.value}
                title={
                  disabled
                    ? (item.disabledReason ?? item.title ?? item.label ?? item.value)
                    : (item.description ?? item.title ?? item.label ?? item.value)
                }
                className={cn(
                  "flex items-center gap-1.5 rounded-md border px-1.5 py-0.5 transition-colors",
                  disabled && "cursor-not-allowed opacity-55",
                  !disabled && "cursor-pointer",
                  stateClass,
                )}
              >
                <Checkbox
                  checked={checked}
                  disabled={disabled}
                  onCheckedChange={disabled ? undefined : () => onToggleValue(item.value)}
                  className="h-3.5 w-3.5 shrink-0"
                />
                <span
                  className={cn(
                    "flex min-w-0 items-baseline gap-1 truncate text-xs",
                    monoItems && item.hint === undefined && "font-mono",
                    itemLabelClassName,
                  )}
                >
                  <span className="truncate">{item.label ?? item.value}</span>
                  {item.hint !== undefined && item.hint !== (item.label ?? item.value) && (
                    <span className="shrink-0 font-mono text-xs lowercase text-muted-foreground/60">
                      {item.hint}
                    </span>
                  )}
                </span>
              </label>
            );
          })}
        </div>
      </div>
    );
  };

  // Flat grid when no group declares a section (backward compatible).
  if (!groups.some((group) => group.section !== undefined)) {
    return <div className={gridClass}>{groups.map(renderGroupCard)}</div>;
  }

  // Otherwise band the groups into domain sections (first-appearance order) with
  // a "select the whole section" checkbox.
  const sectionOrder: string[] = [];
  const bySection = new Map<string, PermissionGroup[]>();
  for (const group of groups) {
    const section = group.section ?? "";
    if (!bySection.has(section)) {
      bySection.set(section, []);
      sectionOrder.push(section);
    }
    bySection.get(section)!.push(group);
  }

  const applySection = (values: string[], selected: boolean) => {
    if (values.length === 0) return;
    if (onToggleValues) {
      onToggleValues(values, selected);
      return;
    }
    for (const value of values) {
      const has = selectedValues.has(value);
      if (selected !== has) onToggleValue(value);
    }
  };

  return (
    <div className="space-y-4">
      {sectionOrder.map((section) => {
        const sectionGroups = bySection.get(section)!;
        // Only enabled items participate in the section toggle.
        const toggleable = sectionGroups.flatMap((group) =>
          group.items.filter((item) => item.disabled !== true).map((item) => item.value),
        );
        const selectedInSection = toggleable.filter((value) => selectedValues.has(value)).length;
        const allSelected = toggleable.length > 0 && selectedInSection === toggleable.length;
        const partialState: boolean | "indeterminate" = selectedInSection > 0 ? "indeterminate" : false;
        const checkedState: boolean | "indeterminate" = allSelected ? true : partialState;

        return (
          <div key={section || "_"} className="space-y-2">
            {section && (
              <label className="flex cursor-pointer items-center gap-2 border-b border-border/25 pb-1.5">
                <Checkbox
                  checked={checkedState}
                  disabled={toggleable.length === 0}
                  onCheckedChange={() => applySection(toggleable, !allSelected)}
                  className="h-3.5 w-3.5 shrink-0"
                />
                <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  {section}
                </span>
                <span className="text-xs tabular-nums text-muted-foreground/70">
                  {selectedInSection}/{toggleable.length}
                </span>
              </label>
            )}
            <div className={gridClass}>{sectionGroups.map(renderGroupCard)}</div>
          </div>
        );
      })}
    </div>
  );
};

export default PermissionGroupGrid;
