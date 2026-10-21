import { setupServer } from "msw/node";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { ConnConfAssignment } from "@/features/connections/connection-config/api";
import { handlers } from "../handlers";

const server = setupServer(...handlers);
const base = `${window.location.origin}/api/v1/connection-config/assignments`;
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

describe("demo configuration assignments", () => {
  it("reports the missing backend resolver instead of returning a malformed successful preview", async () => {
    const response = await fetch(`${window.location.origin}/api/v1/connection-config/effective/host-web-01?protocol=ssh`);
    expect(response.status).toBe(501);
  });
  it.each([undefined, false])("reads back an assignment with its enabled state: %s", async enabled => {
    const result = await fetch(base, {
      method: "POST", headers: {"Content-Type": "application/json"},
      body: JSON.stringify({record_id: "demo-record", scope_type: "global", scope_id: null, weight: 0, enabled}),
    });
    expect(result.status).toBe(201);
    const saved = await result.json() as ConnConfAssignment;
    expect(saved.enabled).toBe(enabled ?? true);
    const read = await fetch(`${base}/${saved.id}`);
    const persisted = await read.json() as ConnConfAssignment;
    expect(persisted.enabled).toBe(enabled ?? true);
  });
});
