import { useMemo, useState } from "react";
import {
  useQuery,
  type QueryKey,
  type UseQueryResult,
} from "@tanstack/react-query";
import type { Paginated } from "@/shared/api/types";
import { useDebouncedValue } from "@/shared/hooks/useDebouncedValue";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import type { DataTableSortState } from "./DataTable";
import {
  buildDataTableFilterParams,
  mapDataTableSortKey,
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
  type DataTableFilterParamConfig,
  type DataTableFilterParamValue,
  type DataTableSortKeyMap,
} from "./tableState";

/**
 * Common fields every server-driven list query composes. Feature query types
 * (for example `TemplateListQuery`, `CustomFieldDefinitionListParams`) extend
 * these; the hook fills them in and hands the result to `listFn`.
 */
export interface ServerTableListParams {
  search?: string;
  sort?: string;
  order?: "asc" | "desc";
  limit?: number;
  offset?: number;
}

export interface UseServerTableOptions<TItem, TParams extends ServerTableListParams> {
  /** Domain list fetch; the hook supplies the composed `TParams`. */
  listFn: (params: TParams) => Promise<Paginated<TItem>>;
  /** Query-key prefix; the composed list params are appended. */
  queryKey: QueryKey;
  /** Allowed UI column keys (the feature's declarative table config). */
  sortKeys: readonly string[];
  /** Optional UI column→backend key mapping, applied after sanitization. */
  sortKeyByColumn?: DataTableSortKeyMap;
  /** Column→param filter mapping the backend accepts. */
  filterColumns?: readonly DataTableFilterParamConfig[];
  /** Fixed page size. Omit to use the persisted `table.page_size` preference. */
  pageSizeDefault?: number;
  /** Keep the previous page visible while refetching (avoids a list flicker). */
  keepPreviousData?: boolean;
  refetchInterval?:
    | number
    | ((query: { state: { data?: Paginated<TItem> } }) => number | false | undefined);
  staleTime?: number;
  /** Disable the list query until the feature's tab/surface is active. */
  enabled?: boolean;
}

export interface UseServerTableResult<TItem> {
  /** Sanitized sort (unknown keys dropped against `sortKeys`). */
  sortState: DataTableSortState | null;
  setSortState: (next: DataTableSortState | null) => void;
  /** Sanitized column filters (unknown keys and empty values dropped). */
  columnFilters: Record<string, string[]>;
  setColumnFilter: (key: string, values: string[]) => void;
  /** Backend filter params, serialized according to the feature's config. */
  filterParams: Record<string, DataTableFilterParamValue>;
  page: number;
  pageSize: number;
  setPage: (page: number) => void;
  setPageSize: (size: number) => void;
  /** Raw search input (drives empty-state copy, not the request). */
  search: string;
  debouncedSearch: string;
  setSearch: (value: string) => void;
  query: UseQueryResult<Paginated<TItem>>;
  refresh: () => void;
}

/**
 * Canonical owner of server-driven list state: page/pageSize, sort, column
 * filters and (debounced) search, sanitized against the table config, composed
 * into one list-params object and driven through `useQuery`.
 *
 * The four inventory list tables (`templates`, `template-runs`, custom-field
 * `definitions` and `choice-sets`) each re-implemented this same state machine
 * plus a hand-rolled debounce; this hook removes those copies so every list is
 * the same surface. API list/query/permission/parameter mapping stays in the
 * feature (passed here as `listFn` + `sortKeys` + `filterColumns`).
 */
export function useServerTable<TItem, TParams extends ServerTableListParams>(
  options: UseServerTableOptions<TItem, TParams>,
): UseServerTableResult<TItem> {
  const {
    listFn,
    queryKey,
    sortKeys,
    sortKeyByColumn,
    filterColumns = [],
    pageSizeDefault,
    keepPreviousData = false,
    refetchInterval,
    staleTime,
    enabled = true,
  } = options;

  const [page, setPageState] = useState(0);
  const [sortState, setSortStateRaw] = useState<DataTableSortState | null>(null);
  const [columnFilters, setColumnFiltersState] = useState<Record<string, string[]>>({});
  const [search, setSearchState] = useState("");

  // Persisted page size (shared `table.page_size` preference) vs a fixed size
  // for bounded widgets such as the "recent runs" section. The persisted query
  // is disabled when a fixed size is given so it cannot re-paginate the table.
  const persistedPageSize = useTablePageSize(pageSizeDefault === undefined);
  const [fixedPageSize, setFixedPageSizeState] = useState(pageSizeDefault ?? 50);
  const pageSize = pageSizeDefault === undefined ? persistedPageSize.value : fixedPageSize;

  const debouncedSearch = useDebouncedValue(search);

  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, sortKeys),
    [sortState, sortKeys],
  );
  const sanitizedColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        columnFilters,
        filterColumns.map((column) => column.colKey),
      ),
    [columnFilters, filterColumns],
  );
  const filterParams = useMemo(
    () => buildDataTableFilterParams(sanitizedColumnFilters, filterColumns),
    [sanitizedColumnFilters, filterColumns],
  );

  const setPage = (next: number) => setPageState(next);
  const setSortState = (next: DataTableSortState | null) => {
    setSortStateRaw(next);
    setPageState(0);
  };
  const setColumnFilter = (key: string, values: string[]) => {
    setColumnFiltersState((prev) => updateDataTableColumnFilters(prev, key, values));
    setPageState(0);
  };
  const setSearch = (value: string) => {
    setSearchState(value);
    setPageState(0);
  };
  const setPageSize = (size: number) => {
    if (pageSizeDefault === undefined) persistedPageSize.setValue(size);
    else setFixedPageSizeState(size);
    setPageState(0);
  };

  const listParams = useMemo<TParams>(() => {
    // The feature owns filter serialization and any API-specific nesting in
    // listFn; the shared request path retains both strings and arrays.
    const params = {
      ...filterParams,
      search: debouncedSearch.trim() || undefined,
      sort: mapDataTableSortKey(sanitizedSortState, sortKeyByColumn),
      order: sanitizedSortState?.dir,
      limit: pageSize,
      offset: page * pageSize,
    };
    return params as unknown as TParams;
  }, [filterParams, debouncedSearch, sanitizedSortState, sortKeyByColumn, pageSize, page]);

  const query = useQuery({
    queryKey: [...queryKey, listParams],
    queryFn: () => listFn(listParams),
    placeholderData: keepPreviousData ? (prev) => prev : undefined,
    refetchInterval,
    staleTime,
    enabled,
  });

  return {
    sortState: sanitizedSortState,
    setSortState,
    columnFilters: sanitizedColumnFilters,
    setColumnFilter,
    filterParams,
    page,
    pageSize,
    setPage,
    setPageSize,
    search,
    debouncedSearch,
    setSearch,
    query,
    refresh: () => {
      void query.refetch();
    },
  };
}
