import { setupServer } from "msw/node";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { handlers } from "../handlers";

const server = setupServer(...handlers);
const base = `${window.location.origin}/api/v1/connections/targets`;
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

interface Target {
  name: string;
  role_id: string;
  role_name: string;
  target_kind: string;
}

async function list(query = ""): Promise<{ items: Target[]; total: number; limit: number; offset: number }> {
  const response = await fetch(`${base}${query}`);
  expect(response.ok).toBe(true);
  return response.json();
}

describe("demo connection targets", () => {
  it("filters the actual role subset and combines search, sorting and pagination", async () => {
    const all = await list();
    expect(all.total).toBe(11);
    const role = all.items.find((item) => item.role_name === "Linux Server");
    expect(role).toBeDefined();
    const roleId = role!.role_id;
    const roleQuery = `role_id=${encodeURIComponent(roleId)}`;
    const filtered = await list(`?${roleQuery}`);
    expect(filtered.total).toBe(5);
    expect(filtered.items.every((item) => item.role_id === roleId)).toBe(true);

    const result = await list(`?${roleQuery}&search=fra&sort=name&order=desc&limit=1&offset=1`);
    expect(result).toMatchObject({ total: 2, limit: 1, offset: 1, items: [{ name: "fra-db-01" }] });
  });

  it("combines multi-value filters and rejects unknown sort keys", async () => {
    const result = await list("?target_kind=virtual_machine&status=planned,active");
    expect(result.total).toBe(2);
    expect(result.items.every((item) => item.target_kind === "virtual_machine")).toBe(true);
    expect((await fetch(`${base}?sort=not-a-column`)).status).toBe(400);
    expect((await fetch(`${base}?limit=invalid`)).status).toBe(400);
  });
});
