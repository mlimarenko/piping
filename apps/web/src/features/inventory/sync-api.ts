import { apiClient, ApiError, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";
import type {
  ListQuery,
  Paginated,
  SyncJob,
  SyncJobStatus,
  SyncProvider,
  TestConnectionResult,
} from "@/shared/api/types";

// ── Providers ────────────────────────────────────────────

type BackendProvider = components["schemas"]["SourceResponse"];
type ProviderListQuery = ListQuery & {
  order?: "asc" | "desc";
  provider_type?: string;
  is_enabled?: string;
};

function toSyncProvider(raw: BackendProvider): SyncProvider {
  return {
    id: raw.id,
    providerId: raw.provider_type,
    name: raw.name,
    // SourceResponse.config is `unknown` in the contract; the UI provider model
    // consumes a string-valued config record. Secrets are omitted by the wire.
    config: raw.config as Record<string, string>,
    enabled: raw.is_enabled,
    secretFields: (raw.secret_fields ?? []).map((secret) => ({
      fieldKey: secret.field_key,
      hasValue: secret.has_value,
      enabled: secret.enabled,
    })),
  };
}

export async function listProviders(q?: ProviderListQuery): Promise<Paginated<SyncProvider>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/providers", {
      params: {
        query: {
          limit: q?.limit,
          offset: q?.offset,
          sort: q?.sort,
          order: q?.order,
          provider_type: q?.provider_type ?? q?.filter?.provider_type,
          is_enabled: q?.is_enabled ?? q?.filter?.is_enabled,
        },
      },
    }),
  );
  const items = (raw.items ?? []).map((provider) => toSyncProvider(provider));
  return { items, total: raw.total, limit: raw.limit, offset: raw.offset };
}

export async function createProvider(
  body: Omit<SyncProvider, "id" | "secretFields">,
): Promise<SyncProvider> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/providers", {
      body: {
        provider_type: body.providerId,
        name: body.name,
        config: body.config,
        is_enabled: body.enabled,
      },
    }),
  );
  return toSyncProvider(raw);
}

export async function updateProvider(
  id: string,
  body: Partial<SyncProvider>,
): Promise<SyncProvider> {
  const raw = unwrap(
    await apiClient.PATCH("/api/v1/providers/{id}", {
      params: { path: { id } },
      body: {
        name: body.name,
        config: body.config,
        is_enabled: body.enabled,
      },
    }),
  );
  return toSyncProvider(raw);
}

export async function deleteProvider(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/providers/{id}", {
    params: { path: { id } },
  });
}

export async function validateProvider(id: string): Promise<TestConnectionResult> {
  // validate_provider's 200 body is `unknown` in the contract; narrow to the
  // shape the UI reads (`valid`/`message`) and pass the raw object through as meta.
  const raw = unwrap(
    await apiClient.POST("/api/v1/providers/{id}/validate", {
      params: { path: { id } },
    }),
  ) as { valid?: boolean; message?: string; [key: string]: unknown };
  return {
    ok: raw.valid ?? true,
    message: raw.message,
    meta: raw,
  };
}

// ── Sync jobs ────────────────────────────────────────────

type BackendSyncJob = components["schemas"]["SyncJobResponse"];

function toSyncJob(raw: BackendSyncJob): SyncJob {
  if (!["queued", "running", "completed", "failed"].includes(raw.status)) {
    throw new Error(`Unsupported sync job status: ${raw.status}`);
  }
  return {
    id: raw.id,
    providerId: raw.provider_id,
    mappingVersion: raw.mapping_version,
    status: raw.status as SyncJob["status"],
    startedAt: raw.started_at ?? undefined,
    finishedAt: raw.finished_at ?? undefined,
    // SyncJobResponse.stats is `unknown` in the contract; the UI exposes a
    // structured stats record.
    stats: raw.stats as Record<string, unknown>,
  };
}

export async function createSyncJob(
  providerId: string,
  mappingVersion: string,
): Promise<SyncJob> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/sync/jobs", {
      body: {
        provider_id: providerId,
        mapping_version: mappingVersion,
      },
    }),
  );
  return toSyncJob(raw);
}

/** Latest job for one provider, or `null` when the provider has never run. */
export interface LatestSyncJob {
  id: string;
  status: SyncJobStatus;
  startedAt?: string;
  finishedAt?: string;
  stats: Record<string, unknown>;
}

export async function getLatestSyncJob(providerId: string): Promise<LatestSyncJob | null> {
  let raw: components["schemas"]["SyncJobReportResponse"];
  try {
    raw = unwrap(
      await apiClient.GET("/api/v1/sync/jobs/latest", {
        params: { query: { provider_id: providerId } },
      }),
    );
  } catch (error) {
    // The bounded endpoint answers 404 when the provider has no job yet; that
    // is an empty state, not a failure.
    if (error instanceof ApiError && error.status === 404) return null;
    throw error;
  }
  if (!["queued", "running", "completed", "failed"].includes(raw.status)) {
    throw new Error(`Unsupported sync job status: ${raw.status}`);
  }
  return {
    id: raw.id,
    status: raw.status as SyncJobStatus,
    startedAt: raw.started_at ?? undefined,
    finishedAt: raw.finished_at ?? undefined,
    stats: raw.stats as Record<string, unknown>,
  };
}
