import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import "@/shared/i18n";
import { afterEach, beforeEach, expect, vi } from "vitest";

import ConnectionInspector from "./ConnectionInspector";
import { testUuid } from "@/test/ids";

const hostMetricsPreviewMock = vi.hoisted(() =>
  vi.fn((_props: { enabled?: boolean }) => null),
);
const playbookApiMock = vi.hoisted(() => ({
  getTargetPlaybookAssignments: vi.fn(),
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
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("renders the shell before enabling the hidden probes", async () => {
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
    expect(hostMetricsPreviewMock).toHaveBeenLastCalledWith(
      expect.objectContaining({ enabled: false }),
    );
    expect(screen.queryByRole("button", { name: "Playbooks" })).not.toBeInTheDocument();

    await advanceDeferredInspectorQueries();

    expect(hostMetricsPreviewMock).toHaveBeenLastCalledWith(
      expect.objectContaining({ enabled: true }),
    );
    await vi.waitFor(() => {
      expect(playbookApiMock.getTargetPlaybookAssignments).toHaveBeenCalledWith("host-1", 20);
    });
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
          },
          playbook: {
            playbook: {
              name: "Linux service restart",
              risk: "low",
              description: "Restart a selected system service",
            },
          },
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
      />,
    );

    await advanceDeferredInspectorQueries();

    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: "Playbooks" })).toBeInTheDocument();
    });

    const sectionButton = screen.getByRole("button", { name: "Playbooks" });
    expect(sectionButton).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(sectionButton);
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
