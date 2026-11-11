import { describe, expect, it } from "vitest";
import { matchesAuditSearch } from "../handlers";
import { rows } from "../store";

describe("PIP-205 demo audit search", () => {
  it("matches synthetic event-time labels and identities, but not metadata", () => {
    const events = rows("audit/events");
    const firstEvent = events[0];
    if (!firstEvent) throw new Error("Expected a synthetic audit event");
    expect(events.filter(event => matchesAuditSearch(event, "synthetic server"))).toHaveLength(2);
    expect(events.filter(event => matchesAuditSearch(event, "synthetic operator"))).toHaveLength(2);
    expect(events.filter(event => matchesAuditSearch(event, "secrets.resolve"))).toHaveLength(1);
    expect(events.filter(event => matchesAuditSearch(event, String(firstEvent.id)))).toHaveLength(1);
    expect(events.filter(event => matchesAuditSearch(event, "ssh"))).toHaveLength(0);
    expect(matchesAuditSearch({
      ...firstEvent,
      metadata: { nested: { credential: "synthetic-credential-canary" } },
    }, "synthetic-credential-canary")).toBe(false);
  });
});
