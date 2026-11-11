import { beforeEach, describe, expect, it, vi } from "vitest";
import type * as ApiClient from "@/shared/api/client";
import { ApiError } from "@/shared/api/client";

const apiMocks = vi.hoisted(() => ({
  DELETE: vi.fn(),
  GET: vi.fn(),
  PATCH: vi.fn(),
  POST: vi.fn(),
  PUT: vi.fn(),
}));

vi.mock("@/shared/api/client", async (original) => ({
  ...await original<typeof ApiClient>(),
  apiClient: apiMocks,
  unwrap: (result: { data?: unknown; error?: unknown }) => {
    if (result.error instanceof Error) throw result.error;
    if (typeof result.error === "string") throw new Error(result.error);
    if (result.error) throw new Error("API mock error");
    return result.data;
  },
}));

import { changeProjectRevision, forkProject, getProjectRevision, getRunGroupConsole, launchJobTemplate, syncProject, type ChangeProjectRevisionRequest } from "./api";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("durable console API", () => {
  it("uses the inclusive cursor and dispatch guard without converting JSON byte arrays", async () => {
    const page = { dispatch_id: "dispatch", dispatch_attempt: 1, chunks: [{ sequence: 7, stream: "stderr", bytes: [0, 255, 27, 13] }], next_sequence: 8, complete: false };
    const signal = new AbortController().signal;
    apiMocks.GET.mockResolvedValue({ data: page });
    await expect(getRunGroupConsole("group/one", { fromSequence: 7, dispatchId: "dispatch", signal })).resolves.toEqual(page);
    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/playbooks/run-groups/{group_id}/console", {
      params: { path: { group_id: "group/one" }, query: { from_sequence: 7, limit: 64, dispatch_id: "dispatch" } }, signal,
    });
  });

  it("keeps queued identity null and omits the dispatch guard before acquisition", async () => {
    const page = { dispatch_id: null, dispatch_attempt: 0, chunks: [], next_sequence: 0, complete: false };
    apiMocks.GET.mockResolvedValue({ data: page });
    await expect(getRunGroupConsole("group", { fromSequence: 0, signal: new AbortController().signal })).resolves.toEqual(page);
    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/playbooks/run-groups/{group_id}/console", expect.objectContaining({
      params: { path: { group_id: "group" }, query: { from_sequence: 0, limit: 64, dispatch_id: undefined } },
    }));
  });

  it("preserves a dispatch conflict as an API error rather than a queued page", async () => {
    apiMocks.GET.mockRejectedValue(new ApiError({ status: 409, code: "CONFLICT", message: "Dispatch identity mismatch" }));
    await expect(getRunGroupConsole("group", { fromSequence: 8, dispatchId: "dispatch", signal: new AbortController().signal }))
      .rejects.toMatchObject({ name: "ApiError", status: 409, code: "CONFLICT" });
  });

  it("passes cancellation through and does not retry or fabricate output", async () => {
    const controller = new AbortController();
    const aborted = new DOMException("Aborted", "AbortError");
    apiMocks.GET.mockRejectedValue(aborted);
    controller.abort();
    await expect(getRunGroupConsole("group", { fromSequence: 3, signal: controller.signal })).rejects.toBe(aborted);
    expect(apiMocks.GET).toHaveBeenCalledTimes(1);
  });

  it("rejects an invalid cursor without making a request", async () => {
    await expect(getRunGroupConsole("group", { fromSequence: -1, signal: new AbortController().signal })).rejects.toThrow("Invalid console replay request");
    expect(apiMocks.GET).not.toHaveBeenCalled();
  });
});

describe("job template launch API", () => {
  it("sends selected target IDs and metadata without retired overrides", async () => {
    const body = { target_ids: ["target-1", "target-2"], metadata: { source: "connections.bulk_launch" } };
    apiMocks.POST.mockResolvedValue({ data: { run_group: { id: "group" }, runs: [] } });
    await launchJobTemplate("job", body);
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/playbooks/job-templates/{id}/launch", {
      params: { path: { id: "job" } }, body,
    });
    expect(apiMocks.POST).toHaveBeenCalledTimes(1);
  });

  it("distinguishes saved membership from an explicit empty selection", async () => {
    apiMocks.POST.mockResolvedValue({ data: { run_group: { id: "group" }, runs: [] } });
    await launchJobTemplate("job");
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/playbooks/job-templates/{id}/launch", {
      params: { path: { id: "job" } }, body: {},
    });
    await launchJobTemplate("job", { target_ids: [] });
    expect(apiMocks.POST).toHaveBeenLastCalledWith("/api/v1/playbooks/job-templates/{id}/launch", {
      params: { path: { id: "job" } }, body: { target_ids: [] },
    });
  });
});

describe("project revision API", () => {
  it("reads an immutable project revision", async () => {
    const revision = { revision_id: "revision", entries: [] };
    apiMocks.GET.mockResolvedValue({ data: revision });
    await expect(getProjectRevision("project", "revision")).resolves.toBe(revision);
    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/playbooks/projects/{id}/revisions/{revision_id}", { params: { path: { id: "project", revision_id: "revision" } } });
  });

  it("preserves base identity, binary bytes, executable mode and symlinks in one revision request", async () => {
    const body: ChangeProjectRevisionRequest = { base_revision_id: "base", changes: [
      { operation: "put", entry: { path: "bin/run", content: { kind: "file", bytes: [0, 255, 65], executable: true } } },
      { operation: "put", entry: { path: "link", content: { kind: "symlink", target: "bin/run" } } },
      { operation: "rename", from: "roles/old", to: "roles/app" },
      { operation: "delete", path: "unused" },
    ] };
    const revision = { revision_id: "next", entries: [] };
    apiMocks.POST.mockResolvedValue({ data: revision });
    await expect(changeProjectRevision("project", body)).resolves.toBe(revision);
    expect(apiMocks.POST).toHaveBeenCalledWith("/api/v1/playbooks/projects/{id}/revisions", { params: { path: { id: "project" } }, body });
    expect(apiMocks.POST).toHaveBeenCalledTimes(1);
  });

  it("does not retry a stale-base conflict with a different revision", async () => {
    const conflict = new ApiError({ status: 409, message: "stale base revision" });
    apiMocks.POST.mockRejectedValue(conflict);
    await expect(changeProjectRevision("project", { base_revision_id: "base", changes: [] })).rejects.toBe(conflict);
    expect(apiMocks.POST).toHaveBeenCalledTimes(1);
  });

  it("returns actual sync and fork responses using explicit revision and environment inputs", async () => {
    const revision = { revision_id: "imported", entries: [] };
    const project = { id: "fork", current_revision_id: "copied" };
    apiMocks.POST.mockResolvedValueOnce({ data: revision }).mockResolvedValueOnce({ data: project });
    await expect(syncProject("project", { base_revision_id: "base", execution_environment_id: "environment" })).resolves.toBe(revision);
    await expect(forkProject("project", { revision_id: "imported", name: "Copy" })).resolves.toBe(project);
    expect(apiMocks.POST).toHaveBeenNthCalledWith(1, "/api/v1/playbooks/projects/{id}/sync", { params: { path: { id: "project" } }, body: { base_revision_id: "base", execution_environment_id: "environment" } });
    expect(apiMocks.POST).toHaveBeenNthCalledWith(2, "/api/v1/playbooks/projects/{id}/fork", { params: { path: { id: "project" } }, body: { revision_id: "imported", name: "Copy" } });
  });
});
