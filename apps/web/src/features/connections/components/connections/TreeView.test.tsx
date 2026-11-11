import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import "@/shared/i18n";
import { afterEach, beforeEach, vi } from "vitest";
import TreeView from "./TreeView";
import { DEFAULT_TREE_CONFIG, type TreeConfig } from "./treeConfig";
import { treeCollapseScope } from "./treeCollapseState";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";
import type { ConnectionTarget } from "@/shared/api/types";
import { testUuid } from "@/test/ids";
import type * as SharedSettingsModule from "@/shared/settings";
import type * as VirtualModule from "@tanstack/react-virtual";


type LaunchOptionsHookResult = {
  data?: unknown;
  dataUpdatedAt: number;
  isPending: boolean;
  isError: boolean;
};

const settingsMock = vi.hoisted(() => ({
  value: { version: 1, byScope: {} },
  setValue: vi.fn(),
  isLoading: false,
}));

const virtualMock = vi.hoisted(() => ({
  enabled: false,
  measureElement: vi.fn(),
  scrollToIndex: vi.fn(),
}));

vi.mock("@tanstack/react-virtual", async (importOriginal) => {
  const actual = await importOriginal<typeof VirtualModule>();
  return {
    ...actual,
    useVirtualizer: (options: Parameters<typeof actual.useVirtualizer>[0]) => {
      const virtualizer = actual.useVirtualizer(options);
      virtualizer.scrollToIndex = virtualMock.scrollToIndex;
      if (virtualMock.enabled) {
        virtualizer.getVirtualItems = Object.assign(
          () => [{ index: 6, key: "test-row", start: 240, end: 280, size: 40, lane: 0 }],
          { updateDeps: virtualizer.getVirtualItems.updateDeps.bind(virtualizer.getVirtualItems) },
        );
        virtualizer.measureElement = virtualMock.measureElement;
      }
      return virtualizer;
    },
  };
});

vi.mock("@/shared/settings", async (importOriginal) => ({
  ...(await importOriginal<typeof SharedSettingsModule>()),
  useSetting: () => ({
    value: settingsMock.value,
    setValue: settingsMock.setValue,
    isLoading: settingsMock.isLoading,
  }),
}));

const useConnectionLaunchOptionsMock = vi.fn<
  (hostId: string, options?: { enabled?: boolean; staleTime?: number }) => LaunchOptionsHookResult
>();
const playbookApiMock = vi.hoisted(() => ({
  getTargetPlaybookAssignments: vi.fn(),
}));

vi.mock("@/features/connections/hooks", () => ({
  connectionLaunchOptionsQueryKey: (hostId: string) => ["connections", "launch-options", hostId],
  useConnectionLaunchOptions: (
    hostId: string,
    options?: { enabled?: boolean; staleTime?: number },
  ) => useConnectionLaunchOptionsMock(hostId, options),
}));

vi.mock("@/features/playbooks/api", () => ({
  TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT: 20,
  getTargetPlaybookAssignments: playbookApiMock.getTargetPlaybookAssignments,
}));

// QuickAccessButtons (rendered by TreeView) gates connection quick-actions on
// CONNECT; grant all capabilities so these tests see the connect affordances.
vi.mock("@/shared/auth/permissionsContext", () => ({
  usePermissions: () => ({
    roles: [],
    isAdmin: true,
    can: () => true,
    canAny: () => true,
    hasRole: () => false,
  }),
}));

const hosts: ConnectionTarget[] = [
  {
    id: testUuid("host-1"),
    name: "pxm-piping01",
    targetKind: "virtual_machine",
    status: "active",
  },
  {
    id: testUuid("host-2"),
    name: "pve-1-az-ge",
    targetKind: "virtual_machine",
    status: "active",
  },
];

function renderTree(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return {
    client,
    ...render(
    <QueryClientProvider client={client}>
      <TooltipProvider>{ui}</TooltipProvider>
    </QueryClientProvider>,
    ),
  };
}

