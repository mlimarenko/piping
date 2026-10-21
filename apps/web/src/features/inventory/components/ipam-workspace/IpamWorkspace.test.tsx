import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import IpamWorkspace from "./IpamWorkspace";
import PageChrome from "@/shared/ui-shared/PageChrome";
import type * as InventoryApi from "@/features/inventory/api";
import type { PrefixTreeNode } from "@/features/inventory/api";
import { AllPermissions } from "@/test/permissions";

const apiMocks = vi.hoisted(() => ({
  createResource: vi.fn(),
  listIpAddressesForPrefix: vi.fn(),
  listIpRangesForPrefix: vi.fn(),
  listPrefixTree: vi.fn(),
  listResources: vi.fn(),
  previewNextAvailablePrefix: vi.fn(),
}));

vi.mock("@/features/inventory/api", async (importOriginal) => {
  const actual = await importOriginal<typeof InventoryApi>();
  return {
    ...actual,
    createResource: apiMocks.createResource,
    listIpAddressesForPrefix: apiMocks.listIpAddressesForPrefix,
    listIpRangesForPrefix: apiMocks.listIpRangesForPrefix,
    listPrefixTree: apiMocks.listPrefixTree,
    listResources: apiMocks.listResources,
    previewNextAvailablePrefix: apiMocks.previewNextAvailablePrefix,
  };
});

function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>{ui}</AllPermissions>
    </QueryClientProvider>,
  );
}

const nodes: PrefixTreeNode[] = [
  {
    id: "root",
    node_id: "prefix:root",
    prefix: "10.0.0.0/24",
    parent_id: null,
    parent_prefix: null,
    root_id: null,
    root_prefix: null,
    depth: 0,
    child_count: 2,
    status: "container",
    family: 4,
    site_id: "site-a",
    site_name: "Core",
    vrf_id: null,
    vrf_name: null,
    tenant_id: null,
    tenant_name: null,
    mark_utilized: false,
    utilized_addresses: 1,
    usable_addresses: 254,
    utilization_percent: 0.4,
    breadcrumbs: ["10.0.0.0/24"],
  },
  {
    id: "child-a",
    node_id: "prefix:child-a",
    prefix: "10.0.0.0/25",
    parent_id: "root",
    parent_prefix: "10.0.0.0/24",
    root_id: null,
    root_prefix: null,
    depth: 1,
    child_count: 0,
    status: "active",
    family: 4,
    site_id: "site-a",
    site_name: "Core",
    vrf_id: null,
    vrf_name: null,
    tenant_id: null,
    tenant_name: null,
    mark_utilized: true,
    utilized_addresses: 120,
    usable_addresses: 126,
    utilization_percent: 95.2,
    breadcrumbs: ["10.0.0.0/24", "10.0.0.0/25"],
  },
  {
    id: "ipv6",
    node_id: "prefix:ipv6",
    prefix: "2001:db8::/64",
    parent_id: null,
    parent_prefix: null,
    root_id: null,
    root_prefix: null,
    depth: 0,
    child_count: 0,
    status: "active",
    family: 6,
    site_id: null,
    site_name: null,
    vrf_id: null,
    vrf_name: null,
    tenant_id: null,
    tenant_name: null,
    mark_utilized: false,
    utilized_addresses: 0,
    usable_addresses: null,
    utilization_percent: null,
    breadcrumbs: ["2001:db8::/64"],
  },
];

