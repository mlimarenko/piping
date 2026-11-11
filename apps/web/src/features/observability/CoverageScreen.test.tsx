import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import CoverageScreen from "./CoverageScreen";
import type * as Api from "./api";
import type { MonitoringCoverageResponse } from "./api";

const api = vi.hoisted(() => ({ getMonitoringCoverage: vi.fn() }));
vi.mock("./api", async (original) => ({ ...(await original<typeof Api>()), ...api }));

const coverage: MonitoringCoverageResponse = {
  hosts: [
    {
      host_id: "host-1",
      host_name: "db-01",
      kind: "device",
      status: "active",
      state: "covered",
      reason: null,
      scrapes: 1,
      targets: [
        {
          assignment_id: "assignment-1",
          assignment_name: "Node Exporter — Linux servers",
          exporter_slug: "node",
          assignment_scope_type: "role",
          assignment_scope_id: "role-1",
          job: "node",
          target: "10.0.0.10:9100",
          labels: { __scheme__: "http", __metrics_path__: "/metrics", piping_job: "node" },
        },
      ],
    },
    {
      host_id: "host-2",
      host_name: "router-01",
      kind: "virtual_machine",
      status: "active",
      state: "duplicate",
      reason: null,
      scrapes: 2,
      targets: [],
    },
    {
      host_id: "host-3",
      host_name: "legacy-01",
      kind: "device",
      status: "active",
      state: "no_scrape",
      reason: "no_assignment",
      scrapes: 0,
      targets: [],
    },
    {
      host_id: "host-4",
      host_name: "retired-01",
      kind: "device",
      status: "retired",
      state: "filtered",
      reason: "status_filtered",
      scrapes: 0,
      targets: [],
    },
  ],
  totals: { hosts_total: 4, covered: 1, no_scrape: 1, duplicate: 1, filtered: 1 },
  assignments: [],
  duplicates: [],
  total: 4,
  limit: 200,
  offset: 0,
};

function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <MemoryRouter>
          <CoverageScreen />
        </MemoryRouter>
      </AllPermissions>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  api.getMonitoringCoverage.mockResolvedValue(coverage);
});

it("shows every active host with state, reason and the assignment behind a target", async () => {
  mount();

  expect(await screen.findByText("db-01")).toBeVisible();
  expect(screen.getByText("router-01")).toBeVisible();
  expect(screen.getByText("legacy-01")).toBeVisible();
  expect(screen.getByText(/no enabled assignment matches this object/i)).toBeVisible();
  expect(
    screen.getByText("node · Node Exporter — Linux servers · 10.0.0.10:9100"),
  ).toBeVisible();
});

it("filters the fleet by a coverage state and resets on the total tile", async () => {
  mount();

  expect(await screen.findByText("db-01")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /duplicate target/i }));
  await waitFor(() => expect(screen.queryByText("db-01")).not.toBeInTheDocument());
  expect(screen.getByText("router-01")).toBeVisible();

  fireEvent.click(screen.getByRole("button", { name: /active hosts/i }));
  expect(await screen.findByText("db-01")).toBeVisible();
});

it("links each host to the inventory page of its kind", async () => {
  mount();

  expect(await screen.findByRole("link", { name: /db-01/i })).toHaveAttribute(
    "href",
    "/inventory/resources/Devices/host-1",
  );
  expect(screen.getByRole("link", { name: /router-01/i })).toHaveAttribute(
    "href",
    "/inventory/resources/Virtual%20Machines/host-2",
  );
});

it("offers a retry when coverage cannot be read", async () => {
  api.getMonitoringCoverage.mockRejectedValueOnce(new Error("boom"));
  mount();

  expect(await screen.findByText(/could not load host coverage/i)).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /retry/i }));
  await waitFor(() => expect(api.getMonitoringCoverage).toHaveBeenCalledTimes(2));
  expect(await screen.findByText("db-01")).toBeVisible();
});
