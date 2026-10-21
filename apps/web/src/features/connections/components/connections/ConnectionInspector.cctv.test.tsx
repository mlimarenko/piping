import { act, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import type { ReactElement } from "react";
import "@/shared/i18n";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import ConnectionInspector from "./ConnectionInspector";
import { testUuid } from "@/test/ids";
import type { ConnectionTarget } from "@/shared/api/types";

// Keep the hidden metric/playbook probes inert so the footer reflects only what
// is under test.
vi.mock("@/features/observability/HostMetricsPreviewSection", () => ({
  default: () => null,
}));

vi.mock("@/features/playbooks/api", () => ({
  TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT: 20,
  getTargetPlaybookAssignments: vi.fn().mockResolvedValue({ target: {}, items: [] }),
}));

vi.mock("@/shared/auth/permissionsContext", () => ({
  usePermissions: () => ({
    roles: [],
    isAdmin: true,
    can: () => true,
    canAny: () => true,
    hasRole: () => false,
  }),
}));

function renderInspector(host: ConnectionTarget) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>{ui(host)}</MemoryRouter>
    </QueryClientProvider>,
  );
}

function ui(host: ConnectionTarget): ReactElement {
  return <ConnectionInspector host={host} onClose={() => {}} onConnect={() => {}} />;
}

async function flushDeferred() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(100);
  });
}

const cctvHost: ConnectionTarget = {
  id: testUuid("device-1"),
  name: "nvr-1",
  targetKind: "device",
  status: "active",
  isCctv: true,
};

describe("ConnectionInspector CCTV device", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  // The CCTV console is opened from the connect panel (the `CCTV` launch button),
  // like SSH/RDP/VNC — the legacy footer "Manage"/"View" affordances were removed
  // so the device has one consistent open flow. Guard against their return.
  it("does not render the removed Manage / View footer actions", async () => {
    renderInspector(cctvHost);
    await flushDeferred();

    expect(screen.queryByRole("link", { name: "Manage" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "View" })).not.toBeInTheDocument();
  });
});
