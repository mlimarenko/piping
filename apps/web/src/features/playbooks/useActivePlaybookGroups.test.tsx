import type { ReactNode } from "react";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ListPlaybookRunGroupsResponse, PlaybookRunGroup, PlaybookRunStatus } from "@/features/playbooks/api";
import type * as PlaybookApiTypes from "@/features/playbooks/api";
import {
  PLAYBOOK_ACTIVE_GROUP_STATUSES,
  PLAYBOOK_ACTIVE_GROUPS_LIMIT,
  useActivePlaybookGroups,
} from "@/features/playbooks/useActivePlaybookGroups";

const apiMock = vi.hoisted(() => ({
  listPlaybookRunGroups: vi.fn(),
}));

vi.mock("@/features/playbooks/api", async (importOriginal) => ({
  ...(await importOriginal<typeof PlaybookApiTypes>()),
  listPlaybookRunGroups: apiMock.listPlaybookRunGroups,
}));

function group(id: string, status: PlaybookRunStatus): PlaybookRunGroup {
  return {
    canceled_runs: 0,
    created_at: "2026-06-29T00:00:00Z",
    failed_runs: 0,
    id,
    metadata: {},
    playbook_id: "playbook-1",
    playbook_version_id: "version-1",
    status,
    succeeded_runs: 0,
    timeout_runs: 0,
    total_runs: 1,
    dispatch_id: null,
    dispatch_attempt: 0,
    trigger_kind: "manual",
    updated_at: "2026-06-29T00:00:00Z",
  };
}

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

describe("useActivePlaybookGroups", () => {
  beforeEach(() => {
    apiMock.listPlaybookRunGroups.mockReset();
  });

  it("calls listPlaybookRunGroups with the exact non-terminal status CSV the plan requires", async () => {
    apiMock.listPlaybookRunGroups.mockResolvedValue({ items: [], limit: PLAYBOOK_ACTIVE_GROUPS_LIMIT, offset: 0, total: 0 } satisfies ListPlaybookRunGroupsResponse);

    const { result } = renderHook(() => useActivePlaybookGroups(), { wrapper });

    await waitFor(() => expect(result.current.isLoading).toBe(false));

    // Pin the literal, not the constant under test — dropping a status (e.g.
    // "leased" or "dispatching") from PLAYBOOK_ACTIVE_GROUP_STATUSES would
    // silently hide those groups from the dock while a self-referential
    // assertion (`status: PLAYBOOK_ACTIVE_GROUP_STATUSES`) would stay green.
    expect(PLAYBOOK_ACTIVE_GROUP_STATUSES).toBe("queued,leased,dispatching,running");
    expect(apiMock.listPlaybookRunGroups).toHaveBeenCalledWith(
      expect.objectContaining({
        status: "queued,leased,dispatching,running",
        limit: PLAYBOOK_ACTIVE_GROUPS_LIMIT,
      }),
    );
  });

  it("re-filters terminal statuses client-side even if the server response leaks one", async () => {
    apiMock.listPlaybookRunGroups.mockResolvedValue({
      items: [
        group("running-1", "running"),
        group("queued-1", "queued"),
        group("succeeded-1", "succeeded"),
        group("failed-1", "failed"),
      ],
      limit: PLAYBOOK_ACTIVE_GROUPS_LIMIT,
      offset: 0,
      total: 4,
    } satisfies ListPlaybookRunGroupsResponse);

    const { result } = renderHook(() => useActivePlaybookGroups(), { wrapper });

    await waitFor(() => expect(result.current.groups).toHaveLength(2));
    expect(result.current.groups.map((g) => g.id).sort()).toEqual(["queued-1", "running-1"]);
  });

  it("flags truncation when the server reports more active groups than this page returned", async () => {
    apiMock.listPlaybookRunGroups.mockResolvedValue({
      items: [group("running-1", "running")],
      limit: PLAYBOOK_ACTIVE_GROUPS_LIMIT,
      offset: 0,
      total: 3,
    } satisfies ListPlaybookRunGroupsResponse);

    const { result } = renderHook(() => useActivePlaybookGroups(), { wrapper });

    await waitFor(() => expect(result.current.isTruncated).toBe(true));
  });

  it("does not flag truncation when the page covers every active group", async () => {
    apiMock.listPlaybookRunGroups.mockResolvedValue({
      items: [group("running-1", "running")],
      limit: PLAYBOOK_ACTIVE_GROUPS_LIMIT,
      offset: 0,
      total: 1,
    } satisfies ListPlaybookRunGroupsResponse);

    const { result } = renderHook(() => useActivePlaybookGroups(), { wrapper });

    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(result.current.isTruncated).toBe(false);
  });

  it("does not fetch when disabled", () => {
    renderHook(() => useActivePlaybookGroups(false), { wrapper });
    expect(apiMock.listPlaybookRunGroups).not.toHaveBeenCalled();
  });
});
