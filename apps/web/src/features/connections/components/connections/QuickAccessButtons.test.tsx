import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import QuickAccessButtons from "./QuickAccessButtons";
import { DEFAULT_TREE_CONFIG } from "./treeConfig";
import type { ConnectionTarget } from "@/shared/api/types";
import { testUuid } from "@/test/ids";

type LaunchOptionsHookResult = {
  data?: unknown;
  dataUpdatedAt: number;
  isPending: boolean;
  isError: boolean;
};

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

// Capability gate is exercised per-test through this mock so we can flip CONNECT
// on and off while keeping every other capability granted.
const canMock = vi.fn<(action: string, resourceType: string) => boolean>(() => true);

vi.mock("@/shared/auth/permissionsContext", () => ({
  usePermissions: () => ({
    roles: [],
    isAdmin: false,
    can: (action: string, resourceType: string) => canMock(action, resourceType),
    canAny: () => false,
    hasRole: () => false,
  }),
}));

const host: ConnectionTarget = {
  id: testUuid("host-1"),
  name: "pxm-piping01",
  targetKind: "virtual_machine",
  status: "active",
};

// Mixed config: one connection protocol (ssh) + playbook quick-actions. This is
// the case that proves the CONNECT gate scopes to the connection branch only.
const mixedConfig = {
  ...DEFAULT_TREE_CONFIG.quickAccess,
  protocols: ["ssh", "playbook"] as const,
};

function renderQuickAccess() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onConnect = vi.fn();
  const onPlaybookQuickAction = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <QuickAccessButtons
        host={host}
        config={{ ...mixedConfig, protocols: [...mixedConfig.protocols] }}
        // intentStartedAt set to an early epoch so the launch-options + playbook
        // queries are enabled immediately and every resolved `dataUpdatedAt`
        // clears the freshness gate — no hover simulation needed.
        intentStartedAt={1}
        onConnect={onConnect}
        onPlaybookQuickAction={onPlaybookQuickAction}
      />
    </QueryClientProvider>,
  );
  return { onConnect, onPlaybookQuickAction };
}

describe("QuickAccessButtons CONNECT gating", () => {
  beforeEach(() => {
    canMock.mockReset();
    canMock.mockReturnValue(true);
    useConnectionLaunchOptionsMock.mockReset();
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
    playbookApiMock.getTargetPlaybookAssignments.mockReset();
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
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("renders connection AND playbook quick-actions when the user holds CONNECT", async () => {
    const { onConnect } = renderQuickAccess();
    const trigger = await screen.findByRole("button", { name: "Connect" });
    expect(screen.getAllByRole("button")).toHaveLength(1);
    expect(onConnect).not.toHaveBeenCalled();
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    expect(await screen.findByRole("menuitem", { name: /SSH.*root/i })).toBeInTheDocument();
    expect(
      await screen.findByRole("menuitem", { name: /Playbook.*Collect inventory/i }),
    ).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.click(screen.getByRole("menuitem", { name: /SSH.*root/i }));
    expect(onConnect).toHaveBeenCalledWith(host, "ssh", "ssh-root", "root");
  });

  it("hides connection quick-actions but keeps playbook quick-actions without CONNECT", async () => {
    canMock.mockImplementation(
      (action, resourceType) => !(action === "CONNECT" && resourceType === "CONNECTION_TARGET"),
    );

    renderQuickAccess();
    fireEvent.pointerDown(await screen.findByRole("button", { name: "Quick actions" }), { button: 0, ctrlKey: false });
    // Playbook quick-action stays visible (it does not require CONNECT)…
    expect(
      await screen.findByRole("menuitem", { name: /Playbook.*Collect inventory/i }),
    ).toBeInTheDocument();
    // …while the connection (SSH) quick-action is suppressed.
    await waitFor(() => {
      expect(screen.queryByRole("menuitem", { name: /SSH.*root/i })).not.toBeInTheDocument();
    });
  });
});
