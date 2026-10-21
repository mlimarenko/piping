import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AllPermissions } from "@/test/permissions";
import type { ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import HostPlaybooksPreviewSection from "./HostPlaybooksPreviewSection";

const playbookApiMock = vi.hoisted(() => ({
  getTargetPlaybookAssignments: vi.fn(),
}));

vi.mock("@/features/playbooks/api", () => ({
  TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT: 20,
  getTargetPlaybookAssignments: playbookApiMock.getTargetPlaybookAssignments,
}));

function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>{ui}</AllPermissions>
    </QueryClientProvider>,
  );
}

describe("HostPlaybooksPreviewSection", () => {
  beforeEach(() => {
    playbookApiMock.getTargetPlaybookAssignments.mockReset();
    playbookApiMock.getTargetPlaybookAssignments.mockResolvedValue({
      target: {
        target_id: "target-1",
        target_kind: "connection_target",
        source_id: "device-1",
        source_kind: "device",
        name: "pxm-piping01",
      },
      items: [
        {
          assignment: {
            id: "assignment-1",
            name: "Restart service",
            scope_kind: "connection_target",
            executable: true,
            enabled: true,
          },
          playbook: {
            playbook: {
              id: "playbook-1",
              name: "Linux service restart",
              risk: "low",
              status: "active",
              description: "Restart a selected system service",
            },
            current_version: {
              id: "version-1",
            },
          },
        },
      ],
    });
  });

  it("renders inspector layout as a compact launch button", async () => {
    const onLaunch = vi.fn();

    renderWithClient(
      <HostPlaybooksPreviewSection
        host={{ id: "host-1" }}
        layout="buttons"
        onLaunch={onLaunch}
      />,
    );

    const button = await screen.findByRole("button", { name: "Restart service" });
    expect(button).toBeInTheDocument();
    expect(screen.queryByText("Restart a selected system service")).not.toBeInTheDocument();

    fireEvent.click(button);

    expect(onLaunch).toHaveBeenCalledTimes(1);
    expect(onLaunch).toHaveBeenCalledWith(
      expect.objectContaining({
        assignment: expect.objectContaining({ id: "assignment-1" }),
      }),
    );
  });

  it("disables compact launch buttons for non-runnable playbooks", async () => {
    playbookApiMock.getTargetPlaybookAssignments.mockResolvedValue({
      target: {
        target_id: "target-1",
        target_kind: "connection_target",
        source_id: "device-1",
        source_kind: "device",
        name: "pxm-piping01",
      },
      items: [
        {
          assignment: {
            id: "assignment-1",
            name: "Draft maintenance",
            scope_kind: "connection_target",
            executable: false,
            enabled: true,
          },
          playbook: {
            playbook: {
              id: "playbook-1",
              name: "Draft maintenance",
              risk: "medium",
              status: "draft",
              description: "Not ready to run",
            },
            current_version: null,
          },
        },
      ],
    });

    const onLaunch = vi.fn();

    renderWithClient(
      <HostPlaybooksPreviewSection
        host={{ id: "host-1" }}
        layout="buttons"
        onLaunch={onLaunch}
      />,
    );

    const button = await screen.findByRole("button", { name: "Draft maintenance" });
    expect(button).toBeDisabled();

    fireEvent.click(button);

    expect(onLaunch).not.toHaveBeenCalled();
  });
});
