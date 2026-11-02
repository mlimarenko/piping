import { setupServer } from "msw/node";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { handlers } from "../handlers";

const server = setupServer(...handlers);
const base = `${window.location.origin}/api/v1/inventory`;
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

async function write(path: string, method: string, body?: unknown) {
  const response = await fetch(`${base}/${path}`, {
    method,
    headers: { "Content-Type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  expect(response.ok).toBe(true);
  return response.status === 204 ? undefined : response.json();
}

describe("demo rack groups and cable bundles", () => {
  it.each(["rack-groups", "cable-bundles"])("persists %s CRUD, searches and paginates", async (segment) => {
    const created = await write(segment, "POST", { name: "QA row", description: "original", comments: "aisle", custom_fields: { row: 1 }, ...(segment === "rack-groups" ? { slug: "qa-row" } : {}) });
    const response = await fetch(`${base}/${segment}?search=QA%20row&limit=1&offset=0`);
    expect(await response.json()).toMatchObject({ total: 1, limit: 1, offset: 0, items: [{ id: created.id, comments: "aisle", custom_fields: { row: 1 } }] });
    await write(`${segment}/${created.id}`, "PUT", { description: "", comments: "", custom_fields: {} });
    expect(await (await fetch(`${base}/${segment}/${created.id}`)).json()).toMatchObject({ name: "QA row", description: "", comments: "", custom_fields: {} });
    await write(`${segment}/${created.id}`, "DELETE");
    expect(await (await fetch(`${base}/${segment}?search=QA%20row`)).json()).toMatchObject({ total: 0, items: [] });
  });

  it.each([
    ["racks", "rack-groups", "group_id", "group_name"],
    ["cables", "cable-bundles", "bundle_id", "bundle_name"],
  ])("hydrates %s relations after changes, renames and clearing", async (segment, parent, idField, nameField) => {
    const collection = await write(parent, "POST", { name: "Shared route", slug: "shared-route" });
    const item = await write(segment, "POST", { name: "Related row", [idField]: collection.id });
    const read = async () => (await fetch(`${base}/${segment}/${item.id}`)).json();
    expect(await read()).toMatchObject({ [idField]: collection.id, [nameField]: "Shared route" });
    await write(`${parent}/${collection.id}`, "PUT", { name: "Renamed route" });
    expect(await read()).toMatchObject({ [nameField]: "Renamed route" });
    await write(`${segment}/${item.id}`, "PUT", { [idField]: null });
    expect(await read()).toMatchObject({ [idField]: null, [nameField]: null });
    await write(`${segment}/${item.id}`, "DELETE");
    await write(`${parent}/${collection.id}`, "DELETE");
  });
});
