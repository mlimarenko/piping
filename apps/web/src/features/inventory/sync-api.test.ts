import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  DELETE: vi.fn(),
  GET: vi.fn(),
  POST: vi.fn(),
  PATCH: vi.fn(),
}));

vi.mock("@/shared/api/client", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>();
  return {
    ...actual,
    apiClient: apiMocks,
    unwrap: (result: { data?: unknown; error?: unknown }) => {
      if (result.error) {
        if (result.error instanceof Error) throw result.error;
        throw new Error(typeof result.error === "string" ? result.error : "API mock error");
      }
      return result.data;
    },
  };
});

import { ApiError } from "@/shared/api/client";
import { getLatestSyncJob, listProviders, updateProvider } from "./sync-api";

const SOURCE = {
  id: "01a0f419-0000-4000-8000-000000000001",
  name: "Printum NetBox",
  provider_type: "netbox",
  config: { base_url: "http://172.31.0.1:18001" },
  is_enabled: true,
  secret_fields: [{ field_key: "api_token", has_value: true, enabled: true }],
};

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("listProviders projection", () => {
  it("maps secret-field presence and never invents transport state", async () => {
    apiMocks.GET.mockResolvedValue({
      data: { items: [SOURCE], total: 1, limit: 25, offset: 0 },
    });

    const page = await listProviders({ limit: 25, offset: 0 });
    const provider = page.items[0];

    expect(provider?.secretFields).toEqual([
      { fieldKey: "api_token", hasValue: true, enabled: true },
    ]);
    // The wire carries no per-provider status/last-sync/last-error; the UI must
    // not fabricate them.
    expect(provider).not.toHaveProperty("status");
    expect(provider).not.toHaveProperty("lastSync");
    expect(provider).not.toHaveProperty("lastError");
  });
});

describe("updateProvider", () => {
  it("sends only the provided config patch so the backend merge keeps the stored token", async () => {
    apiMocks.PATCH.mockResolvedValue({ data: SOURCE });

    await updateProvider(SOURCE.id, { config: { base_url: "http://new.example.com" } });

    expect(apiMocks.PATCH).toHaveBeenCalledWith("/api/v1/providers/{id}", {
      params: { path: { id: SOURCE.id } },
      body: {
        name: undefined,
        config: { base_url: "http://new.example.com" },
        is_enabled: undefined,
      },
    });
  });
});

describe("getLatestSyncJob", () => {
  it("projects the latest job from the bounded provider-scoped endpoint", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        id: "job-1",
        status: "completed",
        started_at: "2026-02-01T00:00:00Z",
        finished_at: "2026-02-01T00:01:00Z",
        stats: { created: 3 },
      },
    });

    const job = await getLatestSyncJob(SOURCE.id);

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/sync/jobs/latest", {
      params: { query: { provider_id: SOURCE.id } },
    });
    expect(job).toEqual({
      id: "job-1",
      status: "completed",
      startedAt: "2026-02-01T00:00:00Z",
      finishedAt: "2026-02-01T00:01:00Z",
      stats: { created: 3 },
    });
  });

  it("treats a 404 as an empty state, not a failure", async () => {
    apiMocks.GET.mockResolvedValue({
      error: new ApiError({ status: 404, message: "No sync job for provider" }),
    });

    await expect(getLatestSyncJob(SOURCE.id)).resolves.toBeNull();
  });
});
