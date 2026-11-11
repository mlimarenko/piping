import { setupServer } from "msw/node";
import { afterAll, beforeAll, expect, it } from "vitest";
import { handlers } from "../handlers";

const server = setupServer(...handlers);
const base = `${window.location.origin}/api/v1/inventory/module-bay-templates`;
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

it("Module bay templates demo persists scoped CRUD and membership clears without metadata or timestamps", async () => {
  const create = await fetch(base, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ name: "QA slot", module_type_id: "mt", module_bay_types: ["category"], enabled: false }) });
  expect(create.ok).toBe(true);
  const row = await create.json() as { id: string };
  expect(await (await fetch(`${base}?module_type_id=mt&search=QA%20slot`)).json()).toMatchObject({ total: 1, items: [{ id: row.id, enabled: false, module_bay_types: ["category"] }] });
  const edit = await fetch(`${base}/${row.id}`, { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ module_bay_types: [], module_type_id: null, device_type_id: "dt" }) });
  expect(edit.ok).toBe(true);
  const saved = await (await fetch(`${base}/${row.id}`)).json() as Record<string, unknown>;
  expect(saved).toMatchObject({ name: "QA slot", enabled: false, module_bay_types: [], module_type_id: null, device_type_id: "dt" });
  for (const field of ["owner_id", "tags", "custom_fields", "created", "last_updated", "created_at", "updated_at"]) expect(saved).not.toHaveProperty(field);
  expect((await fetch(`${base}/${row.id}`, { method: "DELETE" })).status).toBe(204);
  expect(await (await fetch(`${base}?search=QA%20slot`)).json()).toMatchObject({ total: 0, items: [] });
});