describe("TreeView", () => {
  afterEach(() => vi.restoreAllMocks());

  it("waits for the tree configuration before revealing through its saved groups", async () => {
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    const host = { ...hosts[0]!, site: "Berlin" };
    const config: TreeConfig = { ...DEFAULT_TREE_CONFIG, mode: "custom", order: ["site"] };
    settingsMock.value = { version: 1, byScope: { [treeCollapseScope(["site"], true)]: ["site=Berlin|"] } };
    const props = { hosts: [host], selectedHost: host, revealHostId: host.id, onSelectHost: vi.fn(), onConnect: vi.fn() };
    const { client, rerender } = renderTree(<TreeView {...props} revealReady={false}
      config={{ ...DEFAULT_TREE_CONFIG, mode: "custom", order: [] }} />);
    expect(scroll).not.toHaveBeenCalled();
    rerender(<QueryClientProvider client={client}><TooltipProvider><TreeView {...props} revealReady config={config} /></TooltipProvider></QueryClientProvider>);
    await waitFor(() => expect(scroll).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: /collapse Berlin/i })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText(host.name)).toBeInTheDocument();
  });

  it("reveals a linked host after collapsed preferences hydrate, preserving other branches", async () => {
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    const config: TreeConfig = { ...DEFAULT_TREE_CONFIG, mode: "custom", order: ["site"] };
    const fleet = [{ ...hosts[0]!, site: "Berlin" }, { ...hosts[1]!, site: "Paris" }];
    settingsMock.value = { version: 1, byScope: { [treeCollapseScope(["site"], true)]: ["site=Berlin|", "site=Paris|"] } };
    settingsMock.isLoading = true;
    const props = { hosts: fleet, selectedHost: fleet[0]!, revealHostId: fleet[0]!.id, onSelectHost: vi.fn(), onConnect: vi.fn(), config };
    const { client, rerender } = renderTree(<TreeView {...props} />);
    expect(scroll).not.toHaveBeenCalled();
    settingsMock.isLoading = false;
    rerender(<QueryClientProvider client={client}><TooltipProvider><TreeView {...props} /></TooltipProvider></QueryClientProvider>);
    await waitFor(() => expect(scroll).toHaveBeenCalled());
    expect(screen.getByText(fleet[0]!.name)).toBeInTheDocument();
    expect(screen.queryByText(fleet[1]!.name)).not.toBeInTheDocument();
    expect(settingsMock.setValue).not.toHaveBeenCalled();
    const calls = scroll.mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: /collapse Berlin/i }));
    expect(screen.queryByText(fleet[0]!.name)).not.toBeInTheDocument();
    rerender(<QueryClientProvider client={client}><TooltipProvider><TreeView {...props} hosts={fleet.map(item => ({ ...item }))} /></TooltipProvider></QueryClientProvider>);
    expect(screen.queryByText(fleet[0]!.name)).not.toBeInTheDocument();
    expect(scroll).toHaveBeenCalledTimes(calls);
  });

  it("reveals a linked host outside the mounted virtual tree rows", () => {
    virtualMock.enabled = true;
    const fleet = Array.from({ length: 200 }, (_, index) => ({ ...hosts[0]!, id: testUuid(`reveal-${index}`) }));
    renderTree(<TreeView hosts={fleet} selectedHost={fleet[180]!} revealHostId={fleet[180]!.id}
      onSelectHost={vi.fn()} onConnect={vi.fn()} config={{ ...DEFAULT_TREE_CONFIG, mode: "custom", order: [] }} />);
    expect(virtualMock.scrollToIndex).toHaveBeenCalledWith(180, { align: "auto" });
  });
  it.each([false, true])("uses one icon-to-name gap at every depth, including leaves (bulk %s)", (bulkSelectionMode) => {
    const host = {
      ...hosts[0]!,
      region: "Europe",
      site: "Berlin",
      tenant: "North tenant",
      role: "Router",
      platform: "Junos",
      deviceType: "MX",
      cluster: "Primary cluster",
    };
    const config: TreeConfig = {
      ...DEFAULT_TREE_CONFIG,
      mode: "custom",
      order: ["region", "site", "tenant", "role", "platform", "deviceType", "cluster"],
    };
    renderTree(
      <TreeView hosts={[host]} selectedHost={null} onSelectHost={vi.fn()}
        onConnect={vi.fn()} config={config} bulkSelectionMode={bulkSelectionMode} />,
    );
    const names = ["Europe", "Berlin", "North tenant", "Router", "Junos", "MX", "Primary cluster", host.name];
    const indents = names.map((name) => {
      const label = screen.getByText(name);
      const row = label.closest<HTMLElement>(".connection-tree-row")!;
      expect(getComputedStyle(row).columnGap).toBe("0.5rem");
      expect(getComputedStyle(label.parentElement!).columnGap).toBe("0.5rem");
      return Number.parseFloat(row.style.paddingLeft);
    });
    expect(indents.slice(1).map((indent, index) => indent - indents[index]!))
      .toEqual([1.5, 1.5, 1.5, 1.5, 1.5, 1.5, 1.5]);
  });

  it("keeps deep host disclosure alignment in bulk mode and preserves selection", () => {
    const host = {
      ...hosts[0]!,
      region: "Europe",
      site: "Berlin",
      role: "Router",
      deviceType: "MX",
      platform: "Junos",
    };
    const onSelectHost = vi.fn();
    const { client, rerender } = renderTree(
      <TreeView hosts={[host]} selectedHost={null} onSelectHost={onSelectHost}
        onConnect={vi.fn()} config={DEFAULT_TREE_CONFIG} />,
    );
    const row = screen.getByText(host.name).closest<HTMLElement>(".connection-tree-row")!;
    const group = screen.getByText("MX").closest<HTMLElement>(".connection-tree-row")!;
    // Guide overlay precedes the stable disclosure slot and row content.
    const leadingSlots = (element: HTMLElement) => Array.from(element.children)
      .slice(1, 3).map((child) => child.className);
    const hostSlots = leadingSlots(row);
    const groupSlots = leadingSlots(group);
    expect(row.style.paddingLeft).toBe("6.5rem");
    expect(screen.getByText("MX").closest<HTMLElement>(".connection-tree-row")?.style.paddingLeft).toBe("5rem");
    fireEvent.keyDown(row, { key: "Enter" });
    expect(onSelectHost).toHaveBeenCalledWith(host);
    rerender(
      <QueryClientProvider client={client}>
        <TooltipProvider>
          <TreeView hosts={[host]} selectedHost={null} onSelectHost={onSelectHost}
            onConnect={vi.fn()} config={DEFAULT_TREE_CONFIG} bulkSelectionMode
            onBulkSelectionChange={vi.fn()} />
        </TooltipProvider>
      </QueryClientProvider>,
    );
    const bulkHost = screen.getByText(host.name).closest<HTMLElement>(".connection-tree-row")!;
    const bulkGroup = screen.getByText("MX").closest<HTMLElement>(".connection-tree-row")!;
    expect(leadingSlots(bulkHost)).toEqual(hostSlots);
    expect(leadingSlots(bulkGroup)).toEqual(groupSlots);
    expect(bulkHost.lastElementChild).toContainElement(screen.getByRole("checkbox", { name: new RegExp(host.name) }));
    expect(bulkGroup.lastElementChild).toContainElement(screen.getByRole("checkbox", { name: /MX/ }));
    expect(screen.getByText(host.name).closest<HTMLElement>(".connection-tree-row")?.style.paddingLeft).toBe("6.5rem");
    fireEvent.click(screen.getByRole("checkbox", { name: new RegExp(host.name) }));
    expect(onSelectHost).toHaveBeenCalledTimes(1);
  });

  it("keeps deep custom paths intact for group and host selection", () => {
    const host = {
      ...hosts[0]!,
      region: "Europe",
      site: "Berlin",
      tenant: "Tenant",
      role: "Router",
      platform: "Junos",
      deviceType: "MX",
      cluster: "Cluster",
    };
    const config: TreeConfig = {
      ...DEFAULT_TREE_CONFIG,
      mode: "custom",
      order: ["region", "site", "tenant", "role", "platform", "deviceType", "cluster"],
    };
    const selectedNode = { type: "region" as const, id: "region=Europe|", name: "Europe", hosts: [host] };
    const client = new QueryClient();
    const view = (selectedHost: ConnectionTarget | null, selectedGroup: typeof selectedNode | null) => (
      <QueryClientProvider client={client}>
        <TooltipProvider>
          <TreeView hosts={[host]} selectedHost={selectedHost} selectedNode={selectedGroup}
            onSelectHost={vi.fn()} onConnect={vi.fn()} config={config} />
        </TooltipProvider>
      </QueryClientProvider>
    );
    const { rerender } = render(view(null, selectedNode));
    const hostRow = screen.getByText(host.name).closest<HTMLElement>(".connection-tree-row")!;
    expect(hostRow.style.paddingLeft).toBe("11rem");
    expect(screen.getByText("Europe").closest(".connection-tree-row")).toHaveClass("bg-primary/8");
    rerender(view(host, null));
    const selectedRow = screen.getByText(host.name).closest<HTMLElement>(".connection-tree-row")!;
    expect(selectedRow).toHaveClass("bg-primary/8");
    expect(selectedRow.firstElementChild?.querySelector(".border-primary\\/70")).toBeTruthy();
  });
  it("tracks the selected identity through its real groups, collapse and filtering", () => {
    const first = { ...hosts[0]!, region: "A", site: "Shared" };
    const second = { ...hosts[1]!, region: "B", site: "Shared" };
    const config: TreeConfig = { ...DEFAULT_TREE_CONFIG, mode: "custom", order: ["region", "site"] };
    const client = new QueryClient();
    const view = (items: ConnectionTarget[], selected: ConnectionTarget | null, node: { type: "site"; id: string; name: string; hosts: ConnectionTarget[] } | null = null) => (
      <QueryClientProvider client={client}><TooltipProvider><TreeView hosts={items} selectedHost={selected} selectedNode={node}
        onSelectHost={vi.fn()} onConnect={vi.fn()} config={config} bulkSelectionMode bulkSelectedHostIds={new Set([second.id])} /></TooltipProvider></QueryClientProvider>
    );
    const { rerender } = render(view([first, second], first));
    const row = (name: string) => screen.getByText(name).closest<HTMLElement>(".connection-tree-row")!;
    const shared = () => screen.getAllByText("Shared").map((element) => element.closest<HTMLElement>(".connection-tree-row")!);
    expect(row("A")).toHaveClass("bg-primary/5");
    expect(shared()[0]).toHaveClass("bg-primary/5");
    expect(row("B")).not.toHaveClass("bg-primary/5");
    expect(shared()[1]).not.toHaveClass("bg-primary/5");
    expect(row(first.name).firstElementChild?.querySelector(".border-primary\\/70")).toBeTruthy();
    rerender(view([first, second], null, { type: "site", id: "region=B|site=Shared|", name: "Shared", hosts: [second] }));
    expect(row("A")).not.toHaveClass("bg-primary/5");
    expect(row("B")).toHaveClass("bg-primary/5");
    expect(shared()[1]).toHaveClass("bg-primary/8");
    rerender(view([first, second], first));
    fireEvent.click(screen.getByRole("button", { name: /collapse A/i }));
    expect(row("A")).toHaveClass("bg-primary/5");
    expect(screen.queryByText(first.name)).not.toBeInTheDocument();
    rerender(view([second], first));
    expect(row("B")).not.toHaveClass("bg-primary/5");
    rerender(view([second], null, { type: "site", id: "region=A|site=Shared|", name: "Shared", hosts: [first] }));
    expect(row("B")).not.toHaveClass("bg-primary/5");
    expect(shared()[0]).not.toHaveClass("bg-primary/8");
    rerender(view([second], null));
    expect(row("B")).not.toHaveClass("bg-primary/5");
  });
  it("opens target details from keyboard focus on the hostname", async () => {
    renderTree(<TreeView hosts={hosts} config={DEFAULT_TREE_CONFIG} onSelectHost={vi.fn()}
      onSelectNode={vi.fn()} onConnect={vi.fn()} selectedHost={null} />);
    const name = screen.getByText("pxm-piping01", { exact: true });
    act(() => { name.focus(); });
    expect(name).toHaveFocus();
    expect(await screen.findByRole("tooltip")).toHaveTextContent("pxm-piping01");
  });
  beforeEach(() => {
    virtualMock.enabled = false;
    virtualMock.measureElement.mockReset();
    virtualMock.scrollToIndex.mockReset();
    settingsMock.value = { version: 1, byScope: {} };
    settingsMock.setValue.mockReset();
    settingsMock.isLoading = false;
    playbookApiMock.getTargetPlaybookAssignments.mockReset();
    playbookApiMock.getTargetPlaybookAssignments.mockResolvedValue({ target: {}, items: [] });
    useConnectionLaunchOptionsMock.mockImplementation((_hostId: string, options?: { enabled?: boolean; staleTime?: number }) => ({
      data: options?.enabled
        ? {
          hostId: "host-1",
          protocols: [
            {
              protocol: "ssh",
              render: "buttons",
              accounts: [
                {
                  entryId: "ssh-root",
                  kind: "shared",
                  label: "root",
                  username: "root",
                  source: {},
                  isDefault: true,
                  requiresInput: false,
                },
              ],
            },
          ],
        }
        : undefined,
      dataUpdatedAt: options?.enabled === true ? Number.MAX_SAFE_INTEGER : 0,
      isPending: options?.enabled === true,
      isError: false,
    }));
  });

  it("measures unconstrained virtual rows and retains their full ancestor guides", () => {
    virtualMock.enabled = true;
    const manyHosts = Array.from({ length: 161 }, (_, index) => ({
      ...hosts[0]!,
      id: testUuid(`virtual-${index}`),
      name: `virtual-${index}`,
      region: index === 160 ? "Z" : "A",
      site: index < 80 ? "Berlin" : "Hamburg",
      role: "Router",
      deviceType: "MX",
    }));
    const { container } = renderTree(
      <TreeView hosts={manyHosts} selectedHost={null} onSelectHost={vi.fn()}
        onConnect={vi.fn()} config={DEFAULT_TREE_CONFIG} />,
    );
    const wrapper = container.querySelector<HTMLElement>(".connection-tree-virtual-row");
    expect(wrapper).not.toBeNull();
    expect(wrapper?.dataset.index).toBe("6");
    expect(wrapper?.style.height).toBe("");
    expect(virtualMock.measureElement).toHaveBeenCalledWith(wrapper);
    const rails = Array.from(wrapper!.querySelectorAll<HTMLElement>(".border-l"));
    expect(rails.map((rail) => [rail.style.left, rail.style.bottom])).toEqual([
      ["1.25rem", "0px"],
      ["2.75rem", "0px"],
      ["4.25rem", "0px"],
      ["5.75rem", "0px"],
    ]);
    expect(wrapper?.querySelector(".border-t")).toBeInTheDocument();
  });

  it("accents only the passing route rail on a virtualized sibling row", () => {
    virtualMock.enabled = true;
    const manyHosts = Array.from({ length: 161 }, (_, index) => ({
      ...hosts[0]!, id: testUuid(`route-${index}`), name: `route-${index}`,
      region: "A", site: "Berlin", role: "Router", deviceType: "MX",
    }));
    const { container } = renderTree(
      <TreeView hosts={manyHosts} selectedHost={manyHosts[5]!} onSelectHost={vi.fn()}
        onConnect={vi.fn()} config={DEFAULT_TREE_CONFIG} />,
    );
    const row = container.querySelector<HTMLElement>(".connection-tree-virtual-row")!;
    const active = Array.from(row.querySelectorAll<HTMLElement>(".border-primary\\/70"));
    expect(active).toHaveLength(1);
    expect(active[0]!.style.left).toBe("5.75rem");
    expect(active[0]!.style.top).toBe("0px");
    expect(active[0]!.style.bottom).toBe("0px");
    expect(screen.queryByText("A")).not.toBeInTheDocument();
  });

  it("reports only mounted hosts and excludes collapsed branches", async () => {
    const onRenderedHostIdsChange = vi.fn();
    const groupedHosts = hosts.map((host) => ({ ...host, site: "Berlin" }));
    renderTree(
      <TreeView hosts={groupedHosts} config={DEFAULT_TREE_CONFIG} onSelectHost={vi.fn()}
        onSelectNode={vi.fn()} onConnect={vi.fn()} selectedHost={null} onRenderedHostIdsChange={onRenderedHostIdsChange} />,
    );
    await waitFor(() => expect(onRenderedHostIdsChange).toHaveBeenLastCalledWith(hosts.map((host) => host.id)));
    fireEvent.click(screen.getAllByRole("button", { name: /Collapse Berlin/i })[0]!);
    await waitFor(() => expect(onRenderedHostIdsChange).toHaveBeenLastCalledWith([]));
    fireEvent.click(screen.getAllByRole("button", { name: /Expand Berlin/i })[0]!);
    await waitFor(() => expect(onRenderedHostIdsChange).toHaveBeenLastCalledWith(hosts.map((host) => host.id)));
  });

  it("renders hosts flat when auto mode has no grouping dimensions", () => {
    renderTree(
      <TreeView
        hosts={hosts}
        selectedHost={null}
        onSelectHost={() => {}}
        onConnect={() => {}}
        config={DEFAULT_TREE_CONFIG}
      />,
    );

    expect(screen.getByText("pxm-piping01")).toBeInTheDocument();
    expect(screen.getByText("pve-1-az-ge")).toBeInTheDocument();
  });

  it("renders hosts inside root groups without requiring a manual expand click", () => {
    const groupedHosts = hosts.map((host) => ({
      ...host,
      site: "pve-1-az-ge",
      role: "Proxmox VM",
      platform: "Debian Linux",
    }));

    renderTree(
      <TreeView
        hosts={groupedHosts}
        selectedHost={null}
        onSelectHost={() => {}}
        onConnect={() => {}}
        config={DEFAULT_TREE_CONFIG}
      />,
    );

    expect(screen.getAllByText("pve-1-az-ge").length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText("Proxmox VM")).toBeInTheDocument();
    expect(screen.queryByText("Debian Linux")).not.toBeInTheDocument();
    expect(screen.getByText("pxm-piping01")).toBeInTheDocument();
  });

  it("selects a leaf independently of its parent grouping", () => {
    const onSelectHost = vi.fn();
    const groupedHosts = hosts.map((host) => ({
      ...host,
      site: "Germany",
      role: "Platform",
    }));

    renderTree(
      <TreeView
        hosts={groupedHosts}
        selectedHost={null}
        onSelectHost={onSelectHost}
        onConnect={() => {}}
        config={{ ...DEFAULT_TREE_CONFIG, mode: "custom", order: ["site", "role"], includeUnknown: true }}
      />,
    );

    const hostRow = screen.getByRole("button", { name: /pxm-piping01/ });
    fireEvent.click(hostRow);
    expect(onSelectHost).toHaveBeenCalledWith(groupedHosts[0]);
  });

  it("routes a host selection through one callback", () => {
    const onSelectHost = vi.fn();
    const onSelectNode = vi.fn();
    renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={onSelectHost}
        onSelectNode={onSelectNode}
        onConnect={() => {}}
        config={DEFAULT_TREE_CONFIG}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /pxm-piping01/ }));
    expect(onSelectHost).toHaveBeenCalledOnce();
    // Host selection already updates the selected node in the workspace; a
    // second onSelectNode call would write the same URL again.
    expect(onSelectNode).not.toHaveBeenCalled();
  });

  it("expands search results without overwriting saved collapsed branches", async () => {
    const groupedHosts: ConnectionTarget[] = [
      {
        id: testUuid("host-1"),
        name: "alpha-search-target",
        targetKind: "device",
        status: "active",
        site: "Europe",
      },
      {
        id: testUuid("host-2"),
        name: "beta-target",
        targetKind: "device",
        status: "active",
        site: "Asia",
      },
    ];
    const config: TreeConfig = {
      ...DEFAULT_TREE_CONFIG,
      mode: "custom",
      order: ["site"],
      includeUnknown: true,
    };
    const collapseScope = treeCollapseScope(["site"], true);
    settingsMock.value = {
      version: 1,
      byScope: {
        [collapseScope]: ["site=Europe|"],
      },
    };
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const renderSearchTree = (nextHosts: ConnectionTarget[], searchActive: boolean) => (
      <QueryClientProvider client={client}>
        <TooltipProvider>
          <TreeView
            hosts={nextHosts}
            selectedHost={null}
            onSelectHost={() => {}}
            onConnect={() => {}}
            config={config}
            searchActive={searchActive}
          />
        </TooltipProvider>
      </QueryClientProvider>
    );

    const { rerender } = render(renderSearchTree(groupedHosts, false));

    await waitFor(() => {
      expect(screen.queryByText("alpha-search-target")).not.toBeInTheDocument();
    });

    rerender(renderSearchTree([groupedHosts[0]!], true));
    expect(screen.getByText("alpha-search-target")).toBeInTheDocument();

    rerender(renderSearchTree(groupedHosts, false));
    await waitFor(() => {
      expect(screen.queryByText("alpha-search-target")).not.toBeInTheDocument();
    });
    expect(settingsMock.setValue).not.toHaveBeenCalled();
  });

  it("does not render per-host status chips in tree rows", () => {
    renderTree(
      <TreeView
        hosts={hosts}
        selectedHost={null}
        onSelectHost={() => {}}
        onConnect={() => {}}
        config={DEFAULT_TREE_CONFIG}
      />,
    );

    expect(screen.queryByText("active")).not.toBeInTheDocument();
  });

  it("enables launch options on host row hover before selection", () => {
    const onSelectHost = vi.fn();
    renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={onSelectHost}
        onConnect={() => {}}
        config={DEFAULT_TREE_CONFIG}
      />,
    );

    expect(useConnectionLaunchOptionsMock).toHaveBeenLastCalledWith("host-1", { enabled: false });
    fireEvent.pointerEnter(screen.getByText(hosts[0]!.name).closest<HTMLElement>(".connection-tree-row")!);
    expect(useConnectionLaunchOptionsMock).toHaveBeenLastCalledWith("host-1", { enabled: true });
    expect(onSelectHost).not.toHaveBeenCalled();
  });

  it("enables launch options on host row focus before selection", () => {
    const onSelectHost = vi.fn();
    renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={onSelectHost}
        onConnect={() => {}}
        config={DEFAULT_TREE_CONFIG}
      />,
    );

    expect(useConnectionLaunchOptionsMock).toHaveBeenLastCalledWith("host-1", { enabled: false });
    fireEvent.focus(screen.getByText(hosts[0]!.name).closest<HTMLElement>(".connection-tree-row")!);
    expect(useConnectionLaunchOptionsMock).toHaveBeenLastCalledWith("host-1", { enabled: true });
    expect(onSelectHost).not.toHaveBeenCalled();
  });

  it("enables quick-access launch options only after row action intent", () => {
    const onSelectHost = vi.fn();
    const onConnect = vi.fn();
    renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={onSelectHost}
        onConnect={onConnect}
        config={DEFAULT_TREE_CONFIG}
      />,
    );

    expect(useConnectionLaunchOptionsMock).toHaveBeenLastCalledWith("host-1", { enabled: false });

    fireEvent.pointerEnter(screen.getByTestId("quick-access-host-1"));
    fireEvent.pointerDown(screen.getByRole("button", { name: "Connect" }), { button: 0, ctrlKey: false });
    fireEvent.click(screen.getByRole("menuitem", { name: /SSH.*root/i }));

    expect(useConnectionLaunchOptionsMock).toHaveBeenLastCalledWith("host-1", { enabled: true });
    expect(onConnect).toHaveBeenCalledWith(hosts[0], "ssh", "ssh-root", "root");
    expect(onSelectHost).not.toHaveBeenCalled();
  });

  it("omits quick-access RDP actions when the personal credential overlay still requires input", () => {
    const onConnect = vi.fn();
    useConnectionLaunchOptionsMock.mockImplementation((_hostId: string, options?: { enabled?: boolean }) => ({
      data: options?.enabled
        ? {
          hostId: "host-1",
          protocols: [
            {
              protocol: "rdp",
              render: "buttons",
              accounts: [
                {
                  entryId: "rdp-shared",
                  kind: "shared",
                  label: "Сервер — RDP",
                  source: { recordName: "Windows RDP" },
                  isDefault: false,
                  requiresInput: true,
                },
                {
                  entryId: "rdp-grad",
                  kind: "personal",
                  label: "GRAD",
                  username: "m.limarenko@grad.net",
                  source: { credentialSetName: "GRAD" },
                  isDefault: true,
                  requiresInput: true,
                },
              ],
            },
          ],
        }
        : undefined,
      dataUpdatedAt: options?.enabled === true ? Number.MAX_SAFE_INTEGER : 0,
      isPending: false,
      isError: false,
    }));

    renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={() => {}}
        onConnect={onConnect}
        config={{ ...DEFAULT_TREE_CONFIG, quickAccess: { ...DEFAULT_TREE_CONFIG.quickAccess, protocols: ["rdp"] } }}
      />,
    );

    fireEvent.pointerEnter(screen.getByTestId("quick-access-host-1"));

    expect(screen.queryByRole("button", { name: /RDP as GRAD/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /RDP as Сервер — RDP/i })).not.toBeInTheDocument();
    expect(onConnect).not.toHaveBeenCalled();
  });

  it("uses personal RDP credentials behind the configured server quick action", () => {
    const onConnect = vi.fn();
    useConnectionLaunchOptionsMock.mockImplementation((_hostId: string, options?: { enabled?: boolean }) => ({
      data: options?.enabled
        ? {
          hostId: "host-1",
          protocols: [
            {
              protocol: "rdp",
              render: "buttons",
              accounts: [
                {
                  entryId: "rdp-shared",
                  kind: "shared",
                  label: "Сервер — RDP",
                  source: { recordName: "Windows RDP" },
                  isDefault: false,
                  requiresInput: true,
                },
                {
                  entryId: "rdp-grad",
                  kind: "personal",
                  label: "GRAD",
                  username: "m.limarenko@grad.net",
                  source: { credentialSetName: "GRAD" },
                  isDefault: true,
                  requiresInput: false,
                },
              ],
            },
          ],
        }
        : undefined,
      dataUpdatedAt: options?.enabled === true ? Number.MAX_SAFE_INTEGER : 0,
      isPending: false,
      isError: false,
    }));

    renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={() => {}}
        onConnect={onConnect}
        config={{ ...DEFAULT_TREE_CONFIG, quickAccess: { ...DEFAULT_TREE_CONFIG.quickAccess, protocols: ["rdp"] } }}
      />,
    );

    fireEvent.pointerEnter(screen.getByTestId("quick-access-host-1"));
    fireEvent.pointerDown(screen.getByRole("button", { name: "Connect" }), { button: 0, ctrlKey: false });
    fireEvent.click(screen.getByRole("menuitem", { name: /RDP.*Сервер — RDP/i }));

    expect(screen.queryByRole("button", { name: /RDP as GRAD/i })).not.toBeInTheDocument();
    expect(onConnect).toHaveBeenCalledWith(
      hosts[0],
      "rdp",
      "rdp-grad",
      "m.limarenko@grad.net",
    );
  });

  it("uses launch-ready shared RDP credentials in quick access when they exist", () => {
    const onConnect = vi.fn();
    useConnectionLaunchOptionsMock.mockImplementation((_hostId: string, options?: { enabled?: boolean }) => ({
      data: options?.enabled
        ? {
          hostId: "host-1",
          protocols: [
            {
              protocol: "rdp",
              render: "buttons",
              accounts: [
                {
                  entryId: "rdp-shared",
                  kind: "shared",
                  label: "Сервер — RDP",
                  username: "leader",
                  source: { recordName: "Linux RDP" },
                  isDefault: false,
                  requiresInput: false,
                },
                {
                  entryId: "rdp-grad",
                  kind: "personal",
                  label: "GRAD",
                  username: "m.limarenko@grad.net",
                  source: { credentialSetName: "GRAD" },
                  isDefault: true,
                  requiresInput: false,
                },
              ],
            },
          ],
        }
        : undefined,
      dataUpdatedAt: options?.enabled === true ? Number.MAX_SAFE_INTEGER : 0,
      isPending: false,
      isError: false,
    }));

    renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={() => {}}
        onConnect={onConnect}
        config={{ ...DEFAULT_TREE_CONFIG, quickAccess: { ...DEFAULT_TREE_CONFIG.quickAccess, protocols: ["rdp"] } }}
      />,
    );

    fireEvent.pointerEnter(screen.getByTestId("quick-access-host-1"));
    fireEvent.pointerDown(screen.getByRole("button", { name: "Connect" }), { button: 0, ctrlKey: false });
    fireEvent.click(screen.getByRole("menuitem", { name: /RDP.*Сервер — RDP/i }));

    expect(screen.queryByRole("button", { name: /RDP as GRAD/i })).not.toBeInTheDocument();
    expect(onConnect).toHaveBeenCalledWith(
      hosts[0],
      "rdp",
      "rdp-shared",
      "leader",
    );
  });

  it("shows eligible managed jobs in quick access after row action intent", async () => {
    playbookApiMock.getTargetPlaybookAssignments.mockResolvedValue({
      target: {},
      items: [
        {
          assignment: {
            id: "assignment-1",
            name: "Collect inventory",
            quick_action_enabled: true,
            enabled: true,
            executable: true,
            job_template_id: "job-1",
          },
          job_template: { id: "job-1", name: "Linux inventory", inventory: { kind: "managed", target_set_id: "set-1" } },
        },
        {
          assignment: {
            id: "assignment-2",
            name: "Dangerous action",
            quick_action_enabled: true,
            enabled: true,
            executable: true,
            job_template_id: "job-2",
          },
          job_template: { id: "job-2", name: "Dangerous action", inventory: { kind: "native", sources: [] } },
        },
      ],
    });

    const onConnect = vi.fn();
    const onPlaybookQuickAction = vi.fn();
    renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={() => {}}
        onConnect={onConnect}
        onPlaybookQuickAction={onPlaybookQuickAction}
        config={DEFAULT_TREE_CONFIG}
      />,
    );

    fireEvent.pointerEnter(screen.getByTestId("quick-access-host-1"));

    fireEvent.pointerDown(screen.getByRole("button", { name: "Connect" }), { button: 0, ctrlKey: false });
    const playbookButton = await screen.findByRole("menuitem", {
      name: "Playbook · Collect inventory",
    });
    expect(playbookButton).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /Playbook.*Dangerous action/ })).not.toBeInTheDocument();
    expect(playbookApiMock.getTargetPlaybookAssignments).toHaveBeenCalledWith("host-1", 20, expect.any(AbortSignal));

    fireEvent.click(playbookButton);

    expect(onPlaybookQuickAction).toHaveBeenCalledWith(
      hosts[0],
      expect.objectContaining({
        assignment: expect.objectContaining({ id: "assignment-1" }),
      }),
    );
    expect(onConnect).not.toHaveBeenCalled();
  });

  it("still enables quick-access query on intent when cached launch options exist", () => {
    const staleOptions = {
      hostId: "host-1",
      protocols: [
        {
          protocol: "ssh" as const,
          render: "buttons" as const,
          accounts: [
            {
              entryId: "ssh-old",
              kind: "shared" as const,
              label: "old",
              username: "old",
              source: {},
              isDefault: true,
              requiresInput: false,
            },
          ],
        },
      ],
    };
    const { client } = renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={() => {}}
        onConnect={() => {}}
        config={DEFAULT_TREE_CONFIG}
      />,
    );
    client.setQueryData(["connections", "launch-options", "host-1"], staleOptions);

    fireEvent.pointerEnter(screen.getByTestId("quick-access-host-1"));

    expect(useConnectionLaunchOptionsMock).toHaveBeenLastCalledWith("host-1", { enabled: true });
  });

  it("does not expose cached app quick-access actions before fresh intent data resolves", () => {
    const cachedOptions = {
      hostId: "host-1",
      protocols: [
        {
          protocol: "app" as const,
          render: "buttons" as const,
          accounts: [
            {
              entryId: "web-old",
              kind: "shared" as const,
              label: "old web",
              uri: "https://old.example.test",
              source: {},
              isDefault: true,
              requiresInput: false,
            },
          ],
        },
      ],
    };
    const { client } = renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={() => {}}
        onConnect={() => {}}
        config={{
          ...DEFAULT_TREE_CONFIG,
          quickAccess: {
            ...DEFAULT_TREE_CONFIG.quickAccess,
            protocols: ["app", "ssh"],
          },
        }}
      />,
    );
    client.setQueryData(["connections", "launch-options", "host-1"], cachedOptions);

    fireEvent.pointerEnter(screen.getByTestId("quick-access-host-1"));

    expect(useConnectionLaunchOptionsMock).toHaveBeenLastCalledWith("host-1", {
      enabled: true,
    });
    expect(screen.queryByRole("button", { name: /APP as old web/i })).not.toBeInTheDocument();
    fireEvent.pointerDown(screen.getByRole("button", { name: "Connect" }), { button: 0, ctrlKey: false });
    expect(screen.getByRole("menuitem", { name: /SSH.*root/i })).toBeInTheDocument();
  });

  it("does not treat React Query cached app launch data as fresh after hover", () => {
    const cachedOptions = {
      hostId: "host-1",
      protocols: [
        {
          protocol: "app" as const,
          render: "buttons" as const,
          accounts: [
            {
              entryId: "web-old",
              kind: "shared" as const,
              label: "old web",
              uri: "https://old.example.test",
              source: {},
              isDefault: true,
              requiresInput: false,
            },
          ],
        },
      ],
    };
    useConnectionLaunchOptionsMock.mockImplementation((_hostId, options) => ({
      data: options?.enabled ? cachedOptions : undefined,
      dataUpdatedAt: Date.now() - 60_000,
      isPending: options?.enabled === true,
      isError: false,
    }));

    renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={() => {}}
        onConnect={() => {}}
        config={{
          ...DEFAULT_TREE_CONFIG,
          quickAccess: {
            ...DEFAULT_TREE_CONFIG.quickAccess,
            protocols: ["app"],
          },
        }}
      />,
    );

    fireEvent.pointerEnter(screen.getByTestId("quick-access-host-1"));

    expect(screen.queryByRole("button", { name: /APP as old web/i })).not.toBeInTheDocument();
  });

  it("keeps bulk tree selection separate from selecting the inspector node", () => {
    const onSelectHost = vi.fn();
    const onBulkSelectionChange = vi.fn();
    renderTree(
      <TreeView
        hosts={[hosts[0]!]}
        selectedHost={null}
        onSelectHost={onSelectHost}
        onConnect={() => {}}
        config={DEFAULT_TREE_CONFIG}
        bulkSelectionMode
        bulkSelectedHostIds={new Set()}
        onBulkSelectionChange={onBulkSelectionChange}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "Select pxm-piping01 for job launch" }));

    expect(onBulkSelectionChange).toHaveBeenCalledTimes(1);
    expect(Array.from(onBulkSelectionChange.mock.calls[0]?.[0] ?? [])).toEqual(["host-1"]);
    expect(onSelectHost).not.toHaveBeenCalled();
  });
});
