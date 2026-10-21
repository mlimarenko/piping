import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import type { CurrentUser } from "@/features/auth/api";
import type * as ReactRouter from "react-router";
import App from "./App";
import type { QueryClient } from "@tanstack/react-query";
import type * as ProxiesApi from "@/features/connections/proxies-api";
import type * as InventoryApi from "@/features/inventory/api";
import type * as ConnectionsApi from "@/features/connections/api";
import type * as ObservabilityApi from "@/features/observability/api";
import type * as PlaybooksApi from "@/features/playbooks/api";
import type * as UserSettingsApi from "@/features/user-settings/api";
import type * as AuthApi from "@/features/auth/api";
import type * as MappingApi from "@/features/inventory/sync-mapping-api";
import type * as SyncApi from "@/features/inventory/sync-api";
import type * as SecretsApi from "@/features/automation-secrets/api";
import type * as CustomFieldsApi from "@/features/inventory/custom-fields-api";

// Settable initial route for the mocked BrowserRouter; each test mounts a fresh
// MemoryRouter so a change here takes effect on the next render.
const router = vi.hoisted(() => ({ initialPath: "/connections/config", user: undefined as CurrentUser | undefined,
  client: undefined as QueryClient | undefined, state: "loaded" }));
const customFieldsApi = vi.hoisted(() => ({
  listCustomFields: vi.fn(() => Promise.resolve({ items: [], total: 0, limit: 50, offset: 0 })),
  listChoiceSets: vi.fn(() => Promise.resolve({ items: [], total: 0, limit: 50, offset: 0 })),
  listContentTypes: vi.fn(() => Promise.resolve([])),
}));

vi.mock("@/features/inventory/custom-fields-api", async original => ({
  ...await original<typeof CustomFieldsApi>(), ...customFieldsApi,
}));

function recordResponse<T>(value: T): Promise<T | null> {
  if (router.state === "pending") return new Promise(() => {});
  if (router.state === "error") return Promise.reject(Object.assign(new Error("Unavailable"), { status: 400 }));
  return Promise.resolve(router.state === "missing" ? null : value);
}

