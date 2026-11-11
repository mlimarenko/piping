import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import ServiceDiscoveryScreen from "./ServiceDiscoveryScreen";
import type * as Api from "./api";

const api = vi.hoisted(() => ({
  listAllProfileAssignments: vi.fn(),
  listAllProfiles: vi.fn(),
  createProfileAssignment: vi.fn(),
  deleteProfileAssignment: vi.fn(),
}));
vi.mock("./api", async original => ({ ...await original<typeof Api>(), ...api }));
// Scope dictionaries are resolved by their own component test; this screen test
// only needs a stable target to bind the new scope to.
vi.mock("./useScopeEntities", () => ({
  useScopeEntities: () => ({
    optionsByScope: { role: [], connection_target: [], site: [{ id: "site-1", name: "Site One" }], tenant: [] },
    resolveName: (scope: string, id: string | null) => scope === "global" ? "All matching targets" : `Synthetic ${id}`,
    isPending: false,
  }),
}));

const profile = (id: string, name: string) => ({
  id, slug: id, name, enabled: true, exporter_type: "prometheus", default_job: id,
  default_metrics_path: "/metrics", default_scheme: "http", default_target_source: "host",
  labels: {}, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
});

const assignment = (overrides: Record<string, unknown> = {}) => ({
  id: "assign-1", profile_id: "node", profile_slug: "node",
  profile_name: "Node exporter", scope_type: "global", scope_id: null, enabled: true,
  weight: 0, target: null, created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z",
  ...overrides,
});

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter><ServiceDiscoveryScreen /></MemoryRouter></AllPermissions></QueryClientProvider>);
  return client;
}

async function chooseScope(dialog: HTMLElement, option: string) {
  fireEvent.click(within(dialog).getByRole("combobox", { name: /^scope$/i }));
  fireEvent.click(await screen.findByRole("option", { name: option }));
}

beforeEach(async () => {
  vi.clearAllMocks();
  await i18n.changeLanguage("en");
  api.listAllProfileAssignments.mockResolvedValue([assignment()]);
  api.listAllProfiles.mockResolvedValue([]);
  api.createProfileAssignment.mockImplementation((body: Record<string, unknown>) => Promise.resolve(assignment({ id: `created-${String(body.profile_id)}`, ...body })));
  api.deleteProfileAssignment.mockResolvedValue(undefined);
});

it("does not offer the priority column the backend ignores", async () => {
  mount();
  await screen.findByRole("link", { name: "All matching targets" });
  expect(screen.queryByRole("columnheader", { name: /^priority$/i })).not.toBeInTheDocument();
});

it("creates one assignment for exactly the selected exporter profiles", async () => {
  api.listAllProfiles.mockResolvedValue([profile("node", "Node exporter"), profile("nginx", "Nginx exporter"), profile("blackbox", "Blackbox exporter")]);
  mount();
  await screen.findByRole("link", { name: "All matching targets" });
  fireEvent.click(screen.getByRole("button", { name: /^add assignments$/i }));
  const dialog = await screen.findByRole("dialog");

  await chooseScope(dialog, "Site");
  fireEvent.click(await within(dialog).findByRole("combobox", { name: /^target/i }));
  fireEvent.click(await within(dialog).findByText("Site One"));

  const profiles = within(dialog).getByRole("combobox", { name: "Exporter profiles" });
  fireEvent.click(profiles);
  fireEvent.click(await within(dialog).findByText("Node exporter"));
  fireEvent.click(within(dialog).getByText("Nginx exporter"));
  fireEvent.click(profiles);

  fireEvent.click(within(dialog).getByRole("button", { name: "Create 2 assignments" }));

  await waitFor(() => expect(api.createProfileAssignment).toHaveBeenCalledTimes(2));
  const bodies = api.createProfileAssignment.mock.calls.map(call => call[0] as Record<string, unknown>);
  expect(bodies.map(body => body.profile_id).sort()).toEqual(["nginx", "node"]);
  for (const body of bodies) {
    expect(body).toMatchObject({ scope_type: "site", scope_id: "site-1", enabled: true });
    expect(body).not.toHaveProperty("weight");
  }
  expect(bodies.some(body => body.profile_id === "blackbox")).toBe(false);
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(api.listAllProfileAssignments).toHaveBeenCalledTimes(2);
});

it("keeps the dialog open and names the profiles the backend rejected", async () => {
  api.listAllProfiles.mockResolvedValue([profile("node", "Node exporter"), profile("nginx", "Nginx exporter")]);
  api.createProfileAssignment.mockImplementation((body: Record<string, unknown>) =>
    body.profile_id === "nginx" ? Promise.reject(new Error("synthetic rejection")) : Promise.resolve(assignment({ ...body })));
  mount();
  await screen.findByRole("link", { name: "All matching targets" });
  fireEvent.click(screen.getByRole("button", { name: /^add assignments$/i }));
  const dialog = await screen.findByRole("dialog");

  const profiles = within(dialog).getByRole("combobox", { name: "Exporter profiles" });
  fireEvent.click(profiles);
  fireEvent.click(await within(dialog).findByText("Node exporter"));
  fireEvent.click(within(dialog).getByText("Nginx exporter"));
  fireEvent.click(profiles);
  fireEvent.click(within(dialog).getByRole("button", { name: "Create 2 assignments" }));

  const alert = await within(dialog).findByRole("alert");
  expect(alert).toHaveTextContent("nginx");
  expect(alert).not.toHaveTextContent("node");
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  expect(api.listAllProfileAssignments).toHaveBeenCalledTimes(2);
});

it("clones an assignment into another scope with its assignment values intact", async () => {
  api.listAllProfileAssignments.mockResolvedValue([assignment({
    scope_type: "site", scope_id: "site-1", profile_id: "node", profile_slug: "node",
    profile_name: "Node exporter", target: "edge.test", weight: 7, enabled: false,
  })]);
  mount();
  await screen.findByRole("link", { name: "Synthetic site-1" });
  fireEvent.pointerDown(screen.getByRole("button", { name: /actions:/i }), { button: 0, ctrlKey: false });
  fireEvent.click(await screen.findByRole("menuitem", { name: /^clone$/i }));

  const dialog = await screen.findByRole("dialog");
  expect(within(dialog).getByText(/Node exporter \(node\)/)).toBeInTheDocument();
  await chooseScope(dialog, "Global");
  fireEvent.click(within(dialog).getByRole("button", { name: "Create copy" }));

  await waitFor(() => expect(api.createProfileAssignment).toHaveBeenCalledTimes(1));
  const body = api.createProfileAssignment.mock.calls[0]![0] as Record<string, unknown>;
  expect(body).toMatchObject({
    profile_id: "node", enabled: false, target: "edge.test", weight: 7,
    scope_type: "global", scope_id: null,
  });
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(api.listAllProfileAssignments).toHaveBeenCalledTimes(2);
});
