import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useVirtualizer, type VirtualItem } from "@tanstack/react-virtual";
import { AlertTriangle, RotateCcw, ChevronUp, ChevronDown, ChevronsUpDown, Check, Funnel, Settings2, X } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import SurfaceCard from "./SurfaceCard";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui-primitives/popover";
import { Skeleton } from "@/shared/ui-primitives/skeleton";
import { TablePagination, type TablePaginationProps } from "./Pagination";
import { useTableColumnVisibility } from "./useTableColumnVisibility";

const DEFAULT_NON_HIDEABLE_KEYS = ["actions"];

function assertUniqueColumnKeys<T>(columns: Column<T>[], tableId?: string) {
  if (!import.meta.env.DEV) return;

  const seen = new Set<string>();
  const duplicates = new Set<string>();
  for (const column of columns) {
    if (seen.has(column.key)) duplicates.add(column.key);
    else seen.add(column.key);
  }

  if (duplicates.size > 0) {
    const scope = tableId ? ` "${tableId}"` : "";
    throw new Error(
      `DataTable${scope} received duplicate column keys: ${Array.from(duplicates).join(", ")}`,
    );
  }
}

function assertKnownHiddenColumnKeys<T>(
  columns: Column<T>[],
  hiddenColumns: Set<string>,
  nonHideableKeys: Set<string>,
  tableId?: string,
) {
  if (!import.meta.env.DEV || hiddenColumns.size === 0) return;

  const knownKeys = new Set(columns.map((column) => column.key));
  const stale = Array.from(hiddenColumns).filter((key) => !knownKeys.has(key));
  if (stale.length > 0) {
    const scope = tableId ? ` "${tableId}"` : "";
    console.warn(`DataTable${scope} ignored stale hidden column keys: ${stale.join(", ")}`);
  }

  const locked = Array.from(hiddenColumns).filter((key) => nonHideableKeys.has(key));
  if (locked.length > 0) {
    const scope = tableId ? ` "${tableId}"` : "";
    console.warn(`DataTable${scope} ignored hidden non-hideable keys: ${locked.join(", ")}`);
  }
}

function isColumnManagedInSettings<T>(column: Column<T>): boolean {
  return !column.excludeFromSettings && !!column.label;
}

function assertColumnSettingsContract<T>(
  columns: Column<T>[],
  nonHideableKeys: Set<string>,
  tableId?: string,
) {
  if (!import.meta.env.DEV || !tableId) return;

  const invalid = columns.filter(
    (column) =>
      !isColumnManagedInSettings(column) &&
      !column.excludeFromSettings &&
      !nonHideableKeys.has(column.key),
  );
  if (invalid.length === 0) return;

  throw new Error(
    `DataTable "${tableId}" has columns missing a settings contract: ${invalid
      .map((column) => column.key)
      .join(", ")}. Columns rendered in a settings-enabled table must either define a label or set excludeFromSettings: true.`,
  );
}

function assertUniqueManagedColumnLabels<T>(columns: Column<T>[], tableId?: string) {
  if (!import.meta.env.DEV || !tableId) return;

  const seen = new Map<string, string>();
  const duplicates = new Map<string, string[]>();
  for (const column of settingsColumns(columns)) {
    const normalizedLabel = column.label.trim();
    if (!normalizedLabel) continue;

    const existingKey = seen.get(normalizedLabel);
    if (!existingKey) {
      seen.set(normalizedLabel, column.key);
      continue;
    }

    const duplicateKeys = duplicates.get(normalizedLabel) ?? [existingKey];
    duplicateKeys.push(column.key);
    duplicates.set(normalizedLabel, duplicateKeys);
  }

  if (duplicates.size === 0) return;

  throw new Error(
    `DataTable "${tableId}" has duplicate managed column labels: ${Array.from(duplicates.entries())
      .map(([label, keys]) => `${label} (${keys.join(", ")})`)
      .join("; ")}. Settings-managed columns must have unique user-facing labels.`,
  );
}

function settingsColumns<T>(columns: Column<T>[]): Column<T>[] {
  return columns.filter((column) => isColumnManagedInSettings(column));
}

function sanitizeHiddenColumns<T>(
  columns: Column<T>[],
  hiddenColumns: Set<string>,
  nonHideableKeys: Set<string>,
): Set<string> {
  const knownKeys = new Set(
    columns
      .filter((column) => isColumnManagedInSettings(column) && !nonHideableKeys.has(column.key))
      .map((column) => column.key),
  );
  return new Set(
    Array.from(hiddenColumns).filter((key) => knownKeys.has(key)),
  );
}

function errorMessage(error: unknown): string | undefined {
  if (!error) return undefined;
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  if (typeof error === "object" && "message" in error) {
    const message = error.message;
    if (typeof message === "string") return message;
  }
  return undefined;
}

function isNestedInteractiveTarget(target: EventTarget | null, currentTarget: HTMLElement): boolean {
  if (!(target instanceof Element)) return false;
  const interactiveTarget = target.closest(
    'button,a,input,select,textarea,[role="button"],[role="link"],[contenteditable="true"]',
  );
  return !!interactiveTarget && interactiveTarget !== currentTarget;
}

function renderCellValue(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    return String(value);
  }
  return "";
}

function clampPopoverContentToViewport(content: HTMLElement | null) {
  if (!content || typeof window === "undefined") return;

  const wrapper = content.parentElement;
  if (!wrapper?.hasAttribute("data-radix-popper-content-wrapper")) return;

  const margin = 8;
  const rect = wrapper.getBoundingClientRect();
  let deltaX = 0;
  let deltaY = 0;
  if (rect.right > window.innerWidth - margin) {
    deltaX = window.innerWidth - margin - rect.right;
  }
  if (rect.left + deltaX < margin) {
    deltaX += margin - (rect.left + deltaX);
  }
  if (rect.bottom > window.innerHeight - margin) {
    deltaY = window.innerHeight - margin - rect.bottom;
  }
  if (rect.top + deltaY < margin) {
    deltaY += margin - (rect.top + deltaY);
  }
  if (deltaX === 0 && deltaY === 0) return;

  const transform = wrapper.style.transform;
  const match = transform.match(/^translate\((-?\d+(?:\.\d+)?)px,\s*(-?\d+(?:\.\d+)?)px\)$/);
  if (!match) return;
  const [, x, y] = match;
  if (!x || !y) return;

  const nextX = Number.parseFloat(x) + deltaX;
  const nextY = Number.parseFloat(y) + deltaY;
  wrapper.style.transform = `translate(${nextX}px, ${nextY}px)`;
}