vi.mock("@/features/connections/proxies-api", async original => ({
  ...await original<typeof ProxiesApi>(),
  listProxyNodes: () => {
    if (router.state === "pending") return new Promise(() => {});
    if (router.state === "error") return Promise.reject(Object.assign(new Error("Unavailable"), { status: 400 }));
    return Promise.resolve({ items: router.state === "missing" ? [] : [{ id: "node-1", name: "Proxy", slug: "proxy", base_url: "https://proxy.test", enabled: true }], total: 1 });
  },
  listProxyNodeAssignments: () => Promise.resolve({ items: [], total: 0 }),
}));
vi.mock("@/features/inventory/api", async original => ({
  ...await original<typeof InventoryApi>(),
  listResources: () => Promise.resolve({ items: [], total: 0 }),
  listAllResources: () => Promise.resolve([]),
  getResource: () => recordResponse({ id: "site-1", name: "Loaded site", type: "site" }),
  getResourceWithRaw: () => recordResponse({ resource: { id: "site-1", name: "Loaded site", type: "site" }, raw: { id: "site-1", name: "Loaded site", saved_count: 0 } }),
  getTemplate: () => recordResponse({ id: "template-1", name: "Loaded template", slug: "loaded", status: "active", description: "", variables: [], steps: [] }),
  getDataflow: () => recordResponse({ id: "flow-1", name: "Loaded flow", source: { object_type: "all", address_mode: "all" }, destination: { object_type: "all", address_mode: "all" }, destination_ports: [] }),
}));
vi.mock("@/features/inventory/sync-api", async original => ({
  ...await original<typeof SyncApi>(), listProviders: () => Promise.resolve({ items: [], total: 0 }),
}));
vi.mock("@/features/inventory/sync-mapping-api", async original => ({
  ...await original<typeof MappingApi>(),
  getMappingConfiguration: () => recordResponse({ id: "mapping-1", logical_id: "logical-1", name: "Inventory pull", version: 1, is_latest: true, provider_id: "provider-1", target: { kind: "local_inventory" }, status: "active", direction: "pull", conflict_mode: "netbox_wins", entity_mappings: [], created_at: "2026-01-01T00:00:00Z", in_flight_job_count: 0 }),
  listOperators: () => Promise.resolve([]),
}));
vi.mock("@/features/automation-secrets/api", async original => ({
  ...await original<typeof SecretsApi>(), listAutomationSecrets: () => Promise.resolve([]),
}));
vi.mock("@/features/connections/api", async original => ({
  ...await original<typeof ConnectionsApi>(),
  getConnectionTarget: () => new Promise(() => {}),
  searchConnectionTargets: () => Promise.resolve({ items: [], total: 0 }),
  listConnectionTargets: () => Promise.resolve({ items: [], total: 0 }),
}));
vi.mock("@/features/observability/api", async original => ({
  ...await original<typeof ObservabilityApi>(),
  listQuerySources: () => Promise.resolve({ items: [], total: 0 }),
  listMetricQueries: () => Promise.resolve({ items: [], total: 0 }),
  listMetricSets: () => Promise.resolve({ items: [], total: 0 }),
  listAlertSources: () => Promise.resolve({ items: [], total: 0 }),
  listAllExporterAssignments: () => Promise.resolve([]),
  listExporters: () => Promise.resolve({ items: [] }),
}));
vi.mock("@/features/playbooks/api", async original => ({
  ...await original<typeof PlaybooksApi>(),
  listPlaybooks: () => Promise.resolve({ items: [], total: 0 }),
  listProjects: () => Promise.resolve({ items: [], total: 0 }),
  listPlaybookTargetSets: () => Promise.resolve({ items: [], total: 0 }),
  listExecutionEnvironments: () => Promise.resolve({ items: [], total: 0 }),
  listPlaybookServicePrincipals: () => Promise.resolve({ items: [], total: 0 }),
  getPlaybook: () => recordResponse({ playbook: { id: "11111111-1111-4111-8111-111111111111", name: "Loaded playbook", source_kind: "in_app", runtime: "ansible", vars_schema: {}, metadata: {} }, current_version: null }),
  getTargetSet: () => recordResponse({ id: "target-1", name: "Loaded target", mode: "dynamic", selector: {} }),
  getSchedule: () => recordResponse({ id: "schedule-1", name: "Loaded schedule", playbook_id: "p", target_set_id: "t", service_principal_id: "sp", cron_expression: "0 * * * *", timezone: "UTC", concurrency_policy: "allow", enabled: true }),
  getProject: () => recordResponse({ id: "project-1", name: "Loaded project", scm_type: "manual" }),
  getProjectFiles: () => recordResponse({ files: [] }),
}));
vi.mock("@/features/user-settings/api", async original => ({
  ...await original<typeof UserSettingsApi>(),
  listMyCredentials: () => Promise.resolve([]),
  listMyTokens: () => Promise.resolve({ items: [], total: 0 }),
  listTokenScopes: () => Promise.resolve([]),
}));
vi.mock("@/features/auth/api", async original => ({
  ...await original<typeof AuthApi>(),
  me: () => Promise.resolve({ id: "admin", username: "admin", roles: ["admin"], capabilities: [] }),
}));

vi.mock("react-router", async (orig) => {
  const actual = await orig<typeof ReactRouter>();
  return {
    ...actual,
    BrowserRouter: ({ children }: { children: ReactNode }) => (
      <actual.MemoryRouter initialEntries={[router.initialPath]}>{children}</actual.MemoryRouter>
    ),
  };
});

// Minimal shell: the real geometry under test is the App route tree, not the
// authenticated shell chrome. Allow-all permissions so every route gate passes.
vi.mock("@/app/layout/AppShell", async () => {
  const { Outlet } = await import("react-router");
  const { PermissionsContext, buildPermissions } = await import("@/shared/auth/permissionsContext");
  const { PageChromeFixture } = await import("@/test/PageChromeFixture");
  const { useQueryClient } = await import("@tanstack/react-query");
  const allowAll = { roles: [], isAdmin: true, can: () => true, canAny: () => true, hasRole: () => true };
  return {
    default: function TestShell() {
      router.client = useQueryClient();
      return (
      <PermissionsContext.Provider value={router.user ? buildPermissions(router.user) : allowAll}>
        <PageChromeFixture>
          <main id="main-content">
            <Outlet />
          </main>
        </PageChromeFixture>
      </PermissionsContext.Provider>
      );
    },
  };
});

