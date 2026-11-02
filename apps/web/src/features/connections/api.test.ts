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

import { buildConnectionTableParams, listConnectionTargets, openSession } from "./api";
import type { OpenSessionRequest } from "@/shared/api/types";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("buildConnectionTableParams", () => {
  it("forwards search, sort, default order, and comma-joined FK filters", () => {
    expect(
      buildConnectionTableParams({
        limit: 1000,
        offset: 0,
        search: "  cashier  ",
        sort: "platform",
        filters: {
          region_id: ["region-1"],
          tenant_id: ["tenant-1", "tenant-2"],
          platform_id: [],
        },
      }),
    ).toEqual({
      limit: 1000,
      offset: 0,
      search: "cashier",
      sort: "platform",
      order: "asc",
      region_id: "region-1",
      tenant_id: "tenant-1,tenant-2",
    });
  });
});

describe("listConnectionTargets", () => {
  it("sends search plus all standardized FK filters to the targets endpoint", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [],
        total: 0,
        limit: 1000,
        offset: 0,
      },
    });

    await listConnectionTargets({
      search: "ares",
      sort: "deviceType",
      order: "desc",
      filters: {
        status: ["active"],
        target_kind: ["device"],
        region_id: ["region-1"],
        site_id: ["site-1"],
        tenant_id: ["tenant-1"],
        role_id: ["role-1"],
        platform_id: ["platform-1"],
        device_type_id: ["dtype-1"],
      },
    });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/connections/targets", {
      params: {
        query: {
          limit: 50_000,
          offset: 0,
          search: "ares",
          sort: "deviceType",
          order: "desc",
          status: "active",
          target_kind: "device",
          region_id: "region-1",
          site_id: "site-1",
          tenant_id: "tenant-1",
          role_id: "role-1",
          platform_id: "platform-1",
          device_type_id: "dtype-1",
        },
      },
    });
  });
});

describe("openSession transient credentials", () => {
  it("sends an inline VNC password without saving credentials or copying them into session metadata", async () => {
    const password = crypto.randomUUID().replaceAll("-", "").slice(0, 8);
    const input: OpenSessionRequest = {
      hostId: "target-1",
      protocol: "vnc",
      credentials: { password },
    };
    let sent: { target_id: string; protocol: string; params?: { credentials?: Record<string, string> }; metadata?: unknown } | undefined;
    apiMocks.POST.mockImplementation((_path: string, init: { body: unknown }) => {
      sent = JSON.parse(JSON.stringify(init.body));
      return Promise.resolve({
        data: {
          session: { id: "session-1", target_id: input.hostId, protocol: "vnc", status: "active" },
          proxy: { connection_id: "session-1", websocket_url: "wss://example.test/vnc/session-1" },
        },
      });
    });

    const result = await openSession(input);

    expect(apiMocks.POST).toHaveBeenCalledTimes(1);
    expect(apiMocks.POST.mock.calls[0]?.[0]).toBe("/api/v1/connections/sessions");
    expect(sent?.params?.credentials?.password === password).toBe(true);
    expect(Boolean(sent?.params?.credentials?.password)).toBe(true);
    expect(sent?.target_id).toBe(input.hostId);
    expect(sent?.protocol).toBe("vnc");
    expect(sent?.metadata).toBeUndefined();
    expect(JSON.stringify(result.session).includes(password)).toBe(false);
    expect(apiMocks.PUT).not.toHaveBeenCalled();
  });
});