function schedulePopoverViewportClamp(content: HTMLElement | null) {
  clampPopoverContentToViewport(content);
  requestAnimationFrame(() => {
    clampPopoverContentToViewport(content);
    requestAnimationFrame(() => clampPopoverContentToViewport(content));
  });
}

export interface Column<T> {
  key: string;
  label: string;
  excludeFromSettings?: boolean;
  render?: (row: T) => React.ReactNode;
  className?: string;
  width?: string;
  sortable?: boolean;
  filterable?: boolean;
  getSortValue?: (row: T) => unknown;
  filterOptions?: DataTableFilterOption[];
}

export interface DataTableFilterOption {
  key?: string;
  value: string;
  label: string;
  count?: number;
}

type DataTableVirtualization =
  | boolean
  | {
      threshold?: number;
      estimateSize?: number;
      overscan?: number;
    };

interface DataTableProps<T> {
  columns: Column<T>[];
  data: T[];
  onRowClick?: (row: T, element: HTMLTableRowElement) => void;
  isRowClickable?: (row: T) => boolean;
  onRowIntent?: (row: T) => void;
  selectedRow?: T | null;
  getRowKey: (row: T) => string;
  toolbar?: React.ReactNode;
  emptyState?: React.ReactNode;
  className?: string;
  selectable?: boolean;
  selectedKeys?: Set<string>;
  onSelectionChange?: (keys: Set<string>) => void;
  footer?: React.ReactNode;
  loading?: boolean;
  error?: unknown;
  /**
   * Retry handler for the error state. Typed as possibly-async so callers can
   * hand over a query `refetch` directly (`onRetry={() => q.refetch()}`)
   * instead of discarding the promise with the `void` operator: returning it
   * satisfies no-floating-promises without the operator that
   * typescript:S3735 rejects.
   */
  onRetry?: () => void | Promise<unknown>;
  density?: "compact" | "comfortable";
  /**
   * Keeps large client-rendered tables responsive by mounting only visible rows.
   * This is intentionally a rendering-only optimization: sorting, current-page
   * selection, row click handling, and pagination contracts continue to operate
   * on the full `data` array passed by the caller.
   */
  virtualizeRows?: DataTableVirtualization;
  /** Rows currently mounted by the table, including virtualization overscan. */
  onRenderedRowsChange?: (rows: T[]) => void;
  sortState?: DataTableSortState | null;
  onSortChange?: (next: DataTableSortState | null) => void;
  columnFilters?: Record<string, string[]>;
  onColumnFilterChange?: (key: string, values: string[]) => void;
  /**
   * When provided, the table operates in server-driven pagination mode:
   * - `data` is rendered as-is (current page slice, no client sort)
   * - Sort affordances are hidden unless controlled sorting is provided
   * - A `<TablePagination>` control is rendered in the footer
   */
  pagination?: TablePaginationProps;
  /**
   * Stable identifier for this table. When set, a per-user column-visibility
   * control (the column-settings button) is rendered and hidden columns are
   * persisted under the `table.column_visibility` setting keyed by this id.
   */
  tableId?: string;
  /**
   * Column keys that can never be hidden via the column-settings picker.
   * Defaults to `"actions"` and the first data column. The last remaining
   * visible column is always protected as well (at least one stays visible).
   */
  nonHideableKeys?: string[];
}

type SortDir = "asc" | "desc" | null;
export interface DataTableSortState {
  key: string;
  dir: Exclude<SortDir, null>;
}

interface ColumnFilterMenuProps {
  tableId?: string;
  columnKey: string;
  label: string;
  options: DataTableFilterOption[];
  selectedValues: string[];
  onChange: (key: string, values: string[]) => void;
}