// Lazy leaves under test: stubs so the route tree renders without their data.
vi.mock("@/features/connections/connection-config/ConnectionConfig", () => ({
  default: () => <div data-testid="config-list" />,
}));
vi.mock("@/features/access-control/AccessControlWorkspace", () => ({
  default: () => <div data-testid="access-workspace">SSO workspace</div>,
}));
vi.mock("@/features/observability/ServiceDiscoveryScreen", () => ({ default: () => <div data-testid="service-discovery-screen">Scrape target table</div> }));
vi.mock("@/features/observability/InspectorMetricsScreen", () => ({ default: () => <div data-testid="inspector-metrics-screen">Inspector metric configuration</div> }));
vi.mock("@/features/observability/QuickStartWizard", () => ({ default: () => null }));
vi.mock("@/features/inventory/components/ipam-workspace/IpamWorkspace", () => ({ default: () => <div data-testid="ipam-workspace">IPAM prefix workspace</div> }));
vi.mock("@/features/inventory/components/cabling-workstation/CablingWorkstation", () => ({ default: () => <div data-testid="cabling-workspace">Cabling trace workspace</div> }));
// Group-list entry authorization is App-owned; feature queries are verified by
// the playbook owner, not replaced with a second capability model in this test.
vi.mock("@/features/playbooks/PlaybooksPage", () => ({ default: () => <div data-testid="automation-workspace" /> }));
vi.mock("@/features/connections/connection-config/ConnectionConfigDetail", () => ({
  default: () => <div data-testid="config-detail" />,
}));
vi.mock("@/features/connections/connection-config/ConnectionConfigEditor", async () => {
  const { default: EntityEditorPage } = await import("@/shared/ui-shared/EntityEditorPage");
  return {
    default: () => (
      <EntityEditorPage scope="routed" width="form" title="Editor" onBack={() => {}} backLabel="Back">
        <div>editor body</div>
      </EntityEditorPage>
    ),
  };
});

// App-level providers not under test: render children directly.
vi.mock("@/shared/hooks/use-theme", () => ({ ThemeProvider: ({ children }: { children: ReactNode }) => children }));
vi.mock("@/app/BrandingContext", () => ({ BrandingProvider: ({ children }: { children: ReactNode }) => children }));
vi.mock("@/app/desktop/DesktopShellFrame", () => ({ default: ({ children }: { children: ReactNode }) => children }));
vi.mock("@/features/auth/SessionRecovery", () => ({ default: () => null }));
vi.mock("@/shared/ui-primitives/toaster", () => ({ Toaster: () => null }));
vi.mock("@/shared/ui-primitives/sonner", () => ({ Toaster: () => null }));

beforeEach(() => {
  router.client?.clear();
  router.user = undefined;
  router.state = "loaded";
  for (const mock of Object.values(customFieldsApi)) mock.mockClear();
});

