import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, useLocation } from "react-router";
import type { ReactElement } from "react";
import "@/shared/i18n";
import { afterEach, beforeEach, expect, vi } from "vitest";
import TreeNodeInspector from "./TreeNodeInspector";
import { inventoryResourceHref } from "@/features/inventory/resourceLinks";
import type { Resource } from "@/shared/api/types";
import { testUuid } from "@/test/ids";

const inspectorMocks = vi.hoisted(() => {
  const emptyRelation = (): { items: Resource[]; loading: boolean; error: Error | null } => ({
    items: [],
    loading: false,
    error: null,
  });
  return {
    useResolvedGroupId: vi.fn(
      (): { id: string | undefined; loading: boolean } => ({ id: undefined, loading: false }),
    ),
    useEntityDetail: vi.fn(
      (): {
        data: { resource: Record<string, unknown>; raw: Record<string, unknown> } | undefined;
        isLoading: boolean;
      } => ({
        data: undefined,
        isLoading: false,
      }),
    ),
    useEntityContacts: vi.fn(
      (): {
        contacts: {
          id: string;
          name: string;
          role?: string;
          title?: string;
          phone?: string;
          email?: string;
          address?: string;
          link?: string;
          description?: string;
          contactId?: string;
          roleId?: string;
        }[];
        loading: boolean;
      } => ({ contacts: [], loading: false }),
    ),
    useContactCatalog: vi.fn(() => ({ contacts: [], roles: [], loading: false })),
    useHostNetworkInterfaces: vi.fn(
      (): { interfaces: Resource[]; addressesByInterfaceId: Map<string, Resource[]>; loading: boolean } => ({
      interfaces: [],
      addressesByInterfaceId: new Map(),
      loading: false,
    })),
    // Overridable permission gate so individual tests can flip UPDATE to exercise
    // the header edit-in-workspace vs inventory-deep-link fallback.
    can: vi.fn((_action?: string, _subject?: string): boolean => true),
    useRelatedEntityNames: vi.fn((): Map<string, string> => new Map()),
    useSiteDevices: vi.fn(emptyRelation),
    useSiteRacks: vi.fn(emptyRelation),
    useSiteLocations: vi.fn(emptyRelation),
    useRegionSites: vi.fn(emptyRelation),
    useTenantDevices: vi.fn(emptyRelation),
    useTenantVms: vi.fn(emptyRelation),
    useClusterDevices: vi.fn(emptyRelation),
    useClusterVms: vi.fn(emptyRelation),
    hostMetricsPreview: vi.fn((_props: { enabled?: boolean }) => null),
    revealEntityCustomFields: vi.fn(() => Promise.resolve({})),
  };
});

vi.mock("./inspectorData", () => ({
  GROUP_ENTITY: {
    site: "Sites",
    region: "Regions",
    tenant: "Tenants",
    role: "Device Roles",
    platform: "Platforms",
    deviceType: "Device Types",
    cluster: "Clusters",
  },
  entitySupportsContacts: (type: string) => ["host", "site", "region", "tenant", "cluster"].includes(type),
  useResolvedGroupId: inspectorMocks.useResolvedGroupId,
  useEntityDetail: inspectorMocks.useEntityDetail,
  useEntityContacts: inspectorMocks.useEntityContacts,
  useContactCatalog: inspectorMocks.useContactCatalog,
  useHostNetworkInterfaces: inspectorMocks.useHostNetworkInterfaces,
  useRelatedEntityNames: inspectorMocks.useRelatedEntityNames,
  useSiteDevices: inspectorMocks.useSiteDevices,
  useSiteRacks: inspectorMocks.useSiteRacks,
  useSiteLocations: inspectorMocks.useSiteLocations,
  useRegionSites: inspectorMocks.useRegionSites,
  useTenantDevices: inspectorMocks.useTenantDevices,
  useTenantVms: inspectorMocks.useTenantVms,
  useClusterDevices: inspectorMocks.useClusterDevices,
  useClusterVms: inspectorMocks.useClusterVms,
}));

vi.mock("@/features/observability/HostMetricsPreviewSection", () => ({
  default: (props: { enabled?: boolean }) => {
    inspectorMocks.hostMetricsPreview(props);
    return null;
  },
}));

