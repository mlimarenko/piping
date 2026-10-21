import { normalizeTablePageSize, useSetting } from "@/shared/settings";

interface TablePageSizePreference {
  value: number;
  setValue: (next: number | ((prev: number) => number)) => void;
  isLoading: boolean;
}

/**
 * Persisted page-size preference for data tables.
 * Backed by the canonical settings service.
 * Default: 50. Pass `enabled: false` when a table uses a fixed page size
 * (for example a bounded "recent runs" widget) so the shared preference is not
 * fetched and cannot silently re-paginate it.
 */
export function useTablePageSize(enabled: boolean = true): TablePageSizePreference {
  return useSetting<number>("table.page_size", 50, {
    normalize: normalizeTablePageSize,
    enabled,
  });
}
