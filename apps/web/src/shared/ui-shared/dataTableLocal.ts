import type { Column, DataTableFilterOption, DataTableSortState } from "./DataTable";

export function boolFilterOptions(labels: {
  trueLabel: string;
  falseLabel: string;
}): DataTableFilterOption[] {
  return [
    { value: "true", label: labels.trueLabel },
    { value: "false", label: labels.falseLabel },
  ];
}

export function applyLocalColumnFilters<T>(
  rows: readonly T[],
  filters: Record<string, string[]>,
  getValue: (row: T, key: string) => string | null | undefined,
): T[] {
  const active = Object.entries(filters).filter(([, values]) => values.length > 0);
  if (active.length === 0) return [...rows];
  return rows.filter((row) =>
    active.every(([key, values]) => {
      const value = getValue(row, key);
      return value != null && values.includes(value);
    }),
  );
}

export function applyLocalSort<T>(
  rows: readonly T[],
  columns: readonly Column<T>[],
  sortState: DataTableSortState | null,
): T[] {
  if (!sortState) return [...rows];
  const column = columns.find((candidate) => candidate.key === sortState.key);
  const norm = (value: unknown) => {
    if (typeof value === "number") return value;
    if (typeof value === "boolean") return value ? 1 : 0;
    if (typeof value === "string") return value.toLowerCase();
    return "";
  };
  return [...rows].sort((left, right) => {
    const leftValue = norm(
      column?.getSortValue
        ? column.getSortValue(left)
        : (left as Record<string, unknown>)[sortState.key],
    );
    const rightValue = norm(
      column?.getSortValue
        ? column.getSortValue(right)
        : (right as Record<string, unknown>)[sortState.key],
    );
    const cmp = leftValue < rightValue ? -1 : leftValue > rightValue ? 1 : 0;
    return sortState.dir === "asc" ? cmp : -cmp;
  });
}
