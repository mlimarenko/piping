import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import "@/shared/i18n";
import { beforeEach, vi } from "vitest";
import TreeView from "./TreeView";
import { DEFAULT_TREE_CONFIG, type TreeConfig } from "./treeConfig";
import { treeCollapseScope } from "./treeCollapseState";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";
import type { ConnectionTarget } from "@/shared/api/types";
import { testUuid } from "@/test/ids";
import type * as SharedSettingsModule from "@/shared/settings";


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
  it("opens target details from keyboard focus on the hostname", async () => {
    renderTree(<TreeView hosts={hosts} config={DEFAULT_TREE_CONFIG} onSelectHost={vi.fn()}
      onSelectNode={vi.fn()} onConnect={vi.fn()} selectedHost={null} />);
    const name = screen.getByText("pxm-piping01", { exact: true });
    act(() => { name.focus(); });
    expect(name).toHaveFocus();
    expect(await screen.findByRole("tooltip")).toHaveTextContent("pxm-piping01");
  });
  beforeEach(() => {
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
    expect(screen.getByText("Debian Linux")).toBeInTheDocument();
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

  it("shows eligible low-risk playbooks in quick access after row action intent", async () => {
    playbookApiMock.getTargetPlaybookAssignments.mockResolvedValue({
      target: {},
      items: [
        {
          assignment: {
            id: "assignment-1",
            name: "Collect inventory",
            quick_action_enabled: true,
          },
          playbook: {
            playbook: {
              id: "playbook-1",
              name: "Linux inventory",
              status: "active",
              risk: "low",
            },
            current_version: { id: "version-1" },
          },
        },
        {
          assignment: {
            id: "assignment-2",
            name: "Dangerous action",
            quick_action_enabled: true,
          },
          playbook: {
            playbook: {
              id: "playbook-2",
              name: "Dangerous action",
              status: "active",
              risk: "high",
            },
            current_version: { id: "version-2" },
          },
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
    expect(playbookApiMock.getTargetPlaybookAssignments).toHaveBeenCalledWith("host-1", 20);

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
      staleTime: 0,
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

    fireEvent.click(screen.getByRole("checkbox", { name: "Select pxm-piping01 for playbook launch" }));

    expect(onBulkSelectionChange).toHaveBeenCalledTimes(1);
    expect(Array.from(onBulkSelectionChange.mock.calls[0]?.[0] ?? [])).toEqual(["host-1"]);
    expect(onSelectHost).not.toHaveBeenCalled();
  });
});
