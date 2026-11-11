import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import MonitoringSection from "./MonitoringSection";
import { collectorTargetsOf } from "./coverage";
import type * as Api from "./api";
import type { MonitoringCoverageResponse } from "./api";

const api = vi.hoisted(() => ({
  getMonitoringCoverage: vi.fn(),
  listAllProfiles: vi.fn(),
  createProfileAssignment: vi.fn(),
  deleteProfileAssignment: vi.fn(),
}));
vi.mock("./api", async (original) => ({ ...(await original<typeof Api>()), ...api }));

const coverage: MonitoringCoverageResponse = {
  hosts: [
    {
      host_id: "host-1",
      host_name: "db-01",
      kind: "device",
      status: "active",
      state: "covered" as const,
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
      kind: "device",
      status: "active",
      state: "no_scrape" as const,
      reason: "disabled_assignment" as const,
      scrapes: 0,
      targets: [],
    },
  ],
  totals: { hosts_total: 2, covered: 1, no_scrape: 1, duplicate: 0, filtered: 0 },
  assignments: [
    {
      assignment_id: "assignment-1",
      name: "Node Exporter — Linux servers",
      profile_slug: "node",
      scope_type: "role",
      scope_id: "role-1",
      matched_hosts: 1,
      targets: 1,
      duplicates: 0,
    },
  ],
  duplicates: [],
  total: 2,
  limit: 200,
  offset: 0,
};

function mount(canWrite = true) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const can = (action: string) =>
    action === "READ" ? true : canWrite && ["CREATE", "DELETE", "UPDATE"].includes(action);
  return render(
    <QueryClientProvider client={client}>
      <PermissionsContext.Provider
        value={{ roles: [], isAdmin: false, can, canAny: () => false, hasRole: () => false }}
      >
        <MemoryRouter>
          <MonitoringSection subject={{ kind: "role", id: "role-1", label: "Linux servers" }} />
        </MemoryRouter>
      </PermissionsContext.Provider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  api.getMonitoringCoverage.mockResolvedValue(coverage);
  api.listAllProfiles.mockResolvedValue([
      {
        id: "exporter-1",
        name: "Node Exporter",
        slug: "node",
        exporter_type: "node",
        default_job: "node",
        default_scheme: "http",
        default_port: 9100,
        default_metrics_path: "/metrics",
        default_target_source: "primary_ip",
        labels: {},
        enabled: true,
      },
  ]);
  api.createProfileAssignment.mockResolvedValue({ id: "assignment-2" });
  api.deleteProfileAssignment.mockResolvedValue(undefined);
});

it("shows effective scrapes with the assignment that produced each target", async () => {
  mount();

  expect(await screen.findByText("db-01")).toBeVisible();
  expect(screen.getByText("node · 10.0.0.10:9100 · Node Exporter — Linux servers")).toBeVisible();
  expect(screen.getByText("router-01")).toBeVisible();
  expect(screen.getByText(/matching assignment is disabled/i)).toBeVisible();
});

it("shows the profile slug when an assignment has no name", async () => {
  api.getMonitoringCoverage.mockResolvedValue({
    ...coverage,
    assignments: [{ ...coverage.assignments[0], name: null, profile_slug: "linux-profile" }],
  });
  mount();

  expect(await screen.findByText("linux-profile")).toBeVisible();
});

it("creates an assignment at the object scope from the exporter catalog", async () => {
  mount();

  fireEvent.click(await screen.findByRole("button", { name: /add scrape/i }));
  const dialog = await screen.findByRole("dialog");
  fireEvent.click(within(dialog).getByRole("combobox"));
  fireEvent.click(await screen.findByRole("option", { name: /node exporter/i }));
  fireEvent.click(within(dialog).getByRole("button", { name: /^add scrape$/i }));

  await waitFor(() => expect(api.createProfileAssignment).toHaveBeenCalledTimes(1));
  expect(api.createProfileAssignment).toHaveBeenCalledWith(
    expect.objectContaining({
      profile_id: "exporter-1",
      scope_type: "role",
      scope_id: "role-1",
      enabled: true,
    }),
  );
}, 15000);

it("removes an assignment after confirmation", async () => {
  mount();

  fireEvent.click(await screen.findByRole("button", { name: /^delete$/i }));
  const dialog = await screen.findByRole("alertdialog");
  fireEvent.click(within(dialog).getByRole("button", { name: /^delete$/i }));

  await waitFor(() => expect(api.deleteProfileAssignment).toHaveBeenCalledWith("assignment-1"));
});

it("offers an enabled profile beyond the first server page", async () => {
  api.listAllProfiles.mockResolvedValue(Array.from({length:51},(_,index)=>({id:`profile-${index}`,name:`Profile ${index}`,slug:`profile-${index}`,enabled:true,items:[],item_count:1,assignment_count:0})));
  mount();
  fireEvent.click(await screen.findByRole("button",{name:/add scrape/i}));
  const dialog=await screen.findByRole("dialog");
  fireEvent.click(within(dialog).getByRole("combobox"));
  fireEvent.click(await screen.findByRole("option",{name:"Profile 50"}));
  fireEvent.click(within(dialog).getByRole("button",{name:/^add scrape$/i}));
  await waitFor(()=>expect(api.createProfileAssignment).toHaveBeenCalledWith(expect.objectContaining({profile_id:"profile-50"})));
  expect(api.listAllProfiles).toHaveBeenCalledWith({enabled:"true"});
});

it("hides add and remove when the operator cannot write monitoring configuration", async () => {
  mount(false);

  expect(await screen.findByText("db-01")).toBeVisible();
  expect(screen.queryByRole("button", { name: /add scrape/i })).toBeNull();
  expect(screen.queryByRole("button", { name: /^delete$/i })).toBeNull();
});

it("offers the collector artifact for the resolved targets", async () => {
  expect(collectorTargetsOf(coverage)).toHaveLength(1);
  mount();

  expect(await screen.findByRole("heading", { name: /collector configuration/i })).toBeVisible();
  expect(screen.getByText(/relabel_configs/)).toBeVisible();
}, 15000);
