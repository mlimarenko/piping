import { beforeEach, describe, expect, it, vi } from "vitest";
import type { components } from "@/shared/api/schema.gen";

const apiMocks = vi.hoisted(() => ({
  DELETE: vi.fn(),
  GET: vi.fn(),
  POST: vi.fn(),
  PUT: vi.fn(),
}));

vi.mock("@/shared/api/client", () => ({
  apiClient: apiMocks,
  unwrap: (result: { data?: unknown; error?: unknown }) => {
    if (result.error) {
      if (result.error instanceof Error) throw result.error;
      throw new Error(typeof result.error === "string" ? result.error : "API mock error");
    }
    return result.data;
  },
}));

import { buildAuditEventParams, getAuditLog, listAuditLogs } from "./api";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("buildAuditEventParams", () => {
  it("comma-joins the three IN-filter dimensions and drops empty arrays", () => {
    expect(
      buildAuditEventParams({
        eventTypes: ["mcp.tool.invoked", "playbook.created"],
        targetTypes: ["device"],
        outcomes: [],
        limit: 25,
        offset: 0,
      }),
    ).toMatchObject({
      event_type: "mcp.tool.invoked,playbook.created",
      target_type: "device",
      outcome: undefined,
      limit: 25,
      offset: 0,
    });
  });

  it("forwards sort with default asc order", () => {
    const params = buildAuditEventParams({ sort: "event_type" });
    expect(params).toMatchObject({ sort: "event_type", order: "asc" });
  });

  it("forwards explicit desc order", () => {
    const params = buildAuditEventParams({ sort: "created_at", order: "desc" });
    expect(params).toMatchObject({ sort: "created_at", order: "desc" });
  });

  it("forwards context filters without a legacy result parameter", () => {
    const params = buildAuditEventParams({ outcomes: ["denied", "unknown"], stateDomain: "session", stateValue: "active", requestId: "request", sessionId: "session", runId: "run", runGroupId: "group" });
    expect(params).toMatchObject({ outcome: "denied,unknown", state_domain: "session", state_value: "active", request_id: "request", session_id: "session", run_id: "run", run_group_id: "group" });
    expect(params).not.toHaveProperty("result");
    for (const key of ["mcp_tool", "mcp_tool_group", "mcp_protocol", "mcp_risk"]) expect(params).not.toHaveProperty(key);
  });

  it("never emits an order without a sort key", () => {
    const params = buildAuditEventParams({ order: "desc" });
    expect(params).not.toHaveProperty("sort");
    expect(params).not.toHaveProperty("order");
  });

  it("comma-joins every multi-value dimension and maps scalars", () => {
    expect(
      buildAuditEventParams({
        query: "login",
        actorKinds: ["mcp_agent", "api_token"],
        actorIds: ["actor-1", "actor-2"],
        targetIds: ["target-1"],
        from: "2026-01-01T00:00:00Z",
        to: "2026-02-01T00:00:00Z",
      }),
    ).toMatchObject({
      q: "login",
      actor_kind: "mcp_agent,api_token",
      actor_id: "actor-1,actor-2",
      target_id: "target-1",
      created_from: "2026-01-01T00:00:00Z",
      created_to: "2026-02-01T00:00:00Z",
    });
  });
});

it("loads an individual event on demand", async () => {
  apiMocks.GET.mockResolvedValue({
    data: {
      id: "event",
      created_at: "2026-06-15T00:00:00Z",
      event_type: "historical.event",
      actor_kind: "system",
      context: { outcome: "unknown", correlation: {}, display: {} },
      metadata: {},
    } satisfies components["schemas"]["AuditEventResponse"],
  });
  const event = await getAuditLog("event");
  expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/audit/events/{id}", { params: { path: { id: "event" } } });
  expect(event.context.outcome).toBe("unknown");
});

describe("listAuditLogs", () => {
  it("sends comma-joined multi-value filters + sort to /audit/events and returns the envelope", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [{
          id: "evt-1",
          created_at: "2026-06-15T00:00:00Z",
          event_type: "playbook.created",
          actor_kind: "system",
          context: { outcome: "succeeded" },
          metadata: {},
        }],
        total: 7,
        limit: 25,
        offset: 0,
      } satisfies components["schemas"]["AuditEventListResponse"],
    });

    const result = await listAuditLogs({
      eventTypes: ["playbook.created", "playbook.updated"],
      outcomes: ["succeeded"],
      sort: "event_type",
      order: "desc",
      limit: 25,
      offset: 0,
    });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/audit/events", {
      params: {
        query: expect.objectContaining({
          event_type: "playbook.created,playbook.updated",
          outcome: "succeeded",
          sort: "event_type",
          order: "desc",
          limit: 25,
          offset: 0,
        }),
      },
    });
    expect(result.total).toBe(7);
    expect(result.items).toHaveLength(1);
    expect(result.items[0]).toMatchObject({ id: "evt-1", action: "playbook.created" });
    expect(result.items[0]?.context).toEqual({ outcome: "succeeded", correlation: {}, display: {} });
  });
});
