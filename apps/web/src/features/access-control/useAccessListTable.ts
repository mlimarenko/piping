import { useState } from "react";
import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import type { DataTableFilterOption } from "@/shared/ui-shared/DataTable";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import { useDebouncedValue } from "@/shared/hooks/useDebouncedValue";
import { useAccessTableControls, type AccessTableControls } from "@/features/access-control/useAccessTableControls";
import type { AccessFilterContext, AccessTableConfig } from "@/features/access-control/tableConfig";
import type { AccessListQuery } from "@/features/access-control/api";
import type { Paginated } from "@/shared/api/types";

export interface AccessListTableArgs<T> {
  /** Sort/filter consumer contract for this table (see `tableConfig.ts`). */
  config: AccessTableConfig;
  /** Base TanStack Query key (e.g. `USERS_QUERY_KEY` or `["access", "groups"]`).
   *  The paginated key is derived under it so `invalidateQueries` prefix-matches. */
  queryKeyPrefix: readonly unknown[];
  /** The paginated list endpoint; called with limit/offset/search/sort/order/filters. */
  listFn: (query: AccessListQuery) => Promise<Paginated<T>>;
  /** colKey -> resolved FK options (only `type: "fk"` columns need them). */
  fkOptions?: Record<string, DataTableFilterOption[]>;
  /** Dynamic context for custom filter routers (grants `subject` split). */
  filterContext?: AccessFilterContext;
}

export interface AccessListTable<T> {
  page: number;
  pageSize: number;
  setPageSize: (next: number | ((prev: number) => number)) => void;
  search: string;
  /** Search input handler: sets the raw value and resets to page 0. */
  handleSearchChange: (next: string) => void;
  /** Sort + per-column filter controls (see `useAccessTableControls`). */
  controls: AccessTableControls;
  query: UseQueryResult<Paginated<T>>;
  items: T[];
  total: number;
  handlePageChange: (page: number) => void;
  handlePageSizeChange: (size: number) => void;
}

/**
 * The shared list scaffold for every access-control directory table: page /
 * page-size / debounced search state, the sort+filter controls, and the
 * paginated query wiring. Each table used to hold this identical ~30-line block
 * (plus its own copy of the 250 ms debounce); this hook is the one owner.
 *
 * Search is debounced through `shared/hooks/useDebouncedValue` (no hand-rolled
 * timer), and the page resets as the search value changes so typing produces one
 * request instead of one per keystroke.
 */
export function useAccessListTable<T>(args: AccessListTableArgs<T>): AccessListTable<T> {
  const { config, queryKeyPrefix, listFn, fkOptions, filterContext } = args;
  const [page, setPage] = useState(0);
  const { value: pageSize, setValue: setPageSize } = useTablePageSize();
  const [search, setSearch] = useState("");
  const debouncedSearch = useDebouncedValue(search);

  const controls = useAccessTableControls(config, {
    onReset: () => setPage(0),
    fkOptions,
    filterContext,
  });

  const query = useQuery({
    queryKey: [
      ...queryKeyPrefix,
      "paginated",
      { page, pageSize, search: debouncedSearch },
      { sort: controls.sortParam, order: controls.orderParam, filters: controls.filterParams },
    ],
    queryFn: () =>
      listFn({
        limit: pageSize,
        offset: page * pageSize,
        search: debouncedSearch || undefined,
        sort: controls.sortParam,
        order: controls.orderParam,
        filters: controls.filterParams,
      }),
    placeholderData: (previous) => previous,
  });

  return {
    page,
    pageSize,
    setPageSize,
    search,
    controls,
    query,
    items: query.data?.items ?? [],
    total: query.data?.total ?? 0,
    handlePageChange: (next) => setPage(next),
    handlePageSizeChange: (size) => {
      setPageSize(size);
      setPage(0);
    },
    handleSearchChange: (next) => {
      setSearch(next);
      setPage(0);
    },
  };
}
