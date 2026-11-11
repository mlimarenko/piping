import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import "@/shared/i18n";
import { afterEach, beforeEach, expect, vi } from "vitest";

import ConnectionInspector from "./ConnectionInspector";
import { testUuid } from "@/test/ids";
import type * as ConnectionsApi from "@/features/connections/api";

const hostMetricsPreviewMock = vi.hoisted(() =>
  vi.fn((_props: { enabled?: boolean }) => null),
);
const playbookApiMock = vi.hoisted(() => ({
  getTargetPlaybookAssignments: vi.fn(),
}));
const launchOptionsMock = vi.hoisted(() => vi.fn());

vi.mock("@/features/connections/api", async (importOriginal) => ({
  ...await importOriginal<typeof ConnectionsApi>(),
  getConnectionLaunchOptions: launchOptionsMock,
}));

vi.mock("@/features/observability/HostMetricsPreviewSection", () => ({
  default: (props: { enabled?: boolean }) => {
    hostMetricsPreviewMock(props);
    return null;
  },
}));

vi.mock("@/features/playbooks/api", () => ({
  TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT: 20,
  getTargetPlaybookAssignments: playbookApiMock.getTargetPlaybookAssignments,
}));

// ConnectButtons (rendered by ConnectionInspector) gates on CONNECT; grant all
// capabilities so these tests see the connect affordances.
vi.mock("@/shared/auth/permissionsContext", () => ({
  usePermissions: () => ({
    roles: [],
    isAdmin: true,
    can: () => true,
    canAny: () => true,
    hasRole: () => false,
  }),
}));

async function advanceDeferredInspectorQueries() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(100);
  });
}

function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

describe("ConnectionInspector", () => {
  beforeEach(() => {
    window.localStorage.clear();
    vi.useFakeTimers();
    hostMetricsPreviewMock.mockClear();
    playbookApiMock.getTargetPlaybookAssignments.mockReset();
    playbookApiMock.getTargetPlaybookAssignments.mockResolvedValue({ target: {}, items: [] });
    launchOptionsMock.mockReset();
  });

  it("shows authoritative connection controls while the expanded jobs query is unresolved", async () => {
    playbookApiMock.getTargetPlaybookAssignments.mockReturnValue(new Promise(() => {}));
    launchOptionsMock.mockResolvedValue({ hostId: "host-1", protocols: [{ protocol: "ssh", render: "buttons", accounts: [{ entryId: "ssh-root", kind: "shared", label: "root", source: {}, isDefault: true, requiresInput: false }] }] });
    const onConnect = vi.fn();
    const host = { id: testUuid("host-1"), name: "host", targetKind: "device" as const, status: "active" as const };
    renderWithClient(<ConnectionInspector host={host} onClose={() => {}} onConnect={onConnect} onPlaybookQuickAction={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Jobs" }));
    fireEvent.click(screen.getByRole("button", { name: "Connections" }));
    await advanceDeferredInspectorQueries();
    expect(playbookApiMock.getTargetPlaybookAssignments).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: /SSH root/i }));
    expect(onConnect).toHaveBeenCalledWith(host, "ssh", "ssh-root", undefined);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders the shell without fetching collapsed sections", async () => {
    renderWithClient(
      <ConnectionInspector
        host={{
          id: testUuid("host-1"),
          name: "pxm-piping01",
          targetKind: "virtual_machine",
          status: "active",
        }}
        onClose={() => {}}
        onConnect={() => {}}
      />,
    );

    expect(screen.getByRole("heading", { name: "pxm-piping01" })).toBeInTheDocument();
    expect(hostMetricsPreviewMock).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Playbooks" })).not.toBeInTheDocument();

    await advanceDeferredInspectorQueries();

    expect(hostMetricsPreviewMock).not.toHaveBeenCalled();
    expect(playbookApiMock.getTargetPlaybookAssignments).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Playbooks" })).not.toBeInTheDocument();
  });

  it("shows eligible playbook buttons in their own expanded section", async () => {
    playbookApiMock.getTargetPlaybookAssignments.mockResolvedValue({
      target: {},
      items: [
        {
          assignment: {
            id: "assignment-1",
            name: "Restart service",
            scope_kind: "connection_target",
            enabled: true,
            executable: true,
            job_template_id: "job-1",
          },
          job_template: { id: "job-1", name: "Linux service restart", inventory: { kind: "managed", target_set_id: "set-1" } },
        },
      ],
    });

    renderWithClient(
      <ConnectionInspector
        host={{
          id: testUuid("host-1"),
          name: "pxm-piping01",
          targetKind: "virtual_machine",
          status: "active",
        }}
        onClose={() => {}}
        onConnect={() => {}}
        onPlaybookQuickAction={vi.fn()}
      />,
    );

    await advanceDeferredInspectorQueries();

    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: "Jobs" })).toBeInTheDocument();
    });

    const sectionButton = screen.getByRole("button", { name: "Jobs" });
    expect(sectionButton).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(sectionButton);
    await advanceDeferredInspectorQueries();
    const connectionsButton = screen.getByRole("button", { name: "Connections" });
    expect(connectionsButton).toBeInTheDocument();

    const connectionsSection = connectionsButton.parentElement?.parentElement;
    const playbooksSection = sectionButton.parentElement?.parentElement;
    if (!connectionsSection || !playbooksSection) {
      throw new Error("Inspector section wrapper did not render");
    }

    expect(within(playbooksSection).getByText("Restart service")).toBeInTheDocument();
    expect(within(connectionsSection).queryByText("Restart service")).not.toBeInTheDocument();
  });
});
