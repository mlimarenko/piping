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
    if (result.error instanceof Error) throw result.error;
    if (typeof result.error === "string") throw new Error(result.error);
    if (result.error) throw new Error("API mock error");
    return result.data;
  },
}));

import { bulkAssignRecord } from "../api";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("bulkAssignRecord", () => {
  it("posts generated-contract targets and the shared weight to the bulk endpoint", async () => {
    apiMocks.POST.mockResolvedValue({
      data: {
        items: [],
        assigned_count: 0,
        already_assigned_count: 0,
        failed_count: 0,
      },
    });

    await bulkAssignRecord("record-1", ["target-1", "target-2"], 7);

    expect(apiMocks.POST).toHaveBeenCalledWith(
      "/api/v1/connection-config/records/{id}/assignments/bulk",
      {
        params: { path: { id: "record-1" } },
        body: {
          targets: [{ target_id: "target-1" }, { target_id: "target-2" }],
          weight: 7,
        },
      },
    );
  });
});
