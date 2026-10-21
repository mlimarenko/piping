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

import { changeMyPassword } from "./api";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("changeMyPassword", () => {
  it("sends current and new passwords to the self-service endpoint", async () => {
    apiMocks.PUT.mockResolvedValue({
      data: { user_id: "u1", password_changed_at: "2026-06-16T09:00:00Z" },
    });

    await changeMyPassword({
      currentPassword: "current-password",
      newPassword: "new-password",
    });

    expect(apiMocks.PUT).toHaveBeenCalledWith("/api/v1/user/settings/password", {
      body: {
        current_password: "current-password",
        new_password: "new-password",
      },
    });
  });
});
