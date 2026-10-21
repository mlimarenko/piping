import type { Paginated } from "@/shared/api/types";

/**
 * Fetch every page of a paginated endpoint and return the flattened items.
 *
 * `fetchPage(limit, offset)` returns a `Paginated<T>`; the loop advances by the
 * number of items actually returned and stops once it has collected `total`
 * items (or a page comes back empty, which guards against a stale/incorrect
 * `total`). Pass the endpoint's preferred `pageSize` — these are "load
 * everything into a cache" reads, so a larger page means fewer round trips.
 *
 * This is the one paginate-to-completion helper; do not re-inline the loop.
 */
export async function collectAllPages<T>(
  fetchPage: (limit: number, offset: number) => Promise<Paginated<T>>,
  pageSize: number,
): Promise<T[]> {
  const items: T[] = [];
  let offset = 0;
  let total: number | null = null;
  while (total === null || offset < total) {
    const page = await fetchPage(pageSize, offset);
    items.push(...page.items);
    total = page.total;
    offset += page.items.length;
    if (page.items.length === 0) break;
  }
  return items;
}