describe("feature-owned route geometry", () => {
  it("uses full definition width and keeps the same readable owner for choice sets", async () => {
    router.initialPath = "/inventory/custom-fields";
    const { container } = render(<App />);
    await screen.findByRole("tab", { name: "Definitions" });
    const owner = container.querySelector('[data-slot="page-layout"]');
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(owner).toHaveClass("max-w-none", "px-4", "md:px-6");
    fireEvent.keyDown(screen.getByRole("tab", { name: "Choice Sets" }), { key: "Enter" });
    await waitFor(() => expect(owner).toHaveClass("max-w-[1280px]"));
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(container.querySelector('[data-slot="page-layout"]')).toBe(owner);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it("keeps custom-field metadata behind READ INVENTORY inside readable denied geometry", async () => {
    router.initialPath = "/inventory/custom-fields";
    router.user = { id: "reader", username: "reader", email: "reader@example.test", roles: [], capabilities: [] };
    const { container } = render(<App />);
    await screen.findByText(/Your role doesn't grant|Ваша роль не предоставляет/i);
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(container.querySelector('[data-slot="page-layout"]')).toHaveClass("max-w-[1280px]", "px-4", "md:px-6");
    expect(customFieldsApi.listCustomFields).not.toHaveBeenCalled();
    expect(customFieldsApi.listChoiceSets).not.toHaveBeenCalled();
    expect(customFieldsApi.listContentTypes).not.toHaveBeenCalled();
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
  });

  it.each([
    ["/automation/runs", "max-w-none"],
    ["/playbooks", "max-w-[1280px]"],
  ])("retains the explicit role for %s", async (path, width) => {
    router.initialPath = path;
    const { container } = render(<App />);
    await screen.findByTestId("automation-workspace");
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(container.querySelector('[data-slot="page-layout"]')).toHaveClass(width, "px-4", "md:px-6");
  });

  it("gives both observability workbenches available width without replacing their geometry owner", async () => {
    router.initialPath = "/observability";
    const { container } = render(<App />);
    await screen.findByTestId("service-discovery-screen");
    const owner = container.querySelector('[data-slot="page-layout"]');
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(owner).toHaveClass("max-w-none", "px-4", "md:px-6", "h-full", "min-h-0");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);

    fireEvent.click(screen.getByRole("link", { name: /inspector metrics/i }));
    await screen.findByTestId("inspector-metrics-screen");
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(container.querySelector('[data-slot="page-layout"]')).toBe(owner);
    expect(owner).toHaveClass("max-w-none", "px-4", "md:px-6", "h-full", "min-h-0");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it.each([
    ["ipam-workspace", "max-w-none", "ipam-workspace"],
    ["cabling-workstation", "max-w-[1280px]", "cabling-workspace"],
    ["Sites", "max-w-[1280px]", undefined],
  ])("retains the feature-owned inventory role for %s", async (entity, width, workspace) => {
    router.initialPath = `/inventory/resources?entity=${entity}`;
    const { container } = render(<App />);
    if (workspace) await screen.findByTestId(workspace);
    else await screen.findByRole("heading", { level: 1, name: "Sites" });
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(container.querySelector('[data-slot="page-layout"]')).toHaveClass(width, "px-4", "md:px-6", "h-full", "min-h-0");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it("keeps inventory queries behind READ even when the IPAM workspace is selected", async () => {
    router.initialPath = "/inventory/resources?entity=ipam-workspace";
    router.user = { id: "reader", username: "reader", email: "reader@example.test", roles: [], capabilities: [] };
    const { container } = render(<App />);
    await screen.findByText(/Your role doesn't grant|Ваша роль не предоставляет/i);
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(container.querySelector('[data-slot="page-layout"]')).toHaveClass("max-w-[1280px]", "px-4", "md:px-6");
    expect(customFieldsApi.listCustomFields).not.toHaveBeenCalled();
    expect(customFieldsApi.listChoiceSets).not.toHaveBeenCalled();
    expect(customFieldsApi.listContentTypes).not.toHaveBeenCalled();
    expect(screen.queryByTestId("ipam-workspace")).not.toBeInTheDocument();
  });

  it("retains readable geometry and withholds both observability screens without READ MONITORING", async () => {
    router.initialPath = "/observability";
    router.user = { id: "reader", username: "reader", email: "reader@example.test", roles: [], capabilities: [] };
    const { container } = render(<App />);
    await screen.findByText(/Your role doesn't grant|Ваша роль не предоставляет/i);
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(container.querySelector('[data-slot="page-layout"]')).toHaveClass("max-w-[1280px]", "px-4", "md:px-6");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.queryByTestId("service-discovery-screen")).not.toBeInTheDocument();
    expect(screen.queryByTestId("inspector-metrics-screen")).not.toBeInTheDocument();
  });

  it("registers inventory saved detail with one readable-standard ancestor and one heading", async () => {
    router.initialPath = "/inventory/resources/Sites/site-1";
    const { container } = render(<App />);
    await screen.findByRole("heading", { level: 1, name: "Loaded site" });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    const layouts = container.querySelectorAll('[data-slot="page-layout"]');
    expect(layouts).toHaveLength(1);
    expect(layouts[0]).toHaveClass("max-w-[1280px]");
    expect(container.querySelector('[data-slot="entity-editor-page"]')).toBeNull();
  });
  it("denies inventory detail without READ INVENTORY", async () => {
    router.initialPath = "/inventory/resources/Sites/site-1";
    router.user = { id: "reader", username: "reader", email: "reader@example.test", roles: [], capabilities: [{ action: "READ", resourceType: "PLAYBOOK" }] };
    render(<App />);
    expect(await screen.findByText(/Your role doesn't grant|Ваша роль не предоставляет/i)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Loaded site" })).toBeNull();
  });
  describe.each([
    "/playbooks/book-1/edit", "/automation/target-sets/target-1/edit", "/automation/schedules/schedule-1/edit",
    "/automation/projects/project-1/files", "/inventory/resources/Sites/site-1/edit",
    "/inventory/templates/template-1/edit", "/inventory/dataflows/flow-1/edit", "/inventory/sync/mappings/mapping-1/edit",
  ])("completed state-shell family %s", path => {
    it.each(["pending", "error", "missing", "loaded"])("has one feature owner and heading in %s state", async state => {
      router.initialPath = path;
      router.state = state;
      const { container } = render(<App />);
      await screen.findByTestId("entity-editor-page");
      if (state !== "pending") await waitFor(() => expect(router.client?.isFetching()).toBe(0));
      expect(container.querySelectorAll('[data-slot="entity-editor-page"]')).toHaveLength(1);
      expect(container.querySelector('[data-slot="page-layout"]')).toBeNull();
      expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    });
  });

  it("keeps read-only mapping in one feature-owned standard layout", async () => {
    router.initialPath = "/inventory/sync/mappings/mapping-1/readonly";
    const { container } = render(<App />);
    await screen.findByRole("heading", { level: 1, name: "Inventory pull v1" });
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(container.querySelector('[data-slot="page-layout"]')).toHaveClass("max-w-[1280px]");
    expect(container.querySelector('[data-slot="entity-editor-page"]')).toBeNull();
  });

  it("registers the real playbook detail with one standard ancestor and one heading", async () => {
    router.initialPath = "/playbooks/11111111-1111-4111-8111-111111111111?section=targets";
    const { container } = render(<App />);
    await screen.findByRole("heading", { level: 1, name: "Loaded playbook" });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
    expect(container.querySelector('[data-slot="page-layout"]')).toHaveClass("max-w-[1280px]");
  });

  it.each(["PLAYBOOK_RUN_GROUP", "PLAYBOOK_RUN"] as const)("authorizes the global group entry with %s", async resourceType => {
    router.initialPath = "/automation/runs";
    router.user = { id: "reader", username: "reader", email: "reader@example.test", roles: [], capabilities: [{ action: "READ", resourceType }] };
    render(<App />);
    if (resourceType === "PLAYBOOK_RUN_GROUP") expect(await screen.findByTestId("automation-workspace")).toBeInTheDocument();
    else {
      expect(await screen.findByText(/Your role doesn't grant|Ваша роль не предоставляет/i)).toBeInTheDocument();
      expect(screen.queryByTestId("automation-workspace")).toBeNull();
    }
  });

  it.each([
    "/playbooks/new", "/automation/target-sets/new", "/automation/schedules/new",
    "/inventory/resources/Sites/new", "/inventory/templates/new", "/inventory/dataflows/new",
    "/inventory/sync/mappings/new", "/inventory/sync/provider-1/mappings/new",
    "/automation/execution-environments/new", "/automation/projects/new", "/automation/job-templates/new",
    "/user/settings/credentials/new", "/user/settings/tokens/new",
    "/connections/proxies/assignments/new", "/connections/proxies/assignments/missing/edit",
    "/observability/sources/new", "/observability/sources/missing/edit",
    "/observability/exporter-assignments/new", "/observability/exporter-assignments/missing/edit",
    "/observability/metrics/new", "/observability/metrics/missing/edit",
    "/observability/sets/new", "/observability/sets/missing/edit",
    "/observability/alert-sources/new", "/observability/alert-sources/missing/edit",
  ])("mounts the actual self-hosted family with exactly one owner: %s", async path => {
    router.initialPath = path;
    const { container } = render(<App />);
    await screen.findByTestId("entity-editor-page");
    await waitFor(() => expect(router.client?.isFetching()).toBe(0));
    expect(container.querySelector('[data-slot="page-layout"]')).toBeNull();
    expect(container.querySelectorAll('[data-slot="entity-editor-page"]')).toHaveLength(1);
  });

  it.each(["/mcp", "/administration/settings"])("does not nest the actual feature page layout: %s", async path => {
    router.initialPath = path;
    const { container } = render(<App />);
    await waitFor(() => expect(container.querySelector('[data-slot="page-layout"]')).not.toBeNull());
    expect(container.querySelectorAll('[data-slot="page-layout"]')).toHaveLength(1);
  });

  it("mounts the actual automation create editor without an ancestor page layout", async () => {
    router.initialPath = "/automation/service-principals/new";
    const { container } = render(<App />);
    const editor = await screen.findByTestId("entity-editor-page");
    expect(editor.parentElement?.closest('[data-slot="page-layout"]')).toBeNull();
    expect(container.querySelectorAll('[data-slot="entity-editor-page"]')).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Create" })).toBeDisabled();
  });

  it.each(["loaded", "pending", "error", "missing"] as const)("keeps proxy %s state in its single feature-owned editor", async state => {
    router.state = state;
    router.initialPath = "/connections/proxies/node-1/edit";
    const { container } = render(<App />);
    await screen.findByTestId("entity-editor-page");
    if (state === "loaded") await screen.findByDisplayValue("Proxy");
    if (state === "error") await screen.findByRole("button", { name: "Retry" });
    if (state === "missing") await screen.findByText(/not found/i);
    expect(container.querySelector('[data-slot="page-layout"]')).toBeNull();
    expect(container.querySelectorAll('[data-slot="entity-editor-page"]')).toHaveLength(1);
  });

  it("preserves inner proxy write denial without adding a second geometry owner", async () => {
    router.initialPath = "/connections/proxies/node-1/edit";
    router.user = { id: "reader", username: "reader", email: "reader@example.test", roles: [], capabilities: [{ action: "READ", resourceType: "PROXY_NODE" }] };
    const { container } = render(<App />);
    expect(await screen.findByText(/Your role doesn't grant|Ваша роль не предоставляет/i)).toBeInTheDocument();
    expect(container.querySelector('[data-slot="entity-editor-page"]')).toBeNull();
    expect(container.querySelector('[data-slot="page-layout"]')).toBeNull();
  });

  it.each(["/automation/service-principals/new", "/playbooks/11111111-1111-4111-8111-111111111111"])("does not mount privileged content for a denied deep link: %s", async path => {
    router.initialPath = path;
    router.user = { id: "reader", username: "reader", email: "reader@example.test", roles: [], capabilities: [{ action: "READ", resourceType: "INVENTORY" }] };
    render(<App />);
    expect(await screen.findByText(/Your role doesn't grant|Ваша роль не предоставляет/i)).toBeInTheDocument();
    expect(screen.queryByTestId("entity-editor-page")).toBeNull();
    expect(screen.queryByRole("heading", { name: "Loaded playbook" })).toBeNull();
  });

  it("leaves device pending geometry full-width and owned by the feature", async () => {
    router.initialPath = "/connections/devices/device-1";
    const { container } = render(<App />);
    await waitFor(() => expect(container.querySelector('[aria-busy="true"]')).not.toBeNull());
    const layouts = container.querySelectorAll('[data-slot="page-layout"]');
    expect(layouts).toHaveLength(1);
    expect(layouts[0]).toHaveClass("max-w-none");
  });
});

describe("App route geometry", () => {
  beforeEach(() => { router.user = undefined; });
  it("wraps the collection route in a full-width page layout", async () => {
    router.initialPath = "/connections/config";
    const { container } = render(<App />);
    expect(await screen.findByTestId("config-list")).toBeInTheDocument();
    const layouts = container.querySelectorAll('[data-slot="page-layout"]');
    expect(layouts).toHaveLength(1);
    expect(layouts[0]!.className).toContain("max-w-none");
    expect(layouts[0]!.className).not.toContain("max-w-[1280px]");
  });

  it("wraps the detail route in a standard-width page layout", async () => {
    router.initialPath = "/connections/config/abc";
    const { container } = render(<App />);
    expect(await screen.findByTestId("config-detail")).toBeInTheDocument();
    const layouts = container.querySelectorAll('[data-slot="page-layout"]');
    expect(layouts).toHaveLength(1);
    expect(layouts[0]!.className).toContain("max-w-[1280px]");
  });

  it("leaves the editor route self-hosted with no route layout wrapper", async () => {
    router.initialPath = "/connections/config/new";
    const { container } = render(<App />);
    expect(await screen.findByTestId("entity-editor-page")).toBeInTheDocument();
    expect(container.querySelector('[data-slot="page-layout"]')).toBeNull();
    expect(container.querySelector('[data-slot="entity-editor-page"]')).not.toBeNull();
  });
});

describe("access-control route capabilities", () => {
  beforeEach(() => { router.initialPath = "/access-control?tab=sso"; });

  it("admits a KEYCLOAK-only reader to the SSO workspace", async () => {
    router.user = { id: "sso-reader", username: "sso-reader", email: "sso@example.test", roles: [],
      capabilities: [{ action: "READ", resourceType: "KEYCLOAK" }] };
    render(<App />);
    expect(await screen.findByTestId("access-workspace")).toBeInTheDocument();
  });

  it("does not admit an unrelated inventory reader", async () => {
    router.user = { id: "inventory-reader", username: "inventory-reader", email: "reader@example.test", roles: [],
      capabilities: [{ action: "READ", resourceType: "INVENTORY" }] };
    render(<App />);
    expect(await screen.findByText(/Your role doesn't grant|Ваша роль не предоставляет/i)).toBeInTheDocument();
    expect(screen.queryByTestId("access-workspace")).not.toBeInTheDocument();
  });
});
