import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  DELETE: vi.fn(),
  GET: vi.fn(),
  PATCH: vi.fn(),
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

import { getProjectFiles, uploadProjectFiles } from "./api";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("project files API", () => {
  it("reads the stored file set for a project", async () => {
    apiMocks.GET.mockResolvedValue({
      data: { files: [{ path: "site.yml", content: "a", executable: false }] },
    });

    await getProjectFiles("project-1");

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/playbooks/projects/{id}/files", {
      params: { path: { id: "project-1" } },
    });
  });

  it("posts the whole file set to the project files endpoint", async () => {
    apiMocks.POST.mockResolvedValue({ data: { id: "project-1" } });

    const files = [
      { path: "site.yml", content: "a", executable: false },
      { path: "bin/run", content: "#!sh", executable: true },
    ];
    await uploadProjectFiles("project-1", files);

    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/playbooks/projects/{id}/files", {
      params: { path: { id: "project-1" } },
      body: { files },
    });
  });
});
