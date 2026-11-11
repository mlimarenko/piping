import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import type { Resource } from "@/shared/api/types";
import type * as InventoryApi from "@/features/inventory/api";
import { useInventoryDetailFields } from "./InventoryResourceDetails";

const api = vi.hoisted(() => ({ getResource: vi.fn() }));
vi.mock("@/features/inventory/api", async original => ({ ...await original<typeof InventoryApi>(), getResource: api.getResource }));

const role = "11111111-1111-4111-8111-111111111111";
const site = "22222222-2222-4222-8222-222222222222";
const tenant = "33333333-3333-4333-8333-333333333333";
const deviceType = "44444444-4444-4444-8444-444444444444";
const platform = "55555555-5555-4555-8555-555555555555";
const rack = "66666666-6666-4666-8666-666666666666";
const primaryIp = "77777777-7777-4777-8777-777777777777";

const resource = {
  id: "99999999-9999-4999-8999-999999999999",
  name: "edge-01",
  type: "device",
  roleId: role,
  siteId: site,
  tenantId: tenant,
  deviceTypeId: deviceType,
  platformId: platform,
  rackId: rack,
  primaryIp4Id: primaryIp,
} as Resource;

const raw = {
  id: resource.id,
  name: resource.name,
  role_id: role,
  site_id: site,
  tenant_id: tenant,
  device_type_id: deviceType,
  platform_id: platform,
  rack_id: rack,
  primary_ip4_id: primaryIp,
  serial: "SN-1",
};

function Harness() {
  const fields = useInventoryDetailFields("Devices", { resource, raw });
  return <dl>{fields.map((field, index) => (
    <div key={index}><dt>{field.label}</dt><dd>{field.value}</dd></div>
  ))}</dl>;
}

beforeEach(() => { api.getResource.mockReset(); });

it("renders relation names when the detail response carries only FK ids", async () => {
  const related: Record<string, Resource> = {
    [`Device Roles:${role}`]: { id: role, name: "Core Role", type: "device_role" } as Resource,
    [`Sites:${site}`]: { id: site, name: "DC-1", type: "site" } as Resource,
    [`Tenants:${tenant}`]: { id: tenant, name: "Acme", type: "tenant" } as Resource,
    [`Device Types:${deviceType}`]: { id: deviceType, name: "Router 9000", type: "device_type" } as Resource,
    [`Platforms:${platform}`]: { id: platform, name: "Linux", type: "platform" } as Resource,
    [`Racks:${rack}`]: { id: rack, name: "R1", type: "rack" } as Resource,
    [`IP Addresses:${primaryIp}`]: { id: primaryIp, name: "192.0.2.10/24", type: "ip_address" } as Resource,
  };
  api.getResource.mockImplementation(async (entity: string, id: string) => related[`${entity}:${id}`]);

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter>
    <Harness />
  </MemoryRouter></AllPermissions></QueryClientProvider>);

  for (const name of ["Core Role", "DC-1", "Acme", "Router 9000", "Linux", "R1", "192.0.2.10/24"]) {
    expect(await screen.findByText(name)).toBeInTheDocument();
  }
  for (const id of [role, site, tenant, deviceType, platform, rack, primaryIp]) {
    expect(screen.queryByText(id)).not.toBeInTheDocument();
  }
  expect(screen.getByText(resource.id)).toBeInTheDocument();
  view.unmount();
  client.clear();
});
