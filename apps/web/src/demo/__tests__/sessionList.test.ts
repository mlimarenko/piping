import { setupServer } from "msw/node";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { handlers } from "../handlers";

const server = setupServer(...handlers);
const base = `${window.location.origin}/api/v1/connections/sessions`;
type Session = { id: string; target_id: string; user_id: string; protocol: string; status: string; ended_at: string | null; presence: { state: string }; observe: { allowed: boolean; reason: string; websocket_url: string | null } };
async function list(query = ""): Promise<{ items: Session[]; total: number; limit: number; offset: number }> {
  const response = await fetch(`${base}${query}`);
  expect(response.ok).toBe(true);
  return response.json();
}
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

describe("synthetic session catalogue", () => {
  it("separates active and historical sessions and denies native observation", async () => {
    const active = await list();
    const history = await list("?view=history");
    expect(active.total).toBeGreaterThan(1);
    expect(history.total).toBeGreaterThan(0);
    expect(active.items.every(item => item.ended_at === null && ["active", "pending"].includes(item.status))).toBe(true);
    expect(history.items.every(item => item.ended_at !== null || ["disconnected", "terminated", "failed"].includes(item.status))).toBe(true);
    for (const session of [...active.items, ...history.items]) {
      expect(session.presence.state).toBe("unknown");
      expect(session.observe).toMatchObject({ allowed: false, websocket_url: null });
      expect(session.observe.reason).toMatch(/demo|демо/i);
    }
  });

  it("applies search, filters, sorting and pagination before the count", async () => {
    const all = await list("?view=active&sort=protocol&order=asc");
    const first = await list("?view=active&sort=protocol&order=asc&limit=1&offset=0");
    const second = await list("?view=active&sort=protocol&order=asc&limit=1&offset=1");
    expect(first).toMatchObject({ total: all.total, limit: 1, offset: 0, items: [all.items[0]] });
    expect(second).toMatchObject({ total: all.total, limit: 1, offset: 1, items: [all.items[1]] });
    const search = await list("?view=history&search=rdp");
    expect(search.total).toBe(1);
    expect(search.items[0]?.protocol).toBe("rdp");
    const filtered = await list(`?view=active&target_id=${encodeURIComponent(all.items[0]!.target_id)}&protocol=${all.items[0]!.protocol}`);
    expect(filtered.total).toBe(1);
    expect(filtered.items[0]?.id).toBe(all.items[0]!.id);
    expect((await list("?view=active&search=not-a-demo-session")).total).toBe(0);
  });
});
