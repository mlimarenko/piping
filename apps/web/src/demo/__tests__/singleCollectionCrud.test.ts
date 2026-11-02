import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";

import { handlers } from "../handlers";
import { remove } from "../store";

const server = setupServer(...handlers);
const created: Array<{ key: string; id: string }> = [];

beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterEach(() => {
  server.resetHandlers();
  for (const { key, id } of created.splice(0)) remove(key, id);
});
afterAll(() => server.close());

const collections = [
  { key: "custom-fields", fields: { field_type: "text", label: "QA Field", object_types: [] } },
  { key: "custom-field-choice-sets", fields: { extra_choices: [{ value: "qa", label: "QA Option" }] } },
  { key: "config-contexts", fields: { data: { environment: "qa" }, weight: 1000 } },
];

async function createRecord(key: string, body: Record<string, unknown>) {
  const response = await fetch(`${window.location.origin}/api/v1/${key}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  expect(response.status).toBe(201);
  const record = await response.json() as { id: string; name: string };
  created.push({ key, id: record.id });
  return record;
}

describe("demo single-segment collection CRUD", () => {
  it.each(collections)("persists $key create, item read, update and delete", async ({ key, fields }) => {
    const name = `qa-crud-${key}`;
    const record = await createRecord(key, { name, ...fields });
    const url = `${window.location.origin}/api/v1/${key}`;

    const listed = await (await fetch(`${url}?search=${name}`)).json();
    expect(listed.items).toContainEqual(expect.objectContaining({ id: record.id, name, ...fields }));
    expect(await (await fetch(`${url}/${record.id}`)).json()).toMatchObject({ id: record.id, name, ...fields });

    const patched = await fetch(`${url}/${record.id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: `${name}-edited` }),
    });
    expect(patched.ok).toBe(true);
    expect(await patched.json()).toMatchObject({ id: record.id, name: `${name}-edited`, ...fields });
    expect(await (await fetch(`${url}/${record.id}`)).json()).toMatchObject({ id: record.id, name: `${name}-edited` });

    const deleted = await fetch(`${url}/${record.id}`, { method: "DELETE" });
    expect(deleted.status).toBe(204);
    const afterDelete = await (await fetch(`${url}?search=${name}`)).json();
    expect(afterDelete.items).toEqual([]);
    expect(afterDelete.total).toBe(0);
    expect((await fetch(`${url}/${record.id}`)).status).toBe(404);
  });

  it("keeps filtered totals and nonzero-offset pages after choice-set creates", async () => {
    const key = "custom-field-choice-sets";
    const prefix = "qa-choice-pagination";
    for (let index = 0; index < 3; index += 1) {
      await createRecord(key, { name: `${prefix}-${index}`, extra_choices: [{ value: "qa", label: "QA" }] });
    }
    const page = await (await fetch(`${window.location.origin}/api/v1/${key}?search=${prefix}&limit=1&offset=1`)).json();
    expect(page).toMatchObject({ total: 3, limit: 1, offset: 1 });
    expect(page.items).toHaveLength(1);
    expect(page.items[0].name).toBe(`${prefix}-1`);
  });
});
