import type { ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useServerTable, type ServerTableListParams } from "./useServerTable";
import type { Paginated } from "@/shared/api/types";

interface Item {
  id: string;
  name: string;
}

interface ItemParams extends ServerTableListParams {
  status?: string;
}

const mocks = vi.hoisted(() => ({
  pageSize: { value: 25, setValue: vi.fn(), isLoading: false },
  listFn: vi.fn(),
}));

vi.mock("@/shared/hooks/useTablePageSize", () => ({
  useTablePageSize: () => mocks.pageSize,
}));

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

function emptyPage(limit: number, offset: number): Paginated<Item> {
  return { items: [], total: 0, limit, offset };
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe("useServerTable", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("composes pagination, sort and CSV filter params from the shared config", async () => {
    mocks.listFn.mockResolvedValue(emptyPage(25, 0));
    const { result } = renderHook(
      () =>
        useServerTable<Item, ItemParams>({
          listFn: mocks.listFn,
          queryKey: ["items"],
          sortKeys: ["name", "status"],
          filterColumns: [
            { colKey: "status", param: "status", serialize: (values) => values.join(",") },
          ],
        }),
      { wrapper },
    );

    await waitFor(() => expect(mocks.listFn).toHaveBeenCalled());
    expect(mocks.listFn).toHaveBeenCalledWith(expect.objectContaining({ limit: 25, offset: 0 }));

    act(() => result.current.setSortState({ key: "name", dir: "asc" }));
    act(() => result.current.setColumnFilter("status", ["active", "draft"]));
    await flush();

    expect(mocks.listFn).toHaveBeenLastCalledWith(
      expect.objectContaining({
        sort: "name",
        order: "asc",
        status: "active,draft",
        limit: 25,
        offset: 0,
      }),
    );
  });

  it("uses a fixed page size and never touches the persisted preference when given", async () => {
    mocks.listFn.mockResolvedValue(emptyPage(8, 0));
    const { result } = renderHook(
      () =>
        useServerTable<Item, ItemParams>({
          listFn: mocks.listFn,
          queryKey: ["items"],
          sortKeys: ["name"],
          pageSizeDefault: 8,
        }),
      { wrapper },
    );

    await waitFor(() => expect(mocks.listFn).toHaveBeenCalled());
    expect(mocks.listFn).toHaveBeenCalledWith(expect.objectContaining({ limit: 8, offset: 0 }));

    act(() => result.current.setPageSize(16));
    await flush();
    expect(mocks.listFn).toHaveBeenLastCalledWith(
      expect.objectContaining({ limit: 16, offset: 0 }),
    );
    expect(mocks.pageSize.setValue).not.toHaveBeenCalled();
  });

  it("maps UI sort keys, retains configured arrays and CSV, and sanitizes unknown columns", async () => {
    mocks.listFn.mockResolvedValue(emptyPage(25, 0));
    const { result } = renderHook(() => useServerTable<Item, ItemParams>({
      listFn: mocks.listFn,
      queryKey: ["mapped-items"],
      sortKeys: ["lastActivity"],
      sortKeyByColumn: { lastActivity: "last_activity", unknown: "unsafe" },
      filterColumns: [
        { colKey: "owners", param: "owner_ids" },
        { colKey: "status", param: "status", serialize: values => values.join(",") },
      ],
    }), { wrapper });
    await flush();
    act(() => result.current.setPage(2));
    await flush();
    expect(mocks.listFn).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 50 }));
    act(() => result.current.setSortState({ key: "lastActivity", dir: "desc" }));
    await flush();
    expect(mocks.listFn).toHaveBeenLastCalledWith(expect.objectContaining({ sort: "last_activity", order: "desc", offset: 0 }));
    act(() => result.current.setPage(3));
    await flush();
    act(() => {
      result.current.setColumnFilter("owners", ["one", "two", "one", ""]);
      result.current.setColumnFilter("status", ["active", "draft"]);
      result.current.setColumnFilter("unknown", ["unsafe"]);
    });
    await flush();
    expect(mocks.listFn).toHaveBeenLastCalledWith(expect.objectContaining({
      owner_ids: ["one", "two"], status: "active,draft", offset: 0,
    }));
    expect(result.current.filterParams).toEqual({ owner_ids: ["one", "two"], status: "active,draft" });
    act(() => result.current.setSortState({ key: "unknown", dir: "asc" }));
    await flush();
    expect(result.current.sortState).toBeNull();
    expect(result.current.columnFilters).not.toHaveProperty("unknown");
    expect(mocks.listFn).toHaveBeenLastCalledWith(expect.objectContaining({ sort: undefined, order: undefined }));
    expect(mocks.listFn.mock.lastCall?.[0]).not.toHaveProperty("unknown");
  });

  it("resets to the first page on sort, filter and search changes", async () => {
    mocks.listFn.mockResolvedValue(emptyPage(25, 0));
    const { result } = renderHook(
      () =>
        useServerTable<Item, ItemParams>({
          listFn: mocks.listFn,
          queryKey: ["items"],
          sortKeys: ["name", "status"],
          filterColumns: [
            { colKey: "status", param: "status", serialize: (values) => values.join(",") },
          ],
        }),
      { wrapper },
    );

    await waitFor(() => expect(mocks.listFn).toHaveBeenCalled());

    act(() => result.current.setPage(2));
    await flush();
    expect(mocks.listFn).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 50 }));

    act(() => result.current.setSortState({ key: "status", dir: "desc" }));
    await flush();
    expect(mocks.listFn).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 0 }));

    act(() => result.current.setPage(3));
    await flush();
    act(() => result.current.setColumnFilter("status", ["active"]));
    await flush();
    expect(mocks.listFn).toHaveBeenLastCalledWith(
      expect.objectContaining({ offset: 0, status: "active" }),
    );
  });

  it("does not call the list function while disabled", async () => {
    mocks.listFn.mockResolvedValue(emptyPage(25, 0));
    const { result } = renderHook(
      () =>
        useServerTable<Item, ItemParams>({
          listFn: mocks.listFn,
          queryKey: ["items"],
          sortKeys: ["name"],
          enabled: false,
        }),
      { wrapper },
    );

    await flush();
    expect(mocks.listFn).not.toHaveBeenCalled();
    expect(result.current.query.fetchStatus).toBe("idle");
  });

  it("debounces search while resetting the page immediately", async () => {
    vi.useFakeTimers();
    mocks.listFn.mockResolvedValue(emptyPage(25, 0));
    const { result } = renderHook(
      () =>
        useServerTable<Item, ItemParams>({
          listFn: mocks.listFn,
          queryKey: ["items"],
          sortKeys: ["name"],
        }),
      { wrapper },
    );

    await flush();
    expect(mocks.listFn).toHaveBeenCalledTimes(1);

    act(() => result.current.setPage(1));
    await flush();
    expect(mocks.listFn).toHaveBeenCalledTimes(2);

    act(() => result.current.setSearch("store"));
    // Page and raw search reset immediately; the page reset triggers a refetch
    // right away but the debounced search has not landed yet.
    expect(result.current.page).toBe(0);
    expect(result.current.search).toBe("store");
    expect(mocks.listFn).toHaveBeenCalledTimes(3);
    expect(mocks.listFn).toHaveBeenLastCalledWith(
      expect.objectContaining({ offset: 0 }),
    );
    expect(mocks.listFn).toHaveBeenLastCalledWith(
      expect.not.objectContaining({ search: "store" }),
    );

    act(() => {
      vi.advanceTimersByTime(250);
    });
    await flush();

    expect(mocks.listFn).toHaveBeenLastCalledWith(
      expect.objectContaining({ search: "store", offset: 0 }),
    );
  });
});
