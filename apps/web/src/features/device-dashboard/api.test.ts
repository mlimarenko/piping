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

import {
  getDeviceAuthorities,
  getDeviceDisk,
  getDeviceInfo,
  getDeviceRecording,
  getDeviceUsers,
} from "./api";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("getDeviceInfo", () => {
  it("GETs /api/v1/cctv/{target_id}/info keyed by the target id and returns the unwrapped body", async () => {
    const info = { model: "NVR-16", firmware: "V5.2.1", serial: "SN-1", channels: 16, device_time: null };
    apiMocks.GET.mockResolvedValue({ data: info });

    const result = await getDeviceInfo("target-42");

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/cctv/{target_id}/info", {
      params: { path: { target_id: "target-42" } },
    });
    expect(result).toBe(info);
  });

  it("propagates the backend error instead of swallowing it", async () => {
    apiMocks.GET.mockResolvedValue({ error: "no device credentials configured" });

    await expect(getDeviceInfo("target-42")).rejects.toThrow("no device credentials configured");
  });
});

describe("getDeviceUsers", () => {
  it("GETs /api/v1/device-fleet/{target_id}/users keyed by the target id and returns the unwrapped body", async () => {
    const users = { users: [{ name: "admin", group: "Administrators", authorities: ["all"] }] };
    apiMocks.GET.mockResolvedValue({ data: users });

    const result = await getDeviceUsers("target-7");

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/device-fleet/{target_id}/users", {
      params: { path: { target_id: "target-7" } },
    });
    expect(result).toBe(users);
  });
});

describe("getDeviceAuthorities", () => {
  it("GETs the target authority catalog and returns verbatim tokens", async () => {
    const catalog = { authorities: ["Monitor_01", "Playback"] };
    apiMocks.GET.mockResolvedValue({ data: catalog });

    const result = await getDeviceAuthorities("target-7");

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/cctv/{target_id}/authorities", {
      params: { path: { target_id: "target-7" } },
    });
    expect(result).toBe(catalog);
  });
});

describe("getDeviceDisk", () => {
  it("GETs /api/v1/cctv/{target_id}/disk keyed by the target id and returns the unwrapped body", async () => {
    const disk = {
      disks: [{ name: "HDD1", total_bytes: 100, used_bytes: 50, state: "ok" as const, is_error: false }],
    };
    apiMocks.GET.mockResolvedValue({ data: disk });

    const result = await getDeviceDisk("target-9");

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/cctv/{target_id}/disk", {
      params: { path: { target_id: "target-9" } },
    });
    expect(result).toBe(disk);
  });
});

describe("getDeviceRecording", () => {
  it("GETs /api/v1/cctv/{target_id}/recording keyed by the target id and returns the unwrapped body", async () => {
    const recording = { active: true, oldest: null, newest: null, spans: [] };
    apiMocks.GET.mockResolvedValue({ data: recording });

    const result = await getDeviceRecording("target-3");

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/cctv/{target_id}/recording", {
      params: { path: { target_id: "target-3" } },
    });
    expect(result).toBe(recording);
  });
});
