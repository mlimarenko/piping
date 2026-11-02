import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import type { EffectiveSettingDto } from "@/shared/settings/api";
import type { PlaybookRunGroup, PlaybookRunStatus } from "@/features/playbooks/api";
import type * as PlaybookApiTypes from "@/features/playbooks/api";
import PlaybookRunsDock, { PLAYBOOK_DOCK_SETTING_KEY } from "@/features/playbooks/PlaybookRunsDock";

const settingsApiMock = vi.hoisted(() => ({
  deleteAdminSettingDefault: vi.fn(),
  deleteUserSetting: vi.fn(),
  fetchAdminSettingDefaults: vi.fn(),
  fetchEffectiveSetting: vi.fn(),
  fetchEffectiveSettings: vi.fn(),
  upsertAdminSettingDefault: vi.fn(),
  upsertUserSetting: vi.fn(),
}));
vi.mock("@/shared/settings/api", () => settingsApiMock);

const activeGroupsMock = vi.hoisted(() => ({
  useActivePlaybookGroups: vi.fn(),
}));
vi.mock("@/features/playbooks/useActivePlaybookGroups", () => activeGroupsMock);

const playbooksApiMock = vi.hoisted(() => ({
  listPlaybooks: vi.fn(),
}));
vi.mock("@/features/playbooks/api", async (importOriginal) => ({
  ...(await importOriginal<typeof PlaybookApiTypes>()),
  listPlaybooks: playbooksApiMock.listPlaybooks,
}));

const DOCK_QUERY_KEY = ["settings", "effective", PLAYBOOK_DOCK_SETTING_KEY] as const;

function group(id: string, status: PlaybookRunStatus, overrides: Partial<PlaybookRunGroup> = {}): PlaybookRunGroup {
  return {
    canceled_runs: 0,
    created_at: "2026-06-29T00:00:00Z",
    failed_runs: 0,
    id,
    metadata: {},
    playbook_id: "playbook-1",
    playbook_version_id: "version-1",
    started_at: "2026-06-29T00:00:00Z",
    status,
    succeeded_runs: 0,
    timeout_runs: 0,
    total_runs: 3,
    dispatch_id: null,
    dispatch_attempt: 0,
    trigger_kind: "manual",
    updated_at: "2026-06-29T00:00:00Z",
    ...overrides,
  };
}

function renderDock(groups: PlaybookRunGroup[], openTabGroupIds: Set<string>, expanded = true) {
  activeGroupsMock.useActivePlaybookGroups.mockReturnValue({ groups, isLoading: false });
  playbooksApiMock.listPlaybooks.mockResolvedValue({
    items: [{ playbook: { id: "playbook-1", name: "Update inventory" } }],
    limit: 100,
    offset: 0,
    total: 1,
  });

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData<EffectiveSettingDto>(DOCK_QUERY_KEY, {
    key: PLAYBOOK_DOCK_SETTING_KEY,
    value: { expanded },
    source: "user",
    user_value: { expanded },
    instance_value: undefined,
    updated_at: "2026-06-29T00:00:00Z",
  });

  const onFocusRun = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <PlaybookRunsDock openTabGroupIds={openTabGroupIds} onFocusRun={onFocusRun} />
    </QueryClientProvider>,
  );
  return { onFocusRun };
}

describe("PlaybookRunsDock", () => {
  beforeEach(() => {
    activeGroupsMock.useActivePlaybookGroups.mockReset();
    playbooksApiMock.listPlaybooks.mockReset();
    settingsApiMock.upsertUserSetting.mockReset().mockResolvedValue(undefined);
  });

  it("renders nothing when there are no active groups", () => {
    activeGroupsMock.useActivePlaybookGroups.mockReturnValue({ groups: [], isLoading: false });
    const { container } = render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <PlaybookRunsDock openTabGroupIds={new Set()} onFocusRun={vi.fn()} />
      </QueryClientProvider>,
    );
    expect(container).toBeTruthy();
    expect(container.firstChild).toBeNull();
  });

  it("renders a row per active group, resolving the playbook name from the catalog", async () => {
    renderDock([group("group-1", "running")], new Set());
    const row = (await screen.findByText("Update inventory")).closest("button");
    expect(row).toHaveClass("min-h-9", "text-sm");
    const collapse = screen.getByRole("button", { name: "Collapse active runs" });
    expect(collapse).toHaveClass("size-9");
    expect(collapse.parentElement?.parentElement).toHaveClass("bg-card");
    expect(collapse.parentElement?.parentElement).not.toHaveClass("backdrop-blur");
  });

  it("hides rows whose group is already open as a tab", async () => {
    renderDock(
      [group("group-1", "running"), group("group-2", "queued", { total_runs: 7 })],
      new Set(["group-1"]),
    );
    // Only group-2 should render: its distinct host count shows up once, the
    // header count reflects a single visible row.
    expect(await screen.findByText("7 hosts")).toBeInTheDocument();
    expect(screen.getByText("1 active run")).toBeInTheDocument();
  });

  it("calls onFocusRun with the group and its resolved playbook name when a row is clicked", async () => {
    const { onFocusRun } = renderDock([group("group-1", "running")], new Set());
    const row = await screen.findByText("Update inventory");
    fireEvent.click(row.closest("button")!);
    expect(onFocusRun).toHaveBeenCalledTimes(1);
    const [calledGroup, calledName] = onFocusRun.mock.calls[0] as [PlaybookRunGroup, string];
    expect(calledGroup.id).toBe("group-1");
    expect(calledName).toBe("Update inventory");
  });

  it("collapses to a chip pill and expands back to the list on click", async () => {
    renderDock([group("group-1", "running")], new Set(), false);
    // Collapsed: no row content, just the count chip.
    expect(screen.queryByText("Update inventory")).toBeNull();
    const chip = screen.getByRole("button");
    expect(chip).toHaveClass("min-h-9", "text-sm", "bg-card");
    expect(chip).not.toHaveClass("backdrop-blur");
    fireEvent.click(chip);
    await waitFor(() =>
      expect(settingsApiMock.upsertUserSetting).toHaveBeenCalledWith(
        PLAYBOOK_DOCK_SETTING_KEY,
        { expanded: true },
      ),
    );
  });
});
