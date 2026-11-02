import { beforeEach, describe, expect, it, vi } from "vitest";

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

import { buildAuditEventParams, listAuditLogs } from "./api";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("buildAuditEventParams", () => {
  it("comma-joins the three IN-filter dimensions and drops empty arrays", () => {
    expect(
      buildAuditEventParams({
        eventTypes: ["mcp.tool.invoked", "playbook.created"],
        targetTypes: ["device"],
        results: [],
        limit: 25,
        offset: 0,
      }),
    ).toMatchObject({
      event_type: "mcp.tool.invoked,playbook.created",
      target_type: "device",
      result: undefined,
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
        mcpTool: "ssh.exec",
        mcpToolGroups: ["ssh", "rest"],
        mcpProtocols: ["ssh"],
        mcpRisks: [],
        from: "2026-01-01T00:00:00Z",
        to: "2026-02-01T00:00:00Z",
      }),
    ).toMatchObject({
      q: "login",
      actor_kind: "mcp_agent,api_token",
      actor_id: "actor-1,actor-2",
      target_id: "target-1",
      mcp_tool: "ssh.exec",
      mcp_tool_group: "ssh,rest",
      mcp_protocol: "ssh",
      mcp_risk: undefined,
      created_from: "2026-01-01T00:00:00Z",
      created_to: "2026-02-01T00:00:00Z",
    });
  });
});

describe("listAuditLogs", () => {
  it("sends comma-joined multi-value filters + sort to /audit/events and returns the envelope", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [{ id: "evt-1", created_at: "2026-06-15T00:00:00Z", event_type: "playbook.created" }],
        total: 7,
        limit: 25,
        offset: 0,
      },
    });

    const result = await listAuditLogs({
      eventTypes: ["playbook.created", "playbook.updated"],
      results: ["succeeded"],
      sort: "event_type",
      order: "desc",
      limit: 25,
      offset: 0,
    });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/audit/events", {
      params: {
        query: expect.objectContaining({
          event_type: "playbook.created,playbook.updated",
          result: "succeeded",
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
  });
});
