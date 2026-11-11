import type { ReactNode } from "react";
import { Layers } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import PermissionGroupGrid, {
  type PermissionGroup,
  type PermissionGroupState,
} from "@/shared/ui-shared/PermissionGroupGrid";
import { TogglePill } from "@/shared/ui-shared/TogglePill";
import SearchInput from "@/shared/ui-shared/SearchInput";

export interface PermissionBundle {
  /** Stable key + React key. */
  id: string;
  label: ReactNode;
  /** Concrete grid values this bundle toggles on/off together. */
  values: string[];
}

export interface PermissionPickerProps {
  groups: PermissionGroup[];
  selectedValues: ReadonlySet<string>;
  onToggleValue: (value: string) => void;
  /** Batch toggle used by the per-section "select all" checkbox. */
  onToggleValues?: (values: string[], selected: boolean) => void;

  /** Pre-formatted "N selected" summary shown in the toolbar count badge. */
  countLabel: ReactNode;
  /** Drives the count badge accent + whether clear-all renders. */
  hasSelection: boolean;
  onClearAll?: () => void;
  clearAllLabel?: ReactNode;
  /** Extra toolbar controls left of clear-all (e.g. a top-level select-all). */
  toolbarActions?: ReactNode;

  /** Optional quick "pack" pills above the grid. */
  bundles?: PermissionBundle[];
  bundlesLabel?: ReactNode;
  isBundleActive?: (bundle: PermissionBundle) => boolean;
  onToggleBundle?: (bundle: PermissionBundle) => void;

  /** Optional in-section search field above the grid. */
  searchValue?: string;
  onSearchChange?: (value: string) => void;
  searchPlaceholder?: string;

  /** Per-group bulk affordances (Read/Manage, select-section/only…). */
  renderGroupActions?: (group: PermissionGroup, state: PermissionGroupState) => ReactNode;
  emptyLabel: string;
  monoItems?: boolean;

  className?: string;
  gridClassName?: string;
  groupClassName?: string;
  itemGridClassName?: string;
}

/**
 * The shared "grant permissions" surface: a count/clear toolbar, optional quick
 * packs, optional search and a grouped checklist ({@link PermissionGroupGrid}).
 * Both the Create-API-token dialog (scopes grouped by resource, role packs) and
 * the policy builder (actions grouped by category) render this exact component,
 * so the two forms cannot drift apart. Pure presentation — the catalog, the
 * selected set and all toggle logic live at the call site.
 */
const PermissionPicker = ({
  groups,
  selectedValues,
  onToggleValue,
  onToggleValues,
  countLabel,
  hasSelection,
  onClearAll,
  clearAllLabel,
  toolbarActions,
  bundles,
  bundlesLabel,
  isBundleActive,
  onToggleBundle,
  searchValue,
  onSearchChange,
  searchPlaceholder,
  renderGroupActions,
  emptyLabel,
  monoItems,
  className,
  gridClassName,
  groupClassName,
  itemGridClassName,
}: PermissionPickerProps) => (
   <div className={cn("flex min-w-0 flex-col gap-4", className)}>
    <div className="flex flex-wrap items-center gap-2">
      <span
        className={cn(
          "rounded-full px-2 py-0.5 text-xs font-medium tabular-nums",
          hasSelection ? "bg-primary/10 text-primary" : "bg-background/40 text-muted-foreground",
        )}
      >
        {countLabel}
      </span>
      <div className="ml-auto flex items-center gap-1.5">
        {toolbarActions}
        {hasSelection && onClearAll && (
          <button
            type="button"
            onClick={onClearAll}
            className="min-h-8 rounded-md px-2 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            {clearAllLabel}
          </button>
        )}
      </div>
    </div>

    {bundles && bundles.length > 0 && (
      <div className="space-y-2">
        {bundlesLabel && (
          <div className="flex items-center gap-1.5">
            <Layers className="h-3 w-3 text-muted-foreground" />
            <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
              {bundlesLabel}
            </p>
          </div>
        )}
        <div className="flex flex-wrap gap-1.5">
          {bundles.map((bundle) => (
            <TogglePill
              key={bundle.id}
              active={isBundleActive ? isBundleActive(bundle) : false}
              onClick={() => onToggleBundle?.(bundle)}
              showCheck
              count={bundle.values.length}
            >
              {bundle.label}
            </TogglePill>
          ))}
        </div>
      </div>
    )}

    {onSearchChange && (
      <SearchInput
        value={searchValue ?? ""}
        onValueChange={onSearchChange}
        placeholder={searchPlaceholder}
      />
    )}

    <PermissionGroupGrid
      groups={groups}
      selectedValues={selectedValues}
      onToggleValue={onToggleValue}
      onToggleValues={onToggleValues}
      renderHeaderActions={renderGroupActions}
      emptyLabel={emptyLabel}
      monoItems={monoItems}
      className={gridClassName}
      groupClassName={groupClassName}
      itemGridClassName={itemGridClassName}
    />
  </div>
);

export default PermissionPicker;
