import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { testUuid } from "@/test/ids";
import type * as InventoryApi from "@/features/inventory/api";
import TreeNodeInspector from "./TreeNodeInspector";

const inventory = vi.hoisted(() => ({ listResources: vi.fn(), listDevicesForSite: vi.fn(), countDevicesForSite: vi.fn(), listDeviceInterfaces: vi.fn(), listIpAddressesForDevice: vi.fn() }));
vi.mock("@/features/inventory/api", async (importOriginal) => ({
  ...await importOriginal<typeof InventoryApi>(),
  listResources: inventory.listResources,
  listDevicesForSite: inventory.listDevicesForSite,
  countDevicesForSite: inventory.countDevicesForSite,
  listDeviceInterfaces: inventory.listDeviceInterfaces,
  listIpAddressesForDevice: inventory.listIpAddressesForDevice,
}));
vi.mock("@/shared/auth/permissionsContext", () => ({
  usePermissions: () => ({ can: () => true, canAny: () => true, isAdmin: true, roles: [] }),
}));

describe("group inspector relation identity", () => {
  beforeEach(() => {
    window.localStorage.clear();
    inventory.listResources.mockReset();
    inventory.listDevicesForSite.mockReset();
    inventory.countDevicesForSite.mockReset();
    inventory.countDevicesForSite.mockResolvedValue(1);
    inventory.listDeviceInterfaces.mockReset();
    inventory.listIpAddressesForDevice.mockReset();
    inventory.listDeviceInterfaces.mockResolvedValue([]);
    inventory.listIpAddressesForDevice.mockResolvedValue([]);
    inventory.listDevicesForSite.mockResolvedValue([{ id: "device-1", name: "server-1" }]);
  });

  it.each([true, false])("loads a site relation without opening details (host FK: %s)", async (withFk) => {
    const siteId = testUuid("site-1");
    inventory.listResources.mockResolvedValue({ items: [{ id: siteId, name: "Datacenter" }], total: 1, limit: 1000, offset: 0 });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><TreeNodeInspector
      node={{ type: "site", id: "site-node", name: "Datacenter", hosts: [{ id: testUuid("host-1"), name: "host", targetKind: "device", ...(withFk ? { siteId } : {}) }] }}
      onClose={() => {}}
    /></MemoryRouter></QueryClientProvider>);

    expect(inventory.listDevicesForSite).not.toHaveBeenCalled();
    const devices = screen.getByRole("button", { name: /^Devices/ });
    fireEvent.click(devices);
    await waitFor(() => expect(inventory.listDevicesForSite).toHaveBeenCalledWith(siteId));
    expect(await screen.findByText("server-1")).toBeInTheDocument();
    await waitFor(() => expect(devices).toHaveTextContent("1"));
    if (withFk) expect(inventory.listResources).not.toHaveBeenCalled();
    else expect(inventory.listResources).toHaveBeenCalledWith("Sites");
    expect(screen.getByRole("button", { name: "Details" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("button", { name: /^Contacts/ })).toHaveAttribute("aria-expanded", "false");
  });

  it("does not show an empty collection while parent identity is resolving", async () => {
    let finishLookup!: (value: { items: { id: string; name: string }[]; total: number; limit: number; offset: number }) => void;
    inventory.listResources.mockImplementation(() => new Promise((resolve) => { finishLookup = resolve; }));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><TreeNodeInspector
      node={{ type: "site", id: "site-node", name: "Datacenter", hosts: [] }} onClose={() => {}}
    /></MemoryRouter></QueryClientProvider>);
    fireEvent.click(screen.getByRole("button", { name: /^Devices/ }));
    expect(inventory.listDevicesForSite).not.toHaveBeenCalled();
    expect(screen.queryByText("No devices")).not.toBeInTheDocument();
    finishLookup({ items: [{ id: "site-1", name: "Datacenter" }], total: 1, limit: 1000, offset: 0 });
    expect(await screen.findByText("server-1")).toBeInTheDocument();
  });

  it("hides a confirmed empty relation without requiring expansion", async () => {
    inventory.listDevicesForSite.mockResolvedValue([]);
    inventory.countDevicesForSite.mockResolvedValue(0);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><TreeNodeInspector
      node={{ type: "site", id: "site-node", name: "Datacenter", hosts: [{ id: testUuid("host-1"), name: "host", targetKind: "device", siteId: testUuid("site-1") }] }} onClose={() => {}}
    /></MemoryRouter></QueryClientProvider>);
    const devices = screen.getByRole("button", { name: /^Devices/ });
    expect(devices).not.toHaveTextContent("0");
    await waitFor(() => expect(screen.queryByRole("button", { name: /^Devices/ })).not.toBeInTheDocument());
    expect(inventory.countDevicesForSite).toHaveBeenCalledWith(testUuid("site-1"));
    expect(inventory.listDevicesForSite).not.toHaveBeenCalled();
  });

  it("leaves the collapsed interface count unknown until its response", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const host = { id: testUuid("host-1"), name: "host", targetKind: "device" as const, deviceId: testUuid("device-1") };
    render(<QueryClientProvider client={client}><MemoryRouter><TreeNodeInspector
      node={{ type: "host", id: host.id, name: host.name, host, hosts: [host] }} onClose={() => {}}
    /></MemoryRouter></QueryClientProvider>);
    const interfaces = screen.getByRole("button", { name: /^Interfaces/ });
    expect(interfaces).not.toHaveTextContent("0");
    expect(inventory.listDeviceInterfaces).not.toHaveBeenCalled();
    expect(inventory.listIpAddressesForDevice).not.toHaveBeenCalled();
    fireEvent.click(interfaces);
    await waitFor(() => expect(screen.queryByRole("button", { name: /^Interfaces/ })).not.toBeInTheDocument());
    expect(inventory.listDeviceInterfaces).toHaveBeenCalledTimes(1);
    expect(inventory.listIpAddressesForDevice).toHaveBeenCalledTimes(1);
  });
});