function ColumnFilterMenu({
  tableId,
  columnKey,
  label,
  options,
  selectedValues,
  onChange,
}: ColumnFilterMenuProps) {
  const { t } = useTranslation();
  const contentRef = useRef<HTMLDivElement | null>(null);
  const setContentRef = useCallback((node: HTMLDivElement | null) => {
    contentRef.current = node;
    if (node) schedulePopoverViewportClamp(node);
  }, []);
  const [query, setQuery] = useState("");
  const selected = useMemo(() => new Set(selectedValues), [selectedValues]);
  const optionsWithSelectedValues = useMemo(() => {
    const knownValues = new Set(options.map((option) => option.value));
    const missingSelectedOptions: DataTableFilterOption[] = selectedValues
      .filter((value) => !knownValues.has(value))
      .map((value) => ({ key: `selected:${value}`, value, label: value }));
    return missingSelectedOptions.length > 0
      ? [...missingSelectedOptions, ...options]
      : options;
  }, [options, selectedValues]);
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const filteredOptions = useMemo(
    () =>
      normalizedQuery
        ? optionsWithSelectedValues.filter((option) =>
            option.label.toLocaleLowerCase().includes(normalizedQuery),
          )
        : optionsWithSelectedValues,
    [normalizedQuery, optionsWithSelectedValues],
  );
  const selectedCount = selectedValues.length;

  useEffect(() => {
    if (contentRef.current) {
      schedulePopoverViewportClamp(contentRef.current);
    }
  }, [filteredOptions.length, selectedCount]);

  const toggleValue = (value: string) => {
    const next = new Set(selectedValues);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    onChange(columnKey, Array.from(next));
  };

  const clearFilter = () => {
    onChange(columnKey, []);
    setQuery("");
  };

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          data-column-filter-trigger="true"
          data-column-filter-table-id={tableId}
          data-column-filter-key={columnKey}
          data-column-filter-selected-count={selectedCount}
          title={t("dataTable.filterBy", {
            col: label,
            defaultValue: `Filter ${label}`,
          })}
          aria-label={t("dataTable.filterBy", {
            col: label,
            defaultValue: `Filter ${label}`,
          })}
          className={cn(
            "relative size-6 shrink-0 text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground",
            selectedCount > 0 && "bg-primary/10 text-primary opacity-100",
          )}
          onClick={(event) => event.stopPropagation()}
        >
          <Funnel className="h-3 w-3" />
          {selectedCount > 0 && (
            <span className="absolute -right-1 -top-1 flex h-3.5 min-w-3.5 items-center justify-center rounded-full bg-primary px-0.5 text-[9px] font-semibold leading-none text-primary-foreground">
              {selectedCount}
            </span>
          )}
        </Button>
      </PopoverTrigger>
      <PopoverContent
        ref={setContentRef}
        align="end"
        collisionPadding={8}
        className="z-[100] p-0 data-[state=closed]:!animate-none data-[state=open]:!animate-none"
        style={{
          width: "min(18rem, var(--radix-popper-available-width))",
          maxWidth: "calc(100vw - 1rem)",
        }}
        data-column-filter-content="true"
        data-column-filter-table-id={tableId}
        data-column-filter-key={columnKey}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          schedulePopoverViewportClamp(contentRef.current);
        }}
      >
        <div className="border-b border-border/40 px-3 py-2">
          <div className="flex items-start justify-between gap-2">
            <div className="min-w-0">
              <p className="truncate text-xs font-semibold text-foreground">
                {t("dataTable.filterTitle", {
                  col: label,
                  defaultValue: `Filter ${label}`,
                })}
              </p>
              <p className="text-xs text-muted-foreground">
                {selectedCount > 0
                  ? t("dataTable.filterSelected", {
                      count: selectedCount,
                      defaultValue: `${selectedCount} selected`,
                    })
                  : t("dataTable.filterSelectValues", {
                      defaultValue: "Select values",
                    })}
              </p>
            </div>
            {selectedCount > 0 && (
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                className="relative z-[1] h-6 w-6 text-muted-foreground"
                aria-label={t("dataTable.clearFilter", {
                  col: label,
                  defaultValue: `Clear ${label} filter`,
                })}
                data-column-filter-clear="true"
                onClick={clearFilter}
              >
                <X className="h-3.5 w-3.5" />
              </Button>
            )}
          </div>
            <SearchInput
              containerClassName="mt-2"
              value={query}
              onValueChange={setQuery}
              placeholder={t("dataTable.searchValues", {
                defaultValue: "Search values...",
              })}
              data-column-filter-search="true"
            />
        </div>
        <div className="max-h-72 overflow-y-auto stable-scrollbar-gutter p-1">
          {filteredOptions.length > 0 ? (
            filteredOptions.map((option) => {
              const checked = selected.has(option.value);
              return (
                <button
                  key={option.key ?? option.value}
                  type="button"
                  data-column-filter-option="true"
                  data-column-filter-option-value={option.value}
                  data-column-filter-option-selected={checked ? "true" : "false"}
                  className={cn(
                    "flex w-full min-w-0 items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm outline-none transition-colors hover:bg-accent/50 focus-visible:bg-accent/60",
                    checked && "bg-primary/10 text-primary",
                  )}
                  onClick={() => toggleValue(option.value)}
                >
                  <span
                    className={cn(
                      "flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border border-input/30",
                      checked && "border-primary bg-primary text-primary-foreground",
                    )}
                    aria-hidden
                  >
                    {checked && <Check className="h-3 w-3" />}
                  </span>
                  <span className="min-w-0 flex-1 truncate" title={option.label}>
                    {option.label}
                  </span>
                  {typeof option.count === "number" && (
                    <span className="shrink-0 rounded-full bg-foreground/10 px-1.5 py-0.5 text-xs tabular-nums text-muted-foreground">
                      {option.count}
                    </span>
                  )}
                </button>
              );
            })
          ) : (
            <div className="px-3 py-6 text-center text-xs text-muted-foreground">
              {t("dataTable.noFilterValues", {
                defaultValue: "No matching values",
              })}
            </div>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}

interface ColumnSettingsMenuProps<T> {
  tableId?: string;
  columns: Column<T>[];
  hiddenColumns: Set<string>;
  activeFilterKeys: Set<string>;
  activeSortKey: string | null;
  nonHideableKeys: Set<string>;
  onChange: (next: Set<string>) => void;
  onReset: () => void;
}

function ColumnSettingsMenu<T>({
  tableId,
  columns,
  hiddenColumns,
  activeFilterKeys,
  activeSortKey,
  nonHideableKeys,
  onChange,
  onReset,
}: ColumnSettingsMenuProps<T>) {
  const { t } = useTranslation();
  const contentRef = useRef<HTMLDivElement | null>(null);
  const setContentRef = useCallback((node: HTMLDivElement | null) => {
    contentRef.current = node;
    if (node) schedulePopoverViewportClamp(node);
  }, []);
  const [query, setQuery] = useState("");
  const managedColumns = useMemo(() => settingsColumns(columns), [columns]);
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const filteredColumns = useMemo(
    () =>
      normalizedQuery
        ? managedColumns.filter((col) =>
            col.label.toLocaleLowerCase().includes(normalizedQuery),
          )
        : managedColumns,
    [managedColumns, normalizedQuery],
  );
  const visibleCount = columns.filter(
    (col) =>
      isColumnManagedInSettings(col) &&
      (nonHideableKeys.has(col.key) ||
        activeFilterKeys.has(col.key) ||
        activeSortKey === col.key ||
        !hiddenColumns.has(col.key)),
  ).length;
  const hiddenCount = columns.filter(
    (col) =>
      isColumnManagedInSettings(col) &&
      !nonHideableKeys.has(col.key) &&
      !activeFilterKeys.has(col.key) &&
      activeSortKey !== col.key &&
      hiddenColumns.has(col.key),
  ).length;

  const toggleColumn = (key: string, currentlyVisible: boolean) => {
    const next = new Set(hiddenColumns);
    if (currentlyVisible) next.add(key);
    else next.delete(key);
    onChange(next);
  };

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          // Keep a settings-trigger click from bubbling to an ancestor sort
          // handler (e.g. when rendered inside a sortable header cell). The
          // guard lives on the native interactive trigger, not a wrapper.
          onClick={(event) => event.stopPropagation()}
          data-column-settings-trigger={tableId ? "true" : undefined}
          data-column-settings-table-id={tableId}
          title={t("dataTable.columnsButton", { defaultValue: "Columns" })}
          aria-label={t("dataTable.columnsButton", { defaultValue: "Columns" })}
          className={cn(
            "relative shrink-0 text-muted-foreground hover:text-foreground",
            hiddenCount > 0 && "text-primary",
          )}
        >
          <Settings2 className="h-4 w-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        ref={setContentRef}
        align="end"
        collisionPadding={8}
        className="z-[100] p-0 data-[state=closed]:!animate-none data-[state=open]:!animate-none"
        style={{
          width: "min(18rem, var(--radix-popper-available-width))",
          maxWidth: "calc(100vw - 1rem)",
        }}
        data-column-settings-content={tableId ? "true" : undefined}
        data-column-settings-table-id={tableId}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          schedulePopoverViewportClamp(contentRef.current);
        }}
      >
        <div className="border-b border-border/40 px-3 py-2">
          <div className="flex items-start justify-between gap-2">
            <div className="min-w-0">
              <p className="truncate text-xs font-semibold text-foreground">
                {t("dataTable.columnsTitle", { defaultValue: "Show columns" })}
              </p>
              <p
                className="text-xs text-muted-foreground"
                data-column-settings-visible-count={visibleCount}
                data-column-settings-total-count={managedColumns.length}
              >
                {t("dataTable.columnsVisible", {
                  count: visibleCount,
                  defaultValue: `${visibleCount} shown`,
                })}
              </p>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-8 px-2 text-xs text-muted-foreground"
              onClick={onReset}
            >
              {t("dataTable.columnsReset", { defaultValue: "Reset" })}
            </Button>
          </div>
            <SearchInput
              containerClassName="mt-2"
              value={query}
              onValueChange={setQuery}
              placeholder={t("dataTable.searchValues", {
                defaultValue: "Search values...",
              })}
            />
        </div>
        <div className="max-h-72 overflow-y-auto stable-scrollbar-gutter p-1">
          {filteredColumns.length > 0 ? (
            filteredColumns.map((col) => {
              const isLocked = nonHideableKeys.has(col.key);
              const hasActiveFilter = activeFilterKeys.has(col.key);
              const hasActiveSort = activeSortKey === col.key;
              const isVisible =
                isLocked || hasActiveFilter || hasActiveSort || !hiddenColumns.has(col.key);
              // Keep at least one column visible: the last visible column can
              // never be toggled off.
              const isLastVisible = isVisible && visibleCount <= 1;
              const disabled = isLocked || isLastVisible || hasActiveFilter || hasActiveSort;
              return (
                <button
                  key={col.key}
                  type="button"
                  disabled={disabled}
                  aria-checked={isVisible}
                  role="menuitemcheckbox"
                  data-column-settings-key={col.key}
                  data-column-settings-visible={isVisible ? "true" : "false"}
                  className={cn(
                    "flex w-full min-w-0 items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm outline-none transition-colors hover:bg-accent/50 focus-visible:bg-accent/60",
                    isVisible && "text-foreground",
                    disabled && "cursor-not-allowed opacity-70 hover:bg-transparent",
                  )}
                  onClick={() => {
                    if (disabled) return;
                    toggleColumn(col.key, isVisible);
                  }}
                >
                  <span
                    className={cn(
                      "flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border border-input/30",
                      isVisible && "border-primary bg-primary text-primary-foreground",
                    )}
                    aria-hidden
                  >
                    {isVisible && <Check className="h-3 w-3" />}
                  </span>
                  <span className="min-w-0 flex-1 truncate" title={col.label}>
                    {col.label}
                  </span>
                  {(isLocked || hasActiveFilter || hasActiveSort) && (
                    <span className="shrink-0 text-xs uppercase text-muted-foreground">
                      {hasActiveFilter
                        ? t("dataTable.columnFiltered", { defaultValue: "Filtered" })
                        : hasActiveSort
                          ? t("dataTable.columnSorted", { defaultValue: "Sorted" })
                        : t("dataTable.columnLocked", { defaultValue: "Fixed" })}
                    </span>
                  )}
                </button>
              );
            })
          ) : (
            <div className="px-3 py-6 text-center text-xs text-muted-foreground">
              {t("dataTable.noFilterValues", {
                defaultValue: "No matching values",
              })}
            </div>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function getEffectiveSort(
  controlledSort: boolean,
  sortState: DataTableSortState | null | undefined,
  sortKey: string | null,
  sortDir: SortDir,
): { key: string | null; dir: SortDir } {
  if (controlledSort) return { key: sortState?.key ?? null, dir: sortState?.dir ?? null };
  return { key: sortKey, dir: sortDir };
}

interface VirtualizationConfig {
  enabled: boolean;
  threshold: number;
  rowEstimate: number;
  overscan: number;
}

function resolveVirtualizationConfig(
  virtualizeRows: DataTableVirtualization,
  comfortable: boolean,
): VirtualizationConfig {
  const options = typeof virtualizeRows === "object" ? virtualizeRows : undefined;
  return {
    enabled: virtualizeRows !== false,
    threshold: options?.threshold ?? 300,
    rowEstimate: options?.estimateSize ?? (comfortable ? 44 : 37),
    overscan: options?.overscan ?? 12,
  };
}

function computeShouldVirtualize(
  config: VirtualizationConfig,
  showInitialSkeleton: boolean,
  hasError: boolean,
  isDataEmpty: boolean,
  hasEmptyState: boolean,
  rowCount: number,
): boolean {
  const tableBodyReady = !showInitialSkeleton && !hasError && !(isDataEmpty && hasEmptyState);
  return config.enabled && tableBodyReady && rowCount > config.threshold;
}

function resolveVirtualRowCount(shouldVirtualize: boolean, rowCount: number): number {
  return shouldVirtualize ? rowCount : 0;
}

interface VirtualRowWindow {
  rows: VirtualItem[];
  topPadding: number;
  bottomPadding: number;
}

function resolveVirtualRowWindow(
  shouldVirtualize: boolean,
  getVirtualItems: () => VirtualItem[],
  getTotalSize: () => number,
): VirtualRowWindow {
  if (!shouldVirtualize) return { rows: [], topPadding: 0, bottomPadding: 0 };
  const rows = getVirtualItems();
  if (rows.length === 0) return { rows, topPadding: 0, bottomPadding: 0 };
  const topPadding = rows[0]?.start ?? 0;
  const bottomPadding = Math.max(0, getTotalSize() - (rows.at(-1)?.end ?? 0));
  return { rows, topPadding, bottomPadding };
}

function computeAllSelected<T>(
  sortedData: T[],
  selectedKeys: Set<string> | undefined,
  getRowKey: (row: T) => string,
) {
  return sortedData.length > 0 && selectedKeys && sortedData.every((r) => selectedKeys.has(getRowKey(r)));
}

function computeTableRenderState<T>(
  loading: boolean | undefined,
  sortedData: T[],
  visibleColumnCount: number,
  selectable: boolean | undefined,
): { showInitialSkeleton: boolean; colSpan: number } {
  return {
    showInitialSkeleton: !!loading && sortedData.length === 0,
    colSpan: visibleColumnCount + (selectable ? 1 : 0),
  };
}

function renderColumnSettingsMenu<T>(
  enabled: boolean,
  props: ColumnSettingsMenuProps<T>,
): React.ReactNode {
  return enabled ? <ColumnSettingsMenu {...props} /> : null;
}

function computeSettingsInHeader(toolbar: React.ReactNode, columnSettingsMenu: React.ReactNode): boolean {
  return !toolbar && columnSettingsMenu !== null;
}

function renderToolbarRow(toolbar: React.ReactNode, columnSettingsMenu: React.ReactNode): React.ReactNode {
  if (!toolbar) return null;
  return (
    <div data-slot="table-toolbar" className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-4 py-3">
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">{toolbar}</div>
      {columnSettingsMenu && (
        <div className="flex shrink-0 items-center">
          {columnSettingsMenu}
        </div>
      )}
    </div>
  );
}

function renderFooterRow(
  pagination: TablePaginationProps | undefined,
  footer: React.ReactNode,
  comfortable: boolean,
): React.ReactNode {
  if (!footer && !pagination) return null;
  return (
    <div data-slot="table-footer" className={cn("flex shrink-0 items-center gap-2 border-t border-border px-4 py-3 text-muted-foreground", comfortable ? "text-sm" : "text-xs")}>
      {pagination ? <TablePagination {...pagination} /> : footer}
    </div>
  );
}

function nextSortState(
  currentKey: string | null,
  currentDir: SortDir,
  key: string,
): DataTableSortState | null {
  if (currentKey !== key) return { key, dir: "asc" };
  if (currentDir === "asc") return { key, dir: "desc" };
  return null;
}

function sortRows<T>(
  data: T[],
  columns: Column<T>[],
  sortKey: string,
  sortDir: Exclude<SortDir, null>,
): T[] {
  const column = columns.find((col) => col.key === sortKey);
  const norm = (v: unknown) =>
    typeof v === "string" || typeof v === "number" || typeof v === "boolean"
      ? String(v).toLowerCase()
      : "";
  return [...data].sort((a, b) => {
    const av = norm(column?.getSortValue ? column.getSortValue(a) : (a as Record<string, unknown>)[sortKey]);
    const bv = norm(column?.getSortValue ? column.getSortValue(b) : (b as Record<string, unknown>)[sortKey]);
    const cmp = av < bv ? -1 : av > bv ? 1 : 0;
    return sortDir === "asc" ? cmp : -cmp;
  });
}

function getAriaSortValue(
  isActive: boolean,
  dir: SortDir,
  isSortable: boolean,
): React.AriaAttributes["aria-sort"] {
  if (isActive) return dir === "asc" ? "ascending" : "descending";
  return isSortable ? "none" : undefined;
}

function renderSortIcon(isActive: boolean, dir: SortDir, className: string): React.ReactNode {
  if (isActive && dir === "asc") return <ChevronUp className={className} />;
  if (isActive && dir === "desc") return <ChevronDown className={className} />;
  return <ChevronsUpDown className={className} />;
}

function renderColumnFilterMenu<T>(
  col: Column<T>,
  tableId: string | undefined,
  selectedFilterValues: string[],
  onColumnFilterChange: ((key: string, values: string[]) => void) | undefined,
): React.ReactNode {
  if (
    !onColumnFilterChange ||
    col.key === "actions" ||
    !col.label ||
    col.filterable === false ||
    !col.filterOptions ||
    (col.filterOptions.length === 0 && selectedFilterValues.length === 0)
  ) {
    return null;
  }
  return (
    <ColumnFilterMenu
      tableId={tableId}
      columnKey={col.key}
      label={col.label}
      options={col.filterOptions}
      selectedValues={selectedFilterValues}
      onChange={onColumnFilterChange}
    />
  );
}

interface DataTableHeaderCellProps<T> {
  col: Column<T>;
  index: number;
  columnCount: number;
  settingsInHeader: boolean;
  serverPaginated: boolean;
  controlledSort: boolean;
  effectiveSortKey: string | null;
  effectiveSortDir: SortDir;
  comfortable: boolean;
  tableId?: string;
  onSort: (key: string) => void;
  columnFilters?: Record<string, string[]>;
  onColumnFilterChange?: (key: string, values: string[]) => void;
  columnSettingsMenu: React.ReactNode;
}

function DataTableHeaderCell<T>({
  col,
  index,
  columnCount,
  settingsInHeader,
  serverPaginated,
  controlledSort,
  effectiveSortKey,
  effectiveSortDir,
  comfortable,
  tableId,
  onSort,
  columnFilters,
  onColumnFilterChange,
  columnSettingsMenu,
}: Readonly<DataTableHeaderCellProps<T>>) {
  const { t } = useTranslation();
  const isSortable =
    (col.sortable ?? true) && col.key !== "actions" && !!col.label && (!serverPaginated || controlledSort);
  const isActive = isSortable && effectiveSortKey === col.key;
  const sortIconClassName = cn("h-3 w-3 shrink-0", isActive ? "text-primary" : "text-muted-foreground/40");
  const sortIcon = renderSortIcon(isActive, effectiveSortDir, sortIconClassName);
  const showSettingsInThisHeader = settingsInHeader && index === columnCount - 1;
  const ariaSort = getAriaSortValue(isActive, effectiveSortDir, isSortable);
  const selectedFilterValues = columnFilters?.[col.key] ?? [];

  return (
    <th
      scope="col"
      aria-sort={ariaSort}
      data-column-key={col.key}
      data-column-managed={isColumnManagedInSettings(col) ? "true" : "false"}
      className={cn(
        "text-left font-medium text-muted-foreground whitespace-nowrap select-none",
        comfortable ? "px-4 py-3 text-xs" : "px-3 py-2.5 text-xs",
        col.className
      )}
      style={col.width ? { width: col.width } : undefined}
    >
      <div
        className={cn(
          "group/header min-w-0 items-center gap-1.5",
          showSettingsInThisHeader
            ? "flex w-full justify-between"
            : "inline-flex",
        )}
      >
        {isSortable ? (
          <button
            type="button"
            onClick={() => onSort(col.key)}
            title={t("dataTable.sortBy", { col: col.label, defaultValue: `Sort by ${col.label}` })}
            data-column-sort-trigger="true"
            data-column-sort-table-id={tableId}
            data-column-sort-key={col.key}
            className="inline-flex min-w-0 items-center gap-1 hover:text-foreground transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset rounded-sm"
          >
            <span className="truncate">{col.label}</span>
            {sortIcon}
          </button>
        ) : (
          <span className="inline-flex min-w-0 items-center gap-1">
            <span className="truncate">{col.label}</span>
          </span>
        )}
        {renderColumnFilterMenu(col, tableId, selectedFilterValues, onColumnFilterChange)}
        {showSettingsInThisHeader ? (
          <span className="ml-auto flex items-center normal-case tracking-normal">
            {columnSettingsMenu}
          </span>
        ) : null}
      </div>
    </th>
  );
}

interface DataTableRowProps<T> {
  row: T;
  virtualRow?: VirtualItem;
  visibleColumns: Column<T>[];
  getRowKey: (row: T) => string;
  selectedRow?: T | null;
  selectedKeys?: Set<string>;
  onRowClick?: (row: T, element: HTMLTableRowElement) => void;
  isRowClickable?: (row: T) => boolean;
  onRowIntent?: (row: T) => void;
  selectable?: boolean;
  comfortable: boolean;
  onToggleRow: (key: string) => void;
}

function DataTableRow<T>({
  row,
  virtualRow,
  visibleColumns,
  getRowKey,
  selectedRow,
  selectedKeys,
  onRowClick,
  isRowClickable,
  onRowIntent,
  selectable,
  comfortable,
  onToggleRow,
}: Readonly<DataTableRowProps<T>>) {
  const { t } = useTranslation();
  const key = getRowKey(row);
  const isSelected = selectedRow && getRowKey(selectedRow) === key;
  const isChecked = selectedKeys?.has(key);
  const rowCanClick = !!onRowClick && (isRowClickable ? isRowClickable(row) : true);
  return (
    <tr
      key={key}
      data-index={virtualRow?.index}
      onClick={(event) => {
        if (!rowCanClick || isNestedInteractiveTarget(event.target, event.currentTarget)) return;
        onRowClick?.(row, event.currentTarget);
      }}
      onPointerEnter={() => onRowIntent?.(row)}
      onFocusCapture={() => onRowIntent?.(row)}
      {...(rowCanClick
        ? {
            role: "button",
            tabIndex: 0,
            onKeyDown: (e: React.KeyboardEvent<HTMLTableRowElement>) => {
              if (isNestedInteractiveTarget(e.target, e.currentTarget)) return;
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onRowClick?.(row, e.currentTarget);
              }
            },
          }
        : {})}
      className={cn(
        "border-b border-border/10 transition-all duration-150 group outline-none",
        rowCanClick && "cursor-pointer focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset",
        isSelected
          ? "bg-primary/6 shadow-inner"
          : "hover:bg-accent/20"
      )}
    >
      {selectable && (
        <td className="w-8 px-3 py-2" onClick={e => e.stopPropagation()}>
          <Checkbox checked={isChecked} onCheckedChange={() => onToggleRow(key)} aria-label={t("selectRow", { key })} />
        </td>
      )}
      {visibleColumns.map(col => (
        <td key={col.key} className={cn("whitespace-nowrap text-sm", comfortable ? "px-4 py-3" : "px-3 py-2.5", col.className)}>
          {col.render ? col.render(row) : renderCellValue((row as Record<string, unknown>)[col.key])}
        </td>
      ))}
    </tr>
  );
}

interface DataTableBodyProps<T> {
  showInitialSkeleton: boolean;
  error: unknown;
  data: T[];
  emptyState?: React.ReactNode;
  colSpan: number;
  shouldVirtualize: boolean;
  topPadding: number;
  bottomPadding: number;
  virtualRows: VirtualItem[];
  sortedData: T[];
  onRetry?: () => void | Promise<unknown>;
  visibleColumns: Column<T>[];
  getRowKey: (row: T) => string;
  selectedRow?: T | null;
  selectedKeys?: Set<string>;
  onRowClick?: (row: T, element: HTMLTableRowElement) => void;
  isRowClickable?: (row: T) => boolean;
  onRowIntent?: (row: T) => void;
  selectable?: boolean;
  comfortable: boolean;
  onToggleRow: (key: string) => void;
}

function DataTableBody<T>({
  showInitialSkeleton,
  error,
  data,
  emptyState,
  colSpan,
  shouldVirtualize,
  topPadding,
  bottomPadding,
  virtualRows,
  sortedData,
  onRetry,
  visibleColumns,
  getRowKey,
  selectedRow,
  selectedKeys,
  onRowClick,
  isRowClickable,
  onRowIntent,
  selectable,
  comfortable,
  onToggleRow,
}: Readonly<DataTableBodyProps<T>>) {
  const { t } = useTranslation();

  const renderRow = (row: T, virtualRow?: VirtualItem) => (
    <DataTableRow
      key={getRowKey(row)}
      row={row}
      virtualRow={virtualRow}
      visibleColumns={visibleColumns}
      getRowKey={getRowKey}
      selectedRow={selectedRow}
      selectedKeys={selectedKeys}
      onRowClick={onRowClick}
      isRowClickable={isRowClickable}
      onRowIntent={onRowIntent}
      selectable={selectable}
      comfortable={comfortable}
      onToggleRow={onToggleRow}
    />
  );

  if (showInitialSkeleton) {
    return (
      <tbody>
        {[0, 1, 2, 3, 4].map(i => (
          <tr key={`skeleton-${i}`} className="border-b border-border/10">
            <td colSpan={colSpan} className="px-3 py-2">
              <Skeleton className="h-4 w-full" />
            </td>
          </tr>
        ))}
      </tbody>
    );
  }

  if (error) {
    return (
      <tbody>
        <tr>
          <td colSpan={colSpan}>
            <div role="alert" className="sticky left-0 flex w-[100cqw] max-w-full flex-col items-center justify-center gap-3 py-12 px-4 text-center">
              <AlertTriangle className="h-7 w-7 text-status-negative" />
              <div className="space-y-1">
                <p className="text-sm font-semibold text-foreground">{t("dataTable.errorTitle")}</p>
                <p className="mx-auto max-w-xs text-xs text-muted-foreground leading-relaxed text-balance">
                  {errorMessage(error) ?? t("dataTable.errorDescription")}
                </p>
              </div>
              {onRetry && (
                <Button size="sm" variant="outline" onClick={onRetry}>
                  <RotateCcw className="h-3 w-3" />
                  {t("retry")}
                </Button>
              )}
            </div>
          </td>
        </tr>
      </tbody>
    );
  }

  if (data.length === 0 && emptyState) {
    return (
      <tbody>
        <tr>
          <td colSpan={colSpan}><div className="sticky left-0 w-[100cqw] max-w-full">{emptyState}</div></td>
        </tr>
      </tbody>
    );
  }

  if (shouldVirtualize) {
    return (
      <tbody>
        {topPadding > 0 && (
          <tr aria-hidden="true">
            <td colSpan={colSpan} style={{ height: topPadding, padding: 0, border: 0 }} />
          </tr>
        )}
        {virtualRows.map((virtualRow) => {
          const row = sortedData[virtualRow.index];
          return row ? renderRow(row, virtualRow) : null;
        })}
        {bottomPadding > 0 && (
          <tr aria-hidden="true">
            <td colSpan={colSpan} style={{ height: bottomPadding, padding: 0, border: 0 }} />
          </tr>
        )}
      </tbody>
    );
  }

  return <tbody>{sortedData.map((row) => renderRow(row))}</tbody>;
}

function DataTable<T>({ columns, data, onRowClick, isRowClickable, onRowIntent, selectedRow, getRowKey, toolbar, emptyState, className, selectable, selectedKeys, onSelectionChange, footer, loading, error, onRetry, density = "compact", virtualizeRows = true, onRenderedRowsChange, sortState, onSortChange, columnFilters, onColumnFilterChange, pagination, tableId, nonHideableKeys }: Readonly<DataTableProps<T>>) {
  assertUniqueColumnKeys(columns, tableId);

  const { t } = useTranslation();
  const [sortKey, setSortKey] = useState<string | null>(null);
  const [sortDir, setSortDir] = useState<SortDir>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const comfortable = density === "comfortable";

  // Per-user column visibility. The hook is always called (Rules of Hooks) but
  // stays inert unless `tableId` is provided; only the button is gated on it.
  const columnVisibility = useTableColumnVisibility(tableId ?? "");
  const columnVisibilityEnabled = !!tableId;
  const rawHiddenColumns = columnVisibility.hiddenColumns;
  const nonHideableSet = useMemo(() => {
    const keys = nonHideableKeys ?? [...DEFAULT_NON_HIDEABLE_KEYS, columns[0]?.key].filter(
      (key): key is string => typeof key === "string",
    );
    return new Set(keys);
  }, [columns, nonHideableKeys]);
  assertColumnSettingsContract(columns, nonHideableSet, tableId);
  assertUniqueManagedColumnLabels(columns, tableId);
  assertKnownHiddenColumnKeys(columns, rawHiddenColumns, nonHideableSet, tableId);
  const hiddenColumns = useMemo(
    () => sanitizeHiddenColumns(columns, rawHiddenColumns, nonHideableSet),
    [columns, nonHideableSet, rawHiddenColumns],
  );
  const activeFilterKeys = useMemo(
    () =>
      new Set(
        Object.entries(columnFilters ?? {}).flatMap(([key, values]) =>
          values.length > 0 ? [key] : [],
        ),
      ),
    [columnFilters],
  );

  // When pagination is active, client-side sort is suppressed (sorting would
  // only reorder the current page, not the full dataset). Callers that already
  // sort a full data set can pass controlled sorting and still render paged data.
  const serverPaginated = !!pagination;
  const controlledSort = !!onSortChange;
  const effectiveSort = getEffectiveSort(controlledSort, sortState, sortKey, sortDir);
  const effectiveSortKey = effectiveSort.key;
  const effectiveSortDir = effectiveSort.dir;

  const visibleColumns = useMemo(
    () =>
      columnVisibilityEnabled
        ? columns.filter(
            (col) =>
              nonHideableSet.has(col.key) ||
              activeFilterKeys.has(col.key) ||
              effectiveSortKey === col.key ||
              !hiddenColumns.has(col.key),
          )
        : columns,
    [
      activeFilterKeys,
      columnVisibilityEnabled,
      columns,
      effectiveSortKey,
      hiddenColumns,
      nonHideableSet,
    ],
  );
  const visibleColumnSignature = useMemo(
    () => visibleColumns.map((column) => column.key).join("\u0000"),
    [visibleColumns],
  );

  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollLeft = 0;
  }, [tableId, visibleColumnSignature]);

  const cycleSort = (key: string) => {
    if (serverPaginated && !controlledSort) return;
    const next = nextSortState(effectiveSortKey, effectiveSortDir, key);
    if (controlledSort) onSortChange(next);
    else {
      setSortKey(next?.key ?? null);
      setSortDir(next?.dir ?? null);
    }
  };

  const sortedData = useMemo(() => {
    if (serverPaginated || controlledSort || !effectiveSortKey || !effectiveSortDir) return data;
    return sortRows(data, columns, effectiveSortKey, effectiveSortDir);
  }, [columns, controlledSort, data, effectiveSortDir, effectiveSortKey, serverPaginated]);

  const allSelected = computeAllSelected(sortedData, selectedKeys, getRowKey);
  const { showInitialSkeleton, colSpan } = computeTableRenderState(
    loading,
    sortedData,
    visibleColumns.length,
    selectable,
  );

  const virtualization = resolveVirtualizationConfig(virtualizeRows, comfortable);
  const shouldVirtualize = computeShouldVirtualize(
    virtualization,
    showInitialSkeleton,
    !!error,
    data.length === 0,
    !!emptyState,
    sortedData.length,
  );
  const getVirtualRowKey = useCallback(
    (index: number) => {
      const row = sortedData[index];
      return row ? getRowKey(row) : `missing:${index}`;
    },
    [getRowKey, sortedData],
  );

  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual owns imperative scroll measurements outside React's memo model.
  const rowVirtualizer = useVirtualizer({
    count: resolveVirtualRowCount(shouldVirtualize, sortedData.length),
    getScrollElement: () => scrollRef.current,
    estimateSize: () => virtualization.rowEstimate,
    getItemKey: getVirtualRowKey,
    overscan: virtualization.overscan,
  });
  const { rows: virtualRows, topPadding, bottomPadding } = resolveVirtualRowWindow(
    shouldVirtualize,
    () => rowVirtualizer.getVirtualItems(),
    () => rowVirtualizer.getTotalSize(),
  );
  const renderedRows = useMemo(
    () => shouldVirtualize ? virtualRows.map((row) => sortedData[row.index]).filter((row) => row !== undefined) : sortedData,
    [shouldVirtualize, sortedData, virtualRows],
  );
  useEffect(() => {
    onRenderedRowsChange?.(renderedRows);
  }, [onRenderedRowsChange, renderedRows]);

  const toggleAll = () => {
    if (!onSelectionChange) return;
    onSelectionChange(allSelected ? new Set() : new Set(sortedData.map(r => getRowKey(r))));
  };

  const toggleRow = (key: string) => {
    if (!onSelectionChange || !selectedKeys) return;
    const next = new Set(selectedKeys);
    if (next.has(key)) next.delete(key); else next.add(key);
    onSelectionChange(next);
  };

  const columnSettingsMenu = renderColumnSettingsMenu(columnVisibilityEnabled, {
    tableId,
    columns,
    hiddenColumns,
    activeFilterKeys,
    activeSortKey: effectiveSortKey,
    nonHideableKeys: nonHideableSet,
    onChange: columnVisibility.setHidden,
    onReset: columnVisibility.reset,
  });
  const settingsInHeader = computeSettingsInHeader(toolbar, columnSettingsMenu);

  return (
    <SurfaceCard
      className={cn("flex min-w-0 flex-col overflow-hidden", className)}
      data-table-id={tableId}
    >
      {renderToolbarRow(toolbar, columnSettingsMenu)}
      <div
        ref={scrollRef}
        data-table-scroll-viewport="true"
        className="min-w-0 overflow-auto stable-scrollbar-gutter flex-1 [container-type:inline-size]"
      >
        <table className="w-full text-sm">
          <thead className="table-header-surface sticky top-0 z-20">
            <tr className="border-b border-border/40">
              {selectable && (
                <th scope="col" className="w-8 px-3 py-2.5">
                  <Checkbox checked={allSelected} onCheckedChange={toggleAll} aria-label={t("selectAll")} />
                </th>
              )}
              {visibleColumns.map((col, index) => (
                <DataTableHeaderCell
                  key={col.key}
                  col={col}
                  index={index}
                  columnCount={visibleColumns.length}
                  settingsInHeader={settingsInHeader}
                  serverPaginated={serverPaginated}
                  controlledSort={controlledSort}
                  effectiveSortKey={effectiveSortKey}
                  effectiveSortDir={effectiveSortDir}
                  comfortable={comfortable}
                  tableId={tableId}
                  onSort={cycleSort}
                  columnFilters={columnFilters}
                  onColumnFilterChange={onColumnFilterChange}
                  columnSettingsMenu={columnSettingsMenu}
                />
              ))}
            </tr>
          </thead>
          <DataTableBody
            showInitialSkeleton={showInitialSkeleton}
            error={error}
            data={data}
            emptyState={emptyState}
            colSpan={colSpan}
            shouldVirtualize={shouldVirtualize}
            topPadding={topPadding}
            bottomPadding={bottomPadding}
            virtualRows={virtualRows}
            sortedData={sortedData}
            onRetry={onRetry}
            visibleColumns={visibleColumns}
            getRowKey={getRowKey}
            selectedRow={selectedRow}
            selectedKeys={selectedKeys}
            onRowClick={onRowClick}
            isRowClickable={isRowClickable}
            onRowIntent={onRowIntent}
            selectable={selectable}
            comfortable={comfortable}
            onToggleRow={toggleRow}
          />
        </table>
      </div>
      {renderFooterRow(pagination, footer, comfortable)}
    </SurfaceCard>
  );
}

export default DataTable;
