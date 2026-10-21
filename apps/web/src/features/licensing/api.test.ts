import { beforeEach, describe, expect, it, vi } from "vitest";
import { getLicense } from "./api";

const get = vi.hoisted(() => vi.fn());
vi.mock("@/shared/api/client", () => ({ apiClient: { GET: get }, unwrap: (response: unknown) => response }));
const state = { status: "inactive", seatLimit: 10, seatsUsed: 3, version: 0, validFrom: "", validUntil: "" };

describe("license API boundary", () => {
  beforeEach(() => vi.clearAllMocks());
  it("preserves the server's Community usage", async () => {
    get.mockResolvedValue(state);
    await expect(getLicense()).resolves.toMatchObject({ status: "inactive", seats: { used: 3, total: 10 } });
  });
  it("does not increase a signed entitlement's seat limit", async () => {
    get.mockResolvedValue({ ...state, status: "active", seatLimit: 4 });
    await expect(getLicense()).resolves.toMatchObject({ status: "active", seats: { used: 3, total: 4 } });
  });
  it.each([
    { ...state, status: "unknown" },
    { ...state, status: undefined },
    { ...state, seatsUsed: undefined },
    { ...state, seatLimit: -1 },
  ])("rejects malformed state rather than claiming free access", async raw => {
    get.mockResolvedValue(raw);
    await expect(getLicense()).rejects.toThrow();
  });
});
