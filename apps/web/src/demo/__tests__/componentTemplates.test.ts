import { setupServer } from "msw/node";
import { afterAll, beforeAll, expect, it } from "vitest";
import { handlers } from "../handlers";

const server = setupServer(...handlers);
const origin = window.location.origin;
const base = (route: string) => `${origin}/api/v1/inventory/${route}`;
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

it("console port templates persist scoped CRUD without metadata or timestamps", async () => {
  const collection = base("console-port-templates");
  const create = await fetch(collection, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name: "QA console", device_type_id: "dt", module_type_id: null, type: "usb-c" }),
  });
  expect(create.ok).toBe(true);
  const row = await create.json() as { id: string };
  expect(await (await fetch(`${collection}?device_type_id=dt&search=QA%20console`)).json()).toMatchObject({
    total: 1,
    items: [{ id: row.id, type: "usb-c", device_type_id: "dt", module_type_id: null }],
  });
  const edit = await fetch(`${collection}/${row.id}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label: "OOB", type: null, device_type_id: null, module_type_id: "mt" }),
  });
  expect(edit.ok).toBe(true);
  const saved = await (await fetch(`${collection}/${row.id}`)).json() as Record<string, unknown>;
  expect(saved).toMatchObject({ name: "QA console", label: "OOB", type: null, device_type_id: null, module_type_id: "mt" });
  for (const field of ["tags", "custom_fields", "comments", "status", "owner_id", "created", "last_updated", "created_at", "updated_at"]) {
    expect(saved).not.toHaveProperty(field);
  }
  expect((await fetch(`${collection}/${row.id}`, { method: "DELETE" })).status).toBe(204);
  expect(await (await fetch(`${collection}?search=QA%20console`)).json()).toMatchObject({ total: 0, items: [] });
});

it("power outlet and inventory item templates keep nullable references explicit", async () => {
  const outlets = base("power-outlet-templates");
  const outlet = await (await fetch(outlets, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name: "QA outlet", module_type_id: "mt", type: "iec-60320-c13", feed_leg: "A" }),
  })).json() as Record<string, unknown>;
  expect(outlet).toMatchObject({ label: "", description: "", color: "", power_port_id: null, feed_leg: "A", device_type_id: null, module_type_id: "mt" });
  expect(outlet).not.toHaveProperty("custom_fields");

  const items = base("inventory-item-templates");
  const item = await (await fetch(items, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name: "QA item", device_type_id: "dt", part_id: "74-000123" }),
  })).json() as Record<string, unknown>;
  expect(item).toMatchObject({ parent_id: null, manufacturer_id: null, role_id: null, part_id: "74-000123", label: "", description: "" });
  const cleared = await (await fetch(`${items}/${String(item.id)}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ part_id: "", parent_id: null }),
  })).json() as Record<string, unknown>;
  expect(cleared).toMatchObject({ part_id: "", parent_id: null });
});

it("exposes the nine native component content types and rear-port mappings", async () => {
  const types = await (await fetch(`${origin}/api/v1/inventory/content-types`)).json() as Array<{ token: string }>;
  const tokens = types.map((type) => type.token);
  for (const token of [
    "dcim.consoleporttemplate", "dcim.consoleserverporttemplate",
    "dcim.coolingintaketemplate", "dcim.coolingoutflowtemplate",
    "dcim.frontporttemplate", "dcim.interfacetemplate",
    "dcim.poweroutlettemplate", "dcim.powerporttemplate", "dcim.rearporttemplate",
  ]) {
    expect(tokens, token).toContain(token);
  }
  expect(tokens).not.toContain("dcim.devicebaytemplate");
  expect(tokens).not.toContain("dcim.inventoryitemtemplate");
  expect(tokens).not.toContain("dcim.modulebaytemplate");

  const front = await (await fetch(`${base("front-port-templates")}?search=Front%201`)).json() as { items: Array<Record<string, unknown>> };
  expect(front.items[0]?.rear_ports).toEqual([
    { position: 1, rear_port: "00000000-0000-4000-8000-000000009707", rear_port_position: 2 },
    { position: 2, rear_port: "00000000-0000-4000-8000-000000009707", rear_port_position: 1 },
  ]);
  const rear = await (await fetch(`${base("rear-port-templates")}?search=Rear%201`)).json() as { items: Array<Record<string, unknown>> };
  expect(rear.items[0]?.front_ports).toEqual([
    { position: 2, front_port: "00000000-0000-4000-8000-000000009706", front_port_position: 1 },
    { position: 1, front_port: "00000000-0000-4000-8000-000000009706", front_port_position: 2 },
  ]);
});
