import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, vi } from "vitest";
import "@/shared/i18n";
import CardsView from "./CardsView";
import { DEFAULT_TREE_CONFIG } from "./treeConfig";
import type { ConnectionTarget } from "@/shared/api/types";
import { testUuid } from "@/test/ids";

type LaunchOptionsHookResult = {
  data?: unknown;
  dataUpdatedAt: number;
  isPending: boolean;
  isError: boolean;
};

const virtualizerMocks = vi.hoisted(() => ({ measureElement: vi.fn() }));

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: () => ({
    getTotalSize: () => 140,
    getVirtualItems: () => [{ key: "row-0", index: 0, start: 0 }],
    measure: vi.fn(),
    measureElement: virtualizerMocks.measureElement,
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

// QuickAccessButtons (rendered by CardsView) gates connection quick-actions on
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

const host: ConnectionTarget = {
  id: testUuid("host-1"),
  name: "pxm-piping01",
  targetKind: "virtual_machine",
  status: "active",
  role: "Virtual Machine",
  region: "Germany",
  site: "aeza-de",
  primaryIp: "10.0.1.10",
};

function renderCards(props?: Partial<Parameters<typeof CardsView>[0]>) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const onSelectHost = vi.fn();
  const onConnect = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <CardsView
        hosts={[host]}
        selectedHost={null}
        onSelectHost={onSelectHost}
        onConnect={onConnect}
        quickAccessConfig={DEFAULT_TREE_CONFIG.quickAccess}
        {...props}
      />
    </QueryClientProvider>,
  );

  return { onSelectHost, onConnect };
}

describe("CardsView", () => {
  beforeEach(() => {
    playbookApiMock.getTargetPlaybookAssignments.mockReset();
    playbookApiMock.getTargetPlaybookAssignments.mockResolvedValue({ target: {}, items: [] });
    useConnectionLaunchOptionsMock.mockImplementation((_hostId, options) => ({
      data: options?.enabled
        ? {
            hostId: host.id,
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
      isPending: false,
      isError: false,
    }));
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("reports only hosts in mounted virtual rows rather than the full fleet", () => {
    const onRenderedHostIdsChange = vi.fn();
    const fleet = Array.from({ length: 4218 }, (_, index) => ({ ...host, id: testUuid(`fleet-${index}`) }));
    renderCards({ hosts: fleet, onRenderedHostIdsChange });
    expect(onRenderedHostIdsChange).toHaveBeenLastCalledWith([fleet[0]!.id]);
  });

  it("measures mounted rows so additional host labels do not overlap the next row", () => {
    renderCards();
    expect(virtualizerMocks.measureElement).toHaveBeenCalledWith(
      expect.objectContaining({ dataset: expect.objectContaining({ index: "0" }) }),
    );
  });

  it("keeps host metadata and selection separate from connection actions", () => {
    const { onConnect, onSelectHost } = renderCards({ selectedHost: host });
    const select = screen.getByRole("button", { name: "Select target pxm-piping01" });
    expect(select).toHaveAttribute("aria-pressed", "true");
    expect(select).toHaveTextContent("Virtual Machine");
    expect(select).toHaveTextContent("10.0.1.10");
    expect(select).toHaveTextContent("Germany");
    expect(select).toHaveTextContent("aeza-de");
    fireEvent.click(select);
    expect(onSelectHost).toHaveBeenCalledWith(host);
    expect(onConnect).not.toHaveBeenCalled();
  });

  it("loads configured quick-access actions when a host card receives intent", async () => {
    const { onConnect, onSelectHost } = renderCards();

    expect(useConnectionLaunchOptionsMock).toHaveBeenLastCalledWith("host-1", { enabled: false });

    fireEvent.pointerEnter(screen.getByTestId("host-card-host-1"));
    fireEvent.pointerDown(screen.getByRole("button", { name: "Connect" }), { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /SSH.*root/i }));

    expect(screen.getByTestId("quick-access-host-1")).toBeInTheDocument();
    expect(useConnectionLaunchOptionsMock).toHaveBeenLastCalledWith("host-1", { enabled: true });
    expect(onConnect).toHaveBeenCalledWith(host, "ssh", "ssh-root", "root");
    expect(onSelectHost).not.toHaveBeenCalled();
  });

  it("routes card playbook quick actions through the playbook callback", async () => {
    const onPlaybookQuickAction = vi.fn();
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
      ],
    });
    const { onConnect } = renderCards({
      onPlaybookQuickAction,
      quickAccessConfig: {
        ...DEFAULT_TREE_CONFIG.quickAccess,
        protocols: ["playbook"],
      },
    });

    fireEvent.pointerEnter(screen.getByTestId("host-card-host-1"));
    fireEvent.pointerDown(screen.getByRole("button", { name: "Quick actions" }), { button: 0, ctrlKey: false });
    const button = await screen.findByRole("menuitem", {
      name: "Playbook · Collect inventory",
    });
    fireEvent.click(button);

    expect(playbookApiMock.getTargetPlaybookAssignments).toHaveBeenCalledWith("host-1", 20);
    expect(onPlaybookQuickAction).toHaveBeenCalledWith(
      host,
      expect.objectContaining({
        assignment: expect.objectContaining({ id: "assignment-1" }),
      }),
    );
    expect(onConnect).not.toHaveBeenCalled();
  });

  it("keeps bulk card selection separate from opening the host inspector", () => {
    const onBulkSelectionChange = vi.fn();
    const { onSelectHost } = renderCards({
      bulkSelectionMode: true,
      bulkSelectedHostIds: new Set(),
      onBulkSelectionChange,
    });

    fireEvent.click(screen.getByRole("checkbox", { name: "Select pxm-piping01 for playbook launch" }));

    expect(onBulkSelectionChange).toHaveBeenCalledTimes(1);
    expect(Array.from(onBulkSelectionChange.mock.calls[0]?.[0] ?? [])).toEqual(["host-1"]);
    expect(onSelectHost).not.toHaveBeenCalled();
  });
});
