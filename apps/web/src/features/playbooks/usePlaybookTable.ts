import { useSearchParams } from "react-router";
import { useServerTable, type ServerTableListParams, type UseServerTableOptions } from "@/shared/ui-shared/useServerTable";
import { buildFilterParams, sanitizeDataTableColumnFilters, sanitizeDataTableSortState } from "@/shared/ui-shared/tableState";

/** Child collection state belongs to its URL; global collections retain their existing owner. */
export function usePlaybookTable<T, P extends ServerTableListParams>(
  options: UseServerTableOptions<T, P>,
  parentId?: string,
  section = "",
) {
  const [params, setParams] = useSearchParams();
  const prefix = `${section}.`;
  const columns = (options.filterColumns ?? []).filter((column) => !parentId || column.param !== "playbook_id");
  const columnFilters = sanitizeDataTableColumnFilters(Object.fromEntries(columns.map((column) => [
    column.colKey, params.getAll(`${prefix}${column.colKey}`),
  ])), columns.map((column) => column.colKey));
  const sortState = sanitizeDataTableSortState({ key: params.get(`${prefix}sort`) ?? "", dir: params.get(`${prefix}order`) === "desc" ? "desc" : "asc" }, options.sortKeys);
  const integer = (key: string, fallback: number, max: number) => {
    const value = Number(params.get(`${prefix}${key}`) ?? fallback);
    return Number.isSafeInteger(value) && value >= 0 && value <= max ? value : fallback;
  };
  const page = integer("page", 0, 1_000_000);
  const pageSize = Math.max(1, integer("size", options.pageSizeDefault ?? 100, 1000));
  const filterParams = buildFilterParams(columnFilters, columns);
  const urlParams = { ...filterParams, sort: sortState?.key, order: sortState?.dir, limit: pageSize, offset: page * pageSize, playbook_id: parentId };
  const table = useServerTable<T, P>({
    ...options,
    filterColumns: columns,
    queryKey: parentId ? [...options.queryKey, parentId, urlParams] : options.queryKey,
    listFn: (listParams) => options.listFn(parentId ? { ...listParams, ...urlParams } : listParams),
    keepPreviousData: parentId ? false : options.keepPreviousData,
  });
  const update = (values: Record<string, string[]>) => setParams((previous) => {
    const next = new URLSearchParams(previous);
    for (const [key, items] of Object.entries(values)) {
      next.delete(`${prefix}${key}`);
      items.forEach((item) => next.append(`${prefix}${key}`, item));
    }
    return next;
  });
  if (!parentId) return table;
  return { ...table, columnFilters, filterParams, sortState, page, pageSize,
    setPage: (value: number) => { void update({ page: [String(value)] }); },
    setPageSize: (value: number) => { void update({ size: [String(value)], page: [] }); },
    setSortState: (value: typeof sortState) => { void update({ sort: value ? [value.key] : [], order: value ? [value.dir] : [], page: [] }); },
    setColumnFilter: (key: string, values: string[]) => { if (columns.some((column) => column.colKey === key)) void update({ [key]: values, page: [] }); },
  };
}
