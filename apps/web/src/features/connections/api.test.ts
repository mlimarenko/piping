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

import { buildConnectionTableParams, listActiveSessions, listConnectionTargetPage, listConnectionTargets, openSession } from "./api";
import { saveSessionViewSettings } from "./sessionViewPreferences";
import type { OpenSessionRequest } from "@/shared/api/types";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
  window.localStorage.clear();
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

describe("listConnectionTargetPage", () => {
  it("sends the requested page and search and maps the envelope", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [{
          id: "11111111-1111-1111-1111-111111111111",
          name: "edge-250",
          target_kind: "device",
          address: "10.0.0.250",
          address_bound: true,
        }],
        total: 120,
        limit: 50,
        offset: 100,
      },
    });

    const result = await listConnectionTargetPage({ limit: 50, offset: 100, search: "edge" });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/connections/targets", {
      params: { query: { limit: 50, offset: 100, search: "edge" } },
    });
    expect(result.total).toBe(120);
    expect(result.items[0]).toMatchObject({
      id: "11111111-1111-1111-1111-111111111111",
      name: "edge-250",
      primaryIp: "10.0.0.250",
      addressBound: true,
    });
  });
});

describe("listActiveSessions presence", () => {
  it("normalizes untrusted presence and rejects malformed observe capabilities", async () => {
    apiMocks.GET.mockResolvedValue({ data: {
      items: [{ id: "session-1", user_id: "user-1", target_id: "target-1", protocol: "ssh", status: "active", started_at: "now", last_activity: "now", presence: { state: "unexpected", observer_count: -1, operator_attached: "yes" }, observe: { allowed: true, transport: "unexpected" } }],
      total: 1, limit: 10, offset: 0,
    } });
    const result = await listActiveSessions();
    expect(result.items[0]?.presence).toMatchObject({ state: "unknown", observer_count: null, operator_attached: null });
    expect(result.items[0]?.observe).toBeNull();
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

  it("adds a saved VNC quality preset to launch params without touching credentials", async () => {
    saveSessionViewSettings("target-quality", "vnc", { quality: "data_saver" });
    let sent: { params?: { options?: Record<string, unknown> } } | undefined;
    apiMocks.POST.mockImplementation((_path: string, init: { body: unknown }) => {
      sent = JSON.parse(JSON.stringify(init.body));
      return Promise.resolve({
        data: {
          session: { id: "session-2", target_id: "target-quality", protocol: "vnc", status: "active" },
          proxy: { connection_id: "session-2", websocket_url: "wss://example.test/vnc/session-2" },
        },
      });
    });

    await openSession({ hostId: "target-quality", protocol: "vnc" });

    expect(sent?.params?.options).toEqual({ color_depth: 8, force_lossless: false });
  });

  it("keeps the connection defaults when only the display mode is overridden", async () => {
    saveSessionViewSettings("target-display", "vnc", { displayMode: "actual" });
    let sent: { params?: unknown } | undefined;
    apiMocks.POST.mockImplementation((_path: string, init: { body: unknown }) => {
      sent = JSON.parse(JSON.stringify(init.body));
      return Promise.resolve({
        data: {
          session: { id: "session-3", target_id: "target-display", protocol: "vnc", status: "active" },
          proxy: { connection_id: "session-3", websocket_url: "wss://example.test/vnc/session-3" },
        },
      });
    });

    await openSession({ hostId: "target-display", protocol: "vnc" });

    expect(sent?.params).toBeUndefined();
  });
});
