import { setupServer } from "msw/node";
import createClient from "openapi-fetch";
import { render, screen } from "@testing-library/react";
import { createElement } from "react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { paths } from "@/shared/api/schema.gen";
import type * as ApiClientModule from "@/shared/api/client";
import { createDataflow, listDataflows, resolveDataflowEndpoint, updateDataflow } from "@/features/inventory/dataflows-api";
import { createApplication, deleteApplication, updateDataflowGroup } from "@/features/inventory/dataflowCatalog-api";
import { createResource, deleteResource, listResourcesForTable, updateResource } from "@/features/inventory/api";
import { FlowStatus } from "@/features/inventory/DataflowConnection";
import { createDemoDataflows, DEMO_APPLICATION_CONTENT_TYPE } from "../dataflows";
import { handlers } from "../handlers";

vi.mock("@/shared/api/client", async original => {
  const module = await original<typeof ApiClientModule>();
  return { ...module, apiClient: createClient<paths>({ baseUrl: window.location.origin, fetch: request => globalThis.fetch(request) }) };
});
const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterEach(() => server.resetHandlers());
afterAll(() => server.close());

describe("dataflow demo API", () => {
  it("creates and filters typed application contacts without mixing another application's assignments", async () => {
    const application = await createApplication({ name: "Demo contact scope" });
    const assignment = await createResource("Contact Assignments", {
      name: "", type: "contact_assignment", contactId: "ct-0000-0000-0000-000000000002", roleId: "cr-0000-0000-0000-000000000002",
      object: { objectType: "inventory.application", objectId: application.id, label: null },
    });
    expect(assignment.object?.label).toBe(application.name);
    await updateResource("Contact Assignments", assignment.id, { priority: "secondary" });
    const page = await listResourcesForTable("Contact Assignments", { limit: 1, offset: 0, filters: { content_type: [DEMO_APPLICATION_CONTENT_TYPE], object_id: [application.id] } });
    expect(page.total).toBe(1);
    expect(page.items.map(item => item.id)).toEqual([assignment.id]);
    expect(page.items[0]?.priority).toBe("secondary");
    const denied = await fetch(`${window.location.origin}/api/v1/inventory/applications/${application.id}`, { method: "DELETE" });
    expect(denied.status).toBe(409);
    await deleteResource("Contact Assignments", assignment.id);
    await deleteApplication(application.id);
  });

  it("loads the actual directed demo flows through the product pagination client", async () => {
    const flows = await listDataflows();
    expect(flows).toHaveLength(24);
    expect(new Set(flows.map(flow => flow.id)).size).toBe(flows.length);
    expect(new Set(flows.map(flow => flow.status))).toEqual(new Set(["active", "draft", "disabled"]));
    expect(flows.some(flow => flow.sources.some(source => flow.destinations.some(destination => source.object_id === destination.object_id)))).toBe(true);
  });

  it("resolves a real seeded host and excludes a selected address outside it", async () => {
    const [flow] = await listDataflows();
    const result = await resolveDataflowEndpoint(flow!.sources[0]!);
    expect(result.members).toHaveLength(1);
    expect(result.members[0]?.host.object_id).toBe(flow!.sources[0]!.object_id);
    expect(result.addresses).toEqual([{ cidr: "192.0.2.11/32", vrf_id: null }]);
    const outside = await resolveDataflowEndpoint({ ...flow!.sources[0]!, address_mode: "selected", ipaddresses: ["foreign-ip"] });
    expect(outside.addresses).toEqual([]);
  });

  it("can supply a high-cardinality synthetic dataset without customer records", () => {
    const flows = createDemoDataflows(521);
    expect(flows).toHaveLength(521);
    expect(new Set(flows.map(flow => flow.id)).size).toBe(521);
    expect(new Set(flows.map(flow => flow.destinations[0]?.object_id)).size).toBeGreaterThan(490);
  });

  it("returns the canonical derived flow response on create, list and edit", async () => {
    const endpoint = (object_id: string, object_type: "dcim.device" | "virtualization.virtualmachine" = "dcim.device") => ({ object_type, object_id, address_mode: "default" as const, ipaddresses: [] });
    const created = await createDataflow({
      name: "Demo ICMP flow", status: "draft", transport: "icmp", icmp_types: [0, 8],
      destination_ports: null, source_ports: null, allow_cross_vrf: false,
      sources: [endpoint("00000000-0000-4000-8001-000000000001"), endpoint("00000000-0000-4000-8001-000000000002")],
      destinations: [endpoint("00000000-0000-4000-8001-000000000003", "virtualization.virtualmachine")],
    });
    expect(created.effective_status).toBe("draft");
    expect(created.sources.map(source => source.label)).toEqual(["web-prod-01", "db-primary-01"]);
    expect(created.destinations[0]?.label).toBe("app-node-02");
    expect(created.inherited_tags).toEqual([]);
    expect(created.effective_tags).toEqual([]);

    const listed = (await listDataflows()).find(flow => flow.id === created.id);
    expect(listed).toMatchObject({ effective_status: "draft", status: "draft", icmp_types: [0, 8] });
    expect(listed?.sources[0]?.label).toBe("web-prod-01");
    render(createElement(FlowStatus, { flow: listed! }));
    expect(screen.getByText("Draft")).toBeInTheDocument();

    const updated = await updateDataflow(created.id, { status: "active" });
    expect(updated.effective_status).toBe("active");
    expect((await listDataflows()).find(flow => flow.id === created.id)?.effective_status).toBe("active");
  });

  it("recomputes dependent flow status after a group edit", async () => {
    const group = await updateDataflowGroup("group-prod-0000-0000-0000-000000000001", { status: "disabled" });
    expect(group.effective_status).toBe("disabled");
    const disabledFlows = (await listDataflows()).filter(flow => flow.group_id === group.id);
    expect(disabledFlows.length).toBeGreaterThan(0);
    expect(disabledFlows.every(flow => flow.effective_status === "disabled" && flow.blocked_by_group_id === group.id)).toBe(true);
    const restored = await updateDataflowGroup(group.id, { status: "active" });
    expect(restored.effective_status).toBe("active");
  });
});
