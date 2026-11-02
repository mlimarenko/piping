import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";

import { handlers } from "../handlers";
import type { ConnConfRecord } from "@/features/connections/connection-config/api";
import { draftFromRecord } from "@/features/connections/connection-config/recordDraft";

/**
 * The demo lane serves `/api/v1/*` from an in-memory store, and the UI consumes
 * it as if it were the backend. That only holds if the rows it hands back match
 * the generated contract.
 *
 * `ConnConfRecordResponse.description` is a plain `string` — the column is
 * `NOT NULL DEFAULT ''` and the insert binds `unwrap_or_default()`, so the real
 * API cannot return null there. The demo lane could: seeds omitted the field and
 * creates echoed the request body, where the UI sends `null` for an empty
 * description. `draftFromRecord` carried that through, and the update mutation's
 * `draft.description.trim()` threw before `updateRecord` was called — Save
 * Changes on an existing record issued no PUT and surfaced nothing.
 *
 * These assertions pin the shape at the seam where it broke.
 */

const server = setupServer(...handlers);

// The handlers are declared with app-relative paths, which MSW resolves against
// `location.origin`. Anchoring here keeps the generic CRUD handlers in play —
// an origin they do not match silently falls through to the `*/api/v1/*`
// catch-all, whose write branch answers 204 and touches no store.
const RECORDS_URL = `${window.location.origin}/api/v1/connection-config/records`;

beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterEach(() => server.resetHandlers());
afterAll(() => server.close());

async function listRecords(): Promise<ConnConfRecord[]> {
  const response = await fetch(`${RECORDS_URL}?limit=1000&offset=0`);
  expect(response.ok).toBe(true);
  const body = (await response.json()) as { items: ConnConfRecord[] };
  return body.items;
}

/** Exactly what `ConnectionConfig`'s update mutation does with the draft. */
function buildUpdatePayload(record: ConnConfRecord) {
  const draft = draftFromRecord(record);
  return {
    name: draft.name.trim(),
    description: draft.description.trim(),
    protocol: draft.protocol,
    base_record_id: draft.baseRecordId || null,
    enabled: draft.enabled,
    params: draft.params,
  };
}

describe("demo lane: connection-config record contract", () => {
  it("serves seeded records with a string description", async () => {
    const records = await listRecords();
    expect(records.length).toBeGreaterThan(0);
    for (const record of records) {
      expect(typeof record.description, `record ${record.name}`).toBe("string");
    }
  });

  it("stores a create that sends description: null as an empty string", async () => {
    // This is the body `createRecord` sends for a record left without a
    // description — `draft.description.trim() || null`.
    const created = await fetch(RECORDS_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        name: "no-description",
        description: null,
        protocol: "ssh",
        base_record_id: null,
        enabled: true,
        params: { host: "{{primary_ip4}}" },
      }),
    });
    expect(created.status).toBe(201);
    expect((await created.json()).description).toBe("");

    const stored = (await listRecords()).find((r) => r.name === "no-description");
    expect(stored?.description).toBe("");
  });

  it("yields an update payload for every record it serves", async () => {
    for (const record of await listRecords()) {
      // Without the contract-shaped description this threw a TypeError inside
      // the mutation function, which is why the PUT never left the browser.
      expect(() => buildUpdatePayload(record), `record ${record.name}`).not.toThrow();
      expect(typeof buildUpdatePayload(record).description).toBe("string");
    }
  });
});