describe("IpamWorkspace", () => {
  beforeEach(() => {
    apiMocks.createResource.mockReset();
    apiMocks.listIpAddressesForPrefix.mockReset();
    apiMocks.listIpRangesForPrefix.mockReset();
    apiMocks.listPrefixTree.mockReset();
    apiMocks.listResources.mockReset();
    apiMocks.previewNextAvailablePrefix.mockReset();
    apiMocks.createResource.mockResolvedValue({
      id: "created-prefix",
      name: "10.0.0.128/26",
      type: "prefix",
      prefix: "10.0.0.128/26",
      status: "active",
      siteId: "site-a",
    });
    apiMocks.listResources.mockResolvedValue({
      items: [],
      total: 0,
      limit: 50,
      offset: 0,
    });
    apiMocks.listIpAddressesForPrefix.mockResolvedValue({
      items: [
        {
          id: "ip-1",
          name: "10.0.0.10/24",
          type: "ip_address",
          prefix: "10.0.0.10/24",
          status: "active",
          ipamRole: "loopback",
          dnsName: "core-a.example.test",
          interfaceName: "xe-0/0/0",
        },
      ],
      total: 1,
      limit: 50,
      offset: 0,
    });
    apiMocks.listIpRangesForPrefix.mockResolvedValue({
      items: [
        {
          id: "range-1",
          name: "10.0.0.20 - 10.0.0.30",
          type: "ip_range",
          startAddress: "10.0.0.20",
          endAddress: "10.0.0.30",
          size: "11",
          status: "active",
          tenant: "Network",
        },
      ],
      total: 1,
      limit: 50,
      offset: 0,
    });
    apiMocks.listPrefixTree.mockResolvedValue({
      nodes,
      total: nodes.length,
      limit: 300,
      applied_filters: {
        search: null,
        root_id: null,
        root_prefix: null,
        family: null,
        site_id: [],
        vrf_id: [],
        tenant_id: [],
        status: [],
        limit: 300,
      },
    });
    apiMocks.previewNextAvailablePrefix.mockResolvedValue({
      parent_id: "root",
      parent_prefix: "10.0.0.0/24",
      requested_prefix_length: 26,
      status: "available",
      candidate_prefix: "10.0.0.128/26",
      reason: null,
      conflicts: [],
      searched_candidates: 3,
    });
  });

  it("renders the tree, scoped child table, inspector, and null utilization honestly", async () => {
    renderWithClient(<><PageChrome title="IPAM Workspace" /><IpamWorkspace /></>);

    expect(await screen.findByRole("heading", { name: "IPAM Workspace" })).toBeInTheDocument();
    expect((await screen.findAllByText("10.0.0.0/24")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("10.0.0.0/25").length).toBeGreaterThan(0);
    expect(screen.getByRole("tree", { name: "Prefix Tree" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.queryByRole("heading", { level: 2, name: "IPAM Workspace" })).not.toBeInTheDocument();

    const table = screen.getByRole("table");
    expect(within(table).getByText("10.0.0.0/25")).toBeInTheDocument();
    expect(screen.getAllByText("mark utilized").length).toBeGreaterThan(0);
    const utilizedMeters = await screen.findAllByRole("progressbar", {
      name: "Address utilization for 10.0.0.0/25",
    });
    expect(utilizedMeters.some((meter) => meter.getAttribute("aria-valuenow") === "95.2")).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: /2001:db8::\/64/ }));
    expect(screen.getAllByText("not calculated").length).toBeGreaterThan(0);
  });

  it("only shows expand controls on prefixes that actually have children", async () => {
    renderWithClient(<IpamWorkspace />);

    expect(await screen.findByRole("button", { name: "Collapse 10.0.0.0/24" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Expand 10.0.0.0/25" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Collapse 10.0.0.0/25" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Expand 2001:db8::/64" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Collapse 2001:db8::/64" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Collapse 10.0.0.0/24" }));
    expect(screen.getByRole("button", { name: "Expand 10.0.0.0/24" })).toBeInTheDocument();
  });

  it("sends filter state to the tree endpoint", async () => {
    renderWithClient(<IpamWorkspace />);
    await screen.findByRole("tree", { name: "Prefix Tree" });

    fireEvent.change(screen.getByPlaceholderText("Search prefixes, labels, status..."), {
      target: { value: "core" },
    });

    await waitFor(() => {
      expect(apiMocks.listPrefixTree).toHaveBeenLastCalledWith(
        expect.objectContaining({ search: "core", limit: 300 }),
      );
    });
  });

  it("previews next available prefix for the selected prefix", async () => {
    renderWithClient(<IpamWorkspace />);
    await screen.findByRole("tree", { name: "Prefix Tree" });
    await screen.findAllByText("10.0.0.0/24");

    fireEvent.change(screen.getByLabelText("Child prefix length"), {
      target: { value: "26" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Preview/i }));

    await waitFor(() => {
      expect(apiMocks.previewNextAvailablePrefix).toHaveBeenCalledWith("root", 26);
    });
    expect(await screen.findByText("10.0.0.128/26")).toBeInTheDocument();
  });

  it("creates a prefix directly from the workspace planning preview", async () => {
    renderWithClient(<IpamWorkspace />);
    await screen.findByRole("tree", { name: "Prefix Tree" });
    await screen.findAllByText("10.0.0.0/24");

    expect(screen.getByRole("button", { name: "Create prefix" })).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Child prefix length"), {
      target: { value: "26" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Preview/i }));
    expect(await screen.findByText("10.0.0.128/26")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Create from preview" }));
    const dialog = await screen.findByTestId("entity-editor-page");
    expect(within(dialog).getByDisplayValue("10.0.0.128/26")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

    await waitFor(() => {
      expect(apiMocks.createResource).toHaveBeenCalledWith(
        "Prefixes",
        expect.objectContaining({
          prefix: "10.0.0.128/26",
          status: "active",
          siteId: "site-a",
        }),
      );
    });
  });

  it("loads IP addresses contained in the selected prefix", async () => {
    renderWithClient(<IpamWorkspace />);
    await screen.findByRole("tree", { name: "Prefix Tree" });

    fireEvent.keyDown(screen.getByRole("tab", { name: "IP addresses" }), { key: "Enter" });

    await waitFor(() => {
      expect(apiMocks.listIpAddressesForPrefix).toHaveBeenCalledWith("10.0.0.0/24", {
        limit: 50,
        offset: 0,
        vrfId: null,
      });
    });
    expect(await screen.findByText("10.0.0.10/24")).toBeInTheDocument();
    expect(screen.getByText("xe-0/0/0")).toBeInTheDocument();
  });

  it("loads IP ranges fully contained in the selected prefix", async () => {
    renderWithClient(<IpamWorkspace />);
    await screen.findByRole("tree", { name: "Prefix Tree" });

    fireEvent.keyDown(screen.getByRole("tab", { name: "IP ranges" }), { key: "Enter" });

    await waitFor(() => {
      expect(apiMocks.listIpRangesForPrefix).toHaveBeenCalledWith("10.0.0.0/24", {
        limit: 50,
        offset: 0,
        vrfId: null,
      });
    });
    expect(await screen.findByText("10.0.0.20 - 10.0.0.30")).toBeInTheDocument();
    expect(screen.getByText("Network")).toBeInTheDocument();
  });

  it("associates every pane tab with a mounted tabpanel and switches the workspace by keyboard", async () => {
    renderWithClient(<IpamWorkspace />);
    await screen.findByRole("tree", { name: "Prefix Tree" });

    const panelOf = (name: string) => {
      const trigger = screen.getByRole("tab", { name });
      const controls = trigger.getAttribute("aria-controls");
      expect(controls).toBeTruthy();
      const panel = document.getElementById(controls as string);
      expect(panel).not.toBeNull();
      expect(panel).toHaveAttribute("role", "tabpanel");
    };

    for (const pane of ["Browse", "List", "Inspect"]) panelOf(pane);
    panelOf("Prefixes");
    fireEvent.keyDown(screen.getByRole("tab", { name: "IP addresses" }), { key: "Enter" });
    expect(screen.getByRole("tab", { name: "IP addresses" })).toHaveAttribute("aria-selected", "true");
    panelOf("IP addresses");
    fireEvent.keyDown(screen.getByRole("tab", { name: "IP ranges" }), { key: "Enter" });
    expect(screen.getByRole("tab", { name: "IP ranges" })).toHaveAttribute("aria-selected", "true");
    panelOf("IP ranges");
  });
});