vi.mock("@/features/inventory/custom-fields-api", () => ({
  revealEntityCustomFields: inspectorMocks.revealEntityCustomFields,
}));

// ConnectButtons (rendered by TreeNodeInspector for host nodes) gates on
// CONNECT; grant all capabilities so these tests see the connect affordances.
vi.mock("@/shared/auth/permissionsContext", () => ({
  usePermissions: () => ({
    roles: [],
    isAdmin: true,
    can: inspectorMocks.can,
    canAny: () => true,
    hasRole: () => false,
  }),
}));

// The edit affordances navigate to the canonical full-page inventory editor;
// the probe records the resulting location so tests can assert the destination.
function LocationProbe() {
  const location = useLocation();
  return <div data-testid="location">{`${location.pathname}${location.search}`}</div>;
}

function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        {ui}
        <LocationProbe />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("TreeNodeInspector", () => {
  beforeEach(() => {
    window.localStorage.clear();
    vi.useFakeTimers();
    inspectorMocks.useResolvedGroupId.mockClear();
    inspectorMocks.useResolvedGroupId.mockReturnValue({ id: undefined, loading: false });
    inspectorMocks.useEntityDetail.mockClear();
    inspectorMocks.useEntityDetail.mockReturnValue({ data: undefined, isLoading: false });
    inspectorMocks.useEntityContacts.mockClear();
    inspectorMocks.useEntityContacts.mockReturnValue({ contacts: [], loading: false });
    inspectorMocks.useContactCatalog.mockClear();
    inspectorMocks.useContactCatalog.mockReturnValue({ contacts: [], roles: [], loading: false });
    inspectorMocks.useHostNetworkInterfaces.mockClear();
    inspectorMocks.useHostNetworkInterfaces.mockReturnValue({
      interfaces: [],
      addressesByInterfaceId: new Map(),
      loading: false,
    });
    inspectorMocks.useRelatedEntityNames.mockClear();
    inspectorMocks.useRelatedEntityNames.mockReturnValue(new Map());
    for (const hook of [
      inspectorMocks.useSiteDevices,
      inspectorMocks.useSiteRacks,
      inspectorMocks.useSiteLocations,
      inspectorMocks.useRegionSites,
      inspectorMocks.useTenantDevices,
      inspectorMocks.useTenantVms,
      inspectorMocks.useClusterDevices,
      inspectorMocks.useClusterVms,
    ]) {
      hook.mockClear();
      hook.mockReturnValue({ items: [], loading: false, error: null });
    }
    inspectorMocks.hostMetricsPreview.mockClear();
    inspectorMocks.revealEntityCustomFields.mockClear();
    inspectorMocks.revealEntityCustomFields.mockResolvedValue({});
    inspectorMocks.can.mockClear();
    inspectorMocks.can.mockReturnValue(true);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts with collapsed sections and remembers their expansion for another host and remount", () => {
    const inspector = (id: string) => <TreeNodeInspector node={{ type: "host", id, name: id,
      host: { id: testUuid(id), name: id, targetKind: "device", site: "Test site" } }} onClose={() => {}} />;
    const first = renderWithClient(inspector("first"));
    expect(screen.getByRole("button", { name: "Info" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Test site")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Info" }));
    expect(screen.getByText("Test site")).toBeInTheDocument();
    first.unmount();
    const second = renderWithClient(inspector("second"));
    expect(screen.getByRole("button", { name: "Info" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Test site")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Info" }));
    second.unmount();
    renderWithClient(inspector("third"));
    expect(screen.getByRole("button", { name: "Info" })).toHaveAttribute("aria-expanded", "false");
  });

  it("renders tenant group nodes without crashing", () => {
    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "tenant",
          id: "tenant-acme",
          name: "Acme Corp",
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    expect(
      screen.getByRole("heading", { name: "Acme Corp" }),
    ).toBeInTheDocument();
  });

  it("does not render host-derived aggregate rows on group nodes", () => {
    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "tenant",
          id: "tenant-acme",
          name: "Acme Corp",
          hosts: [
            { id: testUuid("h1"), name: "h1", targetKind: "device", site: "DC-1", region: "EU" },
            { id: testUuid("h2"), name: "h2", targetKind: "device", site: "DC-2", region: "EU" },
          ],
        }}
        onClose={() => {}}
      />,
    );

    // Group overview no longer aggregates host sites/regions into rows.
    expect(screen.queryByText("DC-1")).toBeNull();
    expect(screen.queryByText("DC-2")).toBeNull();
    expect(screen.queryByText("EU")).toBeNull();
  });

  it("defers detail and metrics probes until after the inspector shell renders", () => {
    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-1",
          name: "pxm-piping01",
          host: {
            id: testUuid("host-1"),
            name: "pxm-piping01",
            targetKind: "device",
            deviceId: testUuid("device-1"),
          },
          hosts: [],
        }}
        onClose={() => {}}
        onConnect={() => {}}
      />,
    );

    // With UPDATE held, the host header name is the edit-in-workspace button; the
    // inventory deep-link survives as a small secondary "Open in inventory" link.
    expect(screen.getByRole("button", { name: /pxm-piping01/ })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open in inventory" })).toBeInTheDocument();
    expect(inspectorMocks.useEntityDetail).toHaveBeenLastCalledWith("Devices", "device-1", false);
    expect(inspectorMocks.hostMetricsPreview).toHaveBeenLastCalledWith(
      expect.objectContaining({ enabled: false }),
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    expect(inspectorMocks.useEntityDetail).toHaveBeenLastCalledWith("Devices", "device-1", true);
    expect(inspectorMocks.hostMetricsPreview).toHaveBeenLastCalledWith(
      expect.objectContaining({ enabled: true }),
    );
  });

  it("looks up host detail by targetKind: device → Devices/deviceId", () => {
    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-d",
          name: "dev-host",
          host: {
            id: testUuid("host-d"),
            name: "dev-host",
            targetKind: "device",
            deviceId: testUuid("device-9"),
            vmId: testUuid("vm-should-not-be-used"),
          },
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    expect(inspectorMocks.useEntityDetail).toHaveBeenLastCalledWith("Devices", "device-9", false);
  });

  it("looks up host detail by targetKind: virtual_machine → Virtual Machines/vmId", () => {
    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-v",
          name: "vm-host",
          host: {
            id: testUuid("host-v"),
            name: "vm-host",
            targetKind: "virtual_machine",
            deviceId: testUuid("device-should-not-be-used"),
            vmId: testUuid("vm-7"),
          },
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    expect(inspectorMocks.useEntityDetail).toHaveBeenLastCalledWith("Virtual Machines", "vm-7", false);
  });

  it("renders contacts as a read-only relation row with copyable selectable fields", () => {
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const assignmentId = testUuid("assignment-1");
    const contactId = testUuid("contact-9");
    inspectorMocks.useResolvedGroupId.mockReturnValue({
      id: testUuid("tenant-entity-1"),
      loading: false,
    });
    inspectorMocks.useEntityContacts.mockReturnValue({
      contacts: [
        {
          id: assignmentId,
          name: "Jane Ops",
          role: "Owner",
          title: "Regional manager",
          phone: "+7 (812) 335-95-25",
          email: "jane.ops@example.com",
          address: "Nevsky prospect 1",
          link: "https://example.com/jane",
          description: "Escalation contact",
          contactId,
          roleId: testUuid("role-3"),
        },
      ],
      loading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{ type: "tenant", id: "tenant-acme", name: "Acme Corp", hosts: [] }}
        onClose={() => {}}
      />,
    );

    // Contacts section is collapsed by default — expand it (toggle name unchanged
    // by the header "+" action, which is a sibling of the toggle button).
    fireEvent.click(screen.getByRole("button", { name: /^Contacts/ }));

    // With edit permission the Contact name opens its edit form in-workspace (a
    // button, not a navigation link); details remain selectable/copyable beside it.
    const nameButton = screen.getByRole("button", { name: /Jane Ops/ });
    expect(nameButton.tagName).toBe("BUTTON");
    expect(nameButton.querySelectorAll("a")).toHaveLength(0);
    expect(screen.getByText(/Owner/)).toBeInTheDocument();
    expect(screen.getByText("Regional manager")).toBeInTheDocument();
    expect(screen.getByText("+7 (812) 335-95-25")).toBeInTheDocument();
    expect(screen.getByText("jane.ops@example.com")).toBeInTheDocument();
    expect(screen.getByText("Nevsky prospect 1")).toBeInTheDocument();
    expect(screen.getByText("https://example.com/jane")).toBeInTheDocument();
    expect(screen.getByText("Escalation contact")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Copy" })).toHaveLength(6);
    const phoneField = screen.getByText("+7 (812) 335-95-25").closest("[data-contact-detail-row]");
    expect(phoneField).not.toBeNull();
    fireEvent.click(within(phoneField as HTMLElement).getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("+7 (812) 335-95-25");
    expect(screen.queryByPlaceholderText("Contact name")).toBeNull();

    // Explicit requirement: there is NO detach/remove/delete affordance anywhere.
    expect(screen.queryByRole("button", { name: /remove|detach|delete/i })).toBeNull();
    if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
    else Reflect.deleteProperty(navigator, "clipboard");
  });

  it("Contacts section is read-only — no add/attach affordance", () => {
    inspectorMocks.useResolvedGroupId.mockReturnValue({
      id: testUuid("tenant-entity-1"),
      loading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{ type: "tenant", id: "tenant-acme", name: "Acme Corp", hosts: [] }}
        onClose={() => {}}
      />,
    );

    // Expand Contacts: there is NO "+"/attach affordance and no dialog — the
    // inspector is view-only; editing happens via the entity's edit form.
    fireEvent.click(screen.getByRole("button", { name: /^Contacts/ }));
    expect(screen.queryByRole("button", { name: "Add contact" })).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("does not leak VM-only cluster into a device inspector", () => {
    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-d",
          name: "dev-host",
          host: {
            id: testUuid("host-d"),
            name: "dev-host",
            targetKind: "device",
            deviceId: testUuid("device-9"),
            cluster: "Cluster should stay hidden",
            clusterId: testUuid("cluster-1"),
          },
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    expect(screen.queryByText("Cluster should stay hidden")).toBeNull();
  });

  it("does not leak device-only type into a VM inspector", () => {
    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-v",
          name: "vm-host",
          host: {
            id: testUuid("host-v"),
            name: "vm-host",
            targetKind: "virtual_machine",
            vmId: testUuid("vm-7"),
            deviceType: "Server model should stay hidden",
            deviceTypeId: testUuid("device-type-1"),
          },
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    expect(screen.queryByText("Server model should stay hidden")).toBeNull();
  });

  it("renders long group detail text with a show all / hide toggle and copies it", () => {
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const longText = "x".repeat(400);
    inspectorMocks.useResolvedGroupId.mockReturnValue({ id: testUuid("site-1"), loading: false });
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: {
        resource: { id: testUuid("site-1"), name: "DC", description: longText },
        raw: { id: testUuid("site-1"), name: "DC" },
      },
      isLoading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{ type: "site", id: "site-dc", name: "DC", hosts: [] }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    fireEvent.click(screen.getByRole("button", { name: "Details" }));
    const toggle = screen.getByRole("button", { name: "Show all" });
    expect(toggle).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Copy Description" }));
    expect(writeText).toHaveBeenCalledWith(longText);
    fireEvent.click(toggle);
    expect(screen.getByRole("button", { name: "Hide" })).toBeInTheDocument();

    if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
    else Reflect.deleteProperty(navigator, "clipboard");
  });

  it("links host rack detail to the rack inventory record and copies the linked value", () => {
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const rackId = testUuid("rack-1");
    const primaryIpId = testUuid("primary-ip-1");
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: {
        resource: {
          id: testUuid("device-1"),
          name: "srv-1",
          primaryIp: "10.0.0.10/32",
          primaryIp4Id: primaryIpId,
          rackName: "Rack A",
          rackId,
        },
        raw: { id: testUuid("device-1"), name: "srv-1" },
      },
      isLoading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-1",
          name: "srv-1",
          host: {
            id: testUuid("host-1"),
            name: "srv-1",
            targetKind: "device",
            deviceId: testUuid("device-1"),
          },
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    // FK detail rows are edit buttons that navigate to the canonical editor.
    fireEvent.click(screen.getByRole("button", { name: "Details" }));
    const rackButton = screen.getByRole("button", { name: "Rack A" });
    expect(screen.getByRole("button", { name: "10.0.0.10/32" })).toBeInTheDocument();
    fireEvent.click(rackButton);
    expect(screen.getByTestId("location")).toHaveTextContent(
      `/inventory/resources/Racks/${rackId}/edit`,
    );
    fireEvent.click(screen.getByRole("button", { name: "Copy Rack" }));
    expect(writeText).toHaveBeenCalledWith("Rack A");

    if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
    else Reflect.deleteProperty(navigator, "clipboard");
  });

  it("nests IP addresses under their interface (read-only master-detail) instead of a flat IP section", () => {
    const ifaceId = testUuid("iface-1");
    const ipId = testUuid("ip-1");
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: {
        resource: { id: testUuid("device-1"), name: "srv-1" },
        raw: { id: testUuid("device-1"), name: "srv-1" },
      },
      isLoading: false,
    });
    inspectorMocks.useHostNetworkInterfaces.mockReturnValue({
      interfaces: [
        {
          id: ifaceId,
          name: "eth0",
          type: "interface",
          interfaceType: "1000base-t",
          enabled: "true",
          mtu: "1500",
        },
      ],
      addressesByInterfaceId: new Map([
        [ifaceId, [{ id: ipId, name: "10.0.0.20/32", type: "ip-address", prefix: "10.0.0.20/32" }]],
      ]),
      loading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-1",
          name: "srv-1",
          host: {
            id: testUuid("host-1"),
            name: "srv-1",
            targetKind: "device",
            deviceId: testUuid("device-1"),
          },
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    // There is NO separate flat "IP Addresses" section anymore — IPs nest under
    // their interface, mirroring the edit card.
    expect(screen.queryByRole("button", { name: /^IP Addresses/ })).toBeNull();

    // Expand the single Interfaces section; with edit permission the interface
    // name opens its edit form (a button, not a navigation link).
    fireEvent.click(screen.getByRole("button", { name: /^Interfaces/ }));
    expect(screen.getByRole("button", { name: /eth0/ })).toBeInTheDocument();

    // The IP is hidden until the interface row is expanded via its chevron.
    expect(screen.queryByRole("button", { name: /10\.0\.0\.20\/32/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show IP addresses" }));
    expect(screen.getByRole("button", { name: /10\.0\.0\.20\/32/ })).toBeInTheDocument();
  });

  it("surfaces orphan IPs (interface not in the list) under a fallback group", () => {
    const ifaceId = testUuid("iface-1");
    const orphanIfaceId = testUuid("iface-gone");
    const ipId = testUuid("ip-orphan");
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: {
        resource: { id: testUuid("device-1"), name: "srv-1" },
        raw: { id: testUuid("device-1"), name: "srv-1" },
      },
      isLoading: false,
    });
    inspectorMocks.useHostNetworkInterfaces.mockReturnValue({
      interfaces: [{ id: ifaceId, name: "eth0", type: "interface" }],
      addressesByInterfaceId: new Map([
        // Address grouped under an interface id NOT present in `interfaces`.
        [orphanIfaceId, [{ id: ipId, name: "10.9.9.9/32", type: "ip-address", prefix: "10.9.9.9/32" }]],
      ]),
      loading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-1",
          name: "srv-1",
          host: { id: testUuid("host-1"), name: "srv-1", targetKind: "device", deviceId: testUuid("device-1") },
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    // The orphan group renders (not silently dropped); expand it to see the IP.
    const orphanSection = screen.getByRole("button", { name: /Other IP addresses/ });
    expect(orphanSection).toBeInTheDocument();
    fireEvent.click(orphanSection);
    expect(screen.getByRole("button", { name: /10\.9\.9\.9\/32/ })).toBeInTheDocument();
  });

  it("fetches host interfaces/addresses eagerly on card open so counts populate without expanding", () => {
    const deviceId = testUuid("device-1");
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: { resource: { id: deviceId, name: "srv-1" }, raw: { id: deviceId, name: "srv-1" } },
      isLoading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-1",
          name: "srv-1",
          host: { id: testUuid("host-1"), name: "srv-1", targetKind: "device", deviceId },
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    // The host interface/address fetch is EAGER — enabled on card open, not gated
    // on expansion — so the Interfaces count populates in the background (the IP
    // list, now nested per interface, is derived from the same fetch).
    expect(inspectorMocks.useHostNetworkInterfaces).toHaveBeenLastCalledWith("device", deviceId, true);

    // Expanding the Interfaces section keeps the shared fetch enabled.
    fireEvent.click(screen.getByRole("button", { name: /^Interfaces/ }));
    expect(inspectorMocks.useHostNetworkInterfaces).toHaveBeenLastCalledWith("device", deviceId, true);
  });

  it("shows the full field set: previously-hidden fields surface, empties collapse", () => {
    inspectorMocks.useResolvedGroupId.mockReturnValue({ id: testUuid("site-1"), loading: false });
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: {
        resource: { id: testUuid("site-1"), name: "DC" },
        raw: { id: testUuid("site-1"), name: "DC", slug: "dc", facility: "" },
      },
      isLoading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{ type: "site", id: "site-dc", name: "DC", hosts: [] }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    // The Details section now renders for any resolved entity (full field set).
    expect(screen.getByRole("button", { name: "Details" })).toBeInTheDocument();
    // `slug` — a read-only field the CRUD form never carried — is now surfaced.
    fireEvent.click(screen.getByRole("button", { name: "Details" }));
    expect(screen.getByText("Slug")).toBeInTheDocument();
    expect(screen.getByText("dc")).toBeInTheDocument();
    // Empty fields (facility, geo, addresses, …) collapse behind a toggle.
    expect(screen.getByRole("button", { name: /empty fields/i })).toBeInTheDocument();
  });

  it("reveals secret custom fields in the inspector for supported entities", async () => {
    vi.useRealTimers();
    inspectorMocks.revealEntityCustomFields.mockResolvedValue({
      ad_login: "m103-operator",
      cloud_password: "cleartext-secret",
    });
    inspectorMocks.useResolvedGroupId.mockReturnValue({ id: testUuid("site-1"), loading: false });
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: {
        resource: {
          id: testUuid("site-1"),
          name: "DC",
          customFields: { fallback_only: "unused when raw fields are present" },
        },
        raw: {
          id: testUuid("site-1"),
          name: "DC",
          custom_fields: {
            ad_login: "m103-operator",
            cloud_password: "****",
          },
        },
      },
      isLoading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{ type: "site", id: "site-dc", name: "DC", hosts: [] }}
        onClose={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /Custom fields/i }));
    expect(await screen.findByText("cleartext-secret")).toBeInTheDocument();
    expect(screen.queryByText("****")).toBeNull();
    expect(inspectorMocks.revealEntityCustomFields).toHaveBeenCalledWith("sites", testUuid("site-1"));
  });

  it("renders custom fields as their own collapsible copyable section", () => {
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    inspectorMocks.useResolvedGroupId.mockReturnValue({ id: testUuid("site-1"), loading: false });
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: {
        resource: {
          id: testUuid("site-1"),
          name: "DC",
          customFields: { fallback_only: "unused when raw fields are present" },
        },
        raw: {
          id: testUuid("site-1"),
          name: "DC",
          custom_fields: {
            ad_login: "m103-operator",
            cloud_password: "****",
            empty_value: "",
          },
        },
      },
      isLoading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{ type: "site", id: "site-dc", name: "DC", hosts: [] }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    const sectionToggle = screen.getByRole("button", { name: /Custom fields/i });
    expect(sectionToggle).toBeInTheDocument();
    fireEvent.click(sectionToggle);
    expect(screen.getByText("Ad Login")).toBeInTheDocument();
    expect(screen.getByText("m103-operator")).toBeInTheDocument();
    expect(screen.getByText("Cloud Password")).toBeInTheDocument();
    expect(screen.getByText("****")).toBeInTheDocument();
    expect(screen.queryByText("Empty Value")).toBeNull();
    expect(screen.queryByText(/Ad Login: m103-operator/)).toBeNull();

    const loginField = screen.getByText("m103-operator").closest("[data-custom-field-row]");
    expect(loginField).not.toBeNull();
    fireEvent.click(within(loginField as HTMLElement).getByRole("button", { name: "Copy Ad Login" }));
    expect(writeText).toHaveBeenCalledWith("m103-operator");

    fireEvent.click(sectionToggle);
    expect(screen.queryByText("Ad Login")).toBeNull();

    if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
    else Reflect.deleteProperty(navigator, "clipboard");
  });

  it("resolves a group node's FK-only related field to a named inventory link", () => {
    // The detail endpoint returns only `regionId` (no resolved `region` name);
    // the cached related-entity list supplies the display name so the curated
    // related row renders as a named link instead of collapsing as empty.
    const regionId = testUuid("region-eu");
    inspectorMocks.useResolvedGroupId.mockReturnValue({ id: testUuid("site-1"), loading: false });
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: {
        resource: { id: testUuid("site-1"), name: "DC", regionId },
        raw: { id: testUuid("site-1"), name: "DC", region_id: regionId },
      },
      isLoading: false,
    });
    inspectorMocks.useRelatedEntityNames.mockReturnValue(new Map([[regionId, "EU Region"]]));

    renderWithClient(
      <TreeNodeInspector
        node={{ type: "site", id: "site-dc", name: "DC", hosts: [] }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    // The region FK row is a populated edit button that navigates to the canonical
    // editor (not "—" under "show empty", and not an inventory link).
    fireEvent.click(screen.getByRole("button", { name: "Details" }));
    const regionButton = screen.getByRole("button", { name: "EU Region" });
    fireEvent.click(regionButton);
    expect(screen.getByTestId("location")).toHaveTextContent(
      `/inventory/resources/Regions/${regionId}/edit`,
    );
  });

  it("navigates to the canonical editor from the header when UPDATE is held (host)", () => {
    const deviceId = testUuid("device-1");
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: { resource: { id: deviceId, name: "srv-1" }, raw: { id: deviceId, name: "srv-1" } },
      isLoading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-1",
          name: "srv-1",
          host: { id: testUuid("host-1"), name: "srv-1", targetKind: "device", deviceId },
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    // The header name is the edit button; it navigates to the canonical editor.
    const headerButton = screen.getByRole("button", { name: /srv-1/ });
    fireEvent.click(headerButton);
    expect(screen.getByTestId("location")).toHaveTextContent(
      `/inventory/resources/Devices/${deviceId}/edit`,
    );
  });

  it("navigates to the canonical editor for an editable group node (site)", () => {
    const siteId = testUuid("site-1");
    inspectorMocks.useResolvedGroupId.mockReturnValue({ id: siteId, loading: false });
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: { resource: { id: siteId, name: "DC" }, raw: { id: siteId, name: "DC" } },
      isLoading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{ type: "site", id: "site-dc", name: "DC", hosts: [] }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    fireEvent.click(screen.getByRole("button", { name: /^DC/ }));
    expect(screen.getByTestId("location")).toHaveTextContent(
      `/inventory/resources/Sites/${siteId}/edit`,
    );
  });

  it("falls back to the inventory deep-link header (no edit) without UPDATE", () => {
    inspectorMocks.can.mockImplementation((action?: string) => action !== "UPDATE");
    const deviceId = testUuid("device-1");
    inspectorMocks.useEntityDetail.mockReturnValue({
      data: { resource: { id: deviceId, name: "srv-1" }, raw: { id: deviceId, name: "srv-1" } },
      isLoading: false,
    });

    renderWithClient(
      <TreeNodeInspector
        node={{
          type: "host",
          id: "host-1",
          name: "srv-1",
          host: { id: testUuid("host-1"), name: "srv-1", targetKind: "device", deviceId },
          hosts: [],
        }}
        onClose={() => {}}
      />,
    );

    act(() => {
      vi.advanceTimersByTime(100);
    });

    // No UPDATE → header is the inventory deep-link, and nothing navigates.
    const headerLink = screen.getByRole("link", { name: "srv-1" });
    expect(headerLink).toHaveAttribute("href", inventoryResourceHref("Devices", deviceId));
    expect(screen.queryByRole("button", { name: /srv-1/ })).toBeNull();
  });

  describe("relations registry — each node type renders its own section set", () => {
    // Section toggle buttons render `<title> <countBadge>`, so match the leading
    // word. A section's absence means that relation is not in the node's registry.
    const hasSection = (re: RegExp) => screen.queryByRole("button", { name: re }) != null;

    const renderNode = (type: string, name: string, extra: Record<string, unknown> = {}) => {
      inspectorMocks.useResolvedGroupId.mockReturnValue({ id: testUuid(`${type}-1`), loading: false });
      renderWithClient(
        <TreeNodeInspector
          node={{ type, id: `${type}-id`, name, hosts: [], ...extra } as never}
          onClose={() => {}}
        />,
      );
      act(() => {
        vi.advanceTimersByTime(100);
      });
    };

    it("Site → Devices, Racks, Locations, Contacts (no VMs/Sites)", () => {
      renderNode("site", "DC");
      expect(hasSection(/^Devices/)).toBe(true);
      expect(hasSection(/^Racks/)).toBe(true);
      expect(hasSection(/^Locations/)).toBe(true);
      expect(hasSection(/^Contacts/)).toBe(true);
      expect(hasSection(/^Virtual Machines/)).toBe(false);
      expect(hasSection(/^Sites/)).toBe(false);
    });

    it("Region → Sites, Contacts (no Devices/Racks)", () => {
      renderNode("region", "EU");
      expect(hasSection(/^Sites/)).toBe(true);
      expect(hasSection(/^Contacts/)).toBe(true);
      expect(hasSection(/^Devices/)).toBe(false);
      expect(hasSection(/^Racks/)).toBe(false);
    });

    it("Tenant → Devices, Virtual Machines, Contacts (no Racks/Sites)", () => {
      renderNode("tenant", "Acme");
      expect(hasSection(/^Devices/)).toBe(true);
      expect(hasSection(/^Virtual Machines/)).toBe(true);
      expect(hasSection(/^Contacts/)).toBe(true);
      expect(hasSection(/^Racks/)).toBe(false);
      expect(hasSection(/^Sites/)).toBe(false);
    });

    it("Cluster → Devices, Virtual Machines, Contacts (no Racks/Locations)", () => {
      renderNode("cluster", "Prod");
      expect(hasSection(/^Devices/)).toBe(true);
      expect(hasSection(/^Virtual Machines/)).toBe(true);
      expect(hasSection(/^Contacts/)).toBe(true);
      expect(hasSection(/^Racks/)).toBe(false);
      expect(hasSection(/^Locations/)).toBe(false);
    });

    it("Host (device) → Interfaces (IPs nested, no flat IP section), Contacts (no Devices/VMs)", () => {
      renderWithClient(
        <TreeNodeInspector
          node={{
            type: "host",
            id: "host-d",
            name: "srv-1",
            host: { id: testUuid("host-d"), name: "srv-1", targetKind: "device", deviceId: testUuid("device-1") },
            hosts: [],
          }}
          onClose={() => {}}
        />,
      );
      act(() => {
        vi.advanceTimersByTime(100);
      });
      expect(hasSection(/^Interfaces/)).toBe(true);
      // IPs nest under interfaces — there is no separate flat IP Addresses section.
      expect(hasSection(/^IP Addresses/)).toBe(false);
      expect(hasSection(/^Contacts/)).toBe(true);
      expect(hasSection(/^Devices/)).toBe(false);
      expect(hasSection(/^Virtual Machines/)).toBe(false);
    });

    it("Host (VM) → VM Interfaces (IPs nested, no flat IP section), Contacts", () => {
      renderWithClient(
        <TreeNodeInspector
          node={{
            type: "host",
            id: "host-v",
            name: "vm-1",
            host: { id: testUuid("host-v"), name: "vm-1", targetKind: "virtual_machine", vmId: testUuid("vm-1") },
            hosts: [],
          }}
          onClose={() => {}}
        />,
      );
      act(() => {
        vi.advanceTimersByTime(100);
      });
      expect(hasSection(/^VM Interfaces/)).toBe(true);
      expect(hasSection(/^IP Addresses/)).toBe(false);
      expect(hasSection(/^Contacts/)).toBe(true);
    });

    it("role/platform/deviceType nodes have Details only — no relation or contacts sections", () => {
      renderNode("role", "Switch");
      expect(hasSection(/^Contacts/)).toBe(false);
      expect(hasSection(/^Devices/)).toBe(false);
      expect(hasSection(/^Interfaces/)).toBe(false);
    });
  });
});
