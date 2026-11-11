import { setupServer } from "msw/node";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { handlers } from "../handlers";

const server = setupServer(...handlers);
const base = `${window.location.origin}/api/v1/inventory`;
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

async function write(path: string, method: string, body?: unknown) {
  const response = await fetch(`${base}/${path}`, { method, headers: { "Content-Type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
  expect(response.ok).toBe(true);
  return response.status === 204 ? undefined : response.json();
}

describe("Cooling demo collections", () => {
  it.each(["cooling-sources", "cooling-feeds", "cooling-intakes", "cooling-outflows", "cooling-intake-templates", "cooling-outflow-templates"])("persists %s edits, clears, scoped reads and deletion with native timestamps", async segment => {
    const parent = segment === "cooling-sources" ? { site_id: "site" }
      : segment === "cooling-feeds" ? { cooling_source_id: "source" }
        : segment.includes("templates") ? { module_type_id: "module-type" } : { device_id: "device", module_id: "module" };
    const created = await write(segment, "POST", { name: "Cooling QA", ...parent, description: "original", type: "qdc", diameter: 12.75 });
    expect(created.created).toEqual(expect.any(String));
    expect(created.last_updated).toEqual(expect.any(String));
    const [scope, id] = Object.entries(parent)[0]!;
    expect(await (await fetch(`${base}/${segment}?${scope}=${id}&search=Cooling%20QA&limit=1&offset=0`)).json()).toMatchObject({ total: 1, items: [{ id: created.id }] });
    await write(`${segment}/${created.id}`, "PUT", { description: "", diameter: null });
    const edited = await (await fetch(`${base}/${segment}/${created.id}`)).json();
    expect(edited).toMatchObject({ name: "Cooling QA", description: "", diameter: null, last_updated: expect.any(String), ...parent });
    expect(edited).not.toHaveProperty("updated_at");
    if (segment.includes("templates")) {
      expect(edited).not.toHaveProperty("tags");
      expect(edited).not.toHaveProperty("custom_fields");
      expect(edited).not.toHaveProperty("owner_id");
      expect(edited).not.toHaveProperty("status");
    }
    await write(`${segment}/${created.id}`, "DELETE");
    expect(await (await fetch(`${base}/${segment}?search=Cooling%20QA`)).json()).toMatchObject({ total: 0, items: [] });
  });
});
