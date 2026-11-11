import { apiClient, ApiError, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";
import type {
  MappingConfiguration,
  MappingConfigurationStatus,
  MappingTarget,
  EntityMapping,
  OperatorName,
  InvertibilityHint,
  SyncConflictResolution,
  SyncDirection,
  Transform,
} from "@/shared/api/types";

type BackendMappingConfiguration =
  components["schemas"]["SyncMappingConfigurationResponse"];
type BackendOperatorMeta = components["schemas"]["SyncOperatorResponse"];
type BackendPreviewRequest = components["schemas"]["PreviewMappingRequest"];
type BackendSuggestReverseRequest = components["schemas"]["SuggestReverseRequest"];

function toMappingConfiguration(
  raw: BackendMappingConfiguration,
): MappingConfiguration {
  return {
    id: raw.id,
    logical_id: raw.logical_id,
    name: raw.name,
    version: raw.version,
    is_latest: raw.is_latest,
    provider_id: raw.provider_id,
    target: raw.target,
    status: raw.status as MappingConfiguration["status"],
    direction: raw.direction as MappingConfiguration["direction"],
    conflict_mode: raw.conflict_mode as MappingConfiguration["conflict_mode"],
    // Backend serializes entity_mappings as a free-form object; utoipa emits it
    // as `Record<string, never>[]` (no pinned shape), so narrowing to the real
    // EntityMapping[] needs a structural cast. Fixable only by backend schema work.
    entity_mappings: raw.entity_mappings as unknown as EntityMapping[],
    created_at: raw.created_at,
    created_by: raw.created_by ?? undefined,
    in_flight_job_count: raw.in_flight_job_count,
  };
}

/**
 * The one job shape the backend executes: pull from a provider into the local
 * inventory with NetBox-wins conflict resolution (see
 * `TargetSyncRequest::ensure_supported`). `target` is canonical and explicit:
 * an absent or unknown target is never treated as the local inventory, so a
 * caller cannot silently run a remote-provider mapping against local data.
 *
 * This is the frontend mirror of the backend gate, used by the run-sync picker
 * to restrict runnable configurations through the typed target discriminator.
 */
export interface RunnableMappingShape {
  status: MappingConfigurationStatus;
  direction: SyncDirection;
  conflict_mode: SyncConflictResolution;
  target: MappingTarget;
}

export function isRunnableMappingConfiguration(shape: RunnableMappingShape): boolean {
  return (
    shape.status === "active" &&
    shape.direction === "pull" &&
    shape.conflict_mode === "netbox_wins" &&
    shape.target.kind === "local_inventory"
  );
}

// ── Local inventory target catalog ───────────────────────────────────────────
//
// The canonical entity/field catalogue for the `local_inventory` target. A
// mapping editor reads its source entities and their fields from here, not from
// the remote provider's registry.

export interface LocalInventoryResourceField {
  name: string;
  fieldType: string;
  required: boolean;
}

export interface LocalInventoryResource {
  sourceEntity: string;
  targetEntity: string;
  fields: LocalInventoryResourceField[];
}

export interface LocalInventoryResources {
  resources: LocalInventoryResource[];
  providerEnabled?: boolean;
}

function toLocalInventoryResource(
  raw: components["schemas"]["LocalInventoryResource"],
): LocalInventoryResource {
  return {
    sourceEntity: raw.source_entity,
    targetEntity: raw.target_entity,
    fields: raw.fields.map((field) => ({
      name: field.name,
      fieldType: field.field_type,
      required: field.required,
    })),
  };
}

/** The full local-inventory target catalogue, optionally annotated for a provider. */
export async function listLocalInventoryResources(
  providerId?: string,
): Promise<LocalInventoryResources> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/sync/targets/local-inventory/resources", {
      params: { query: { provider_id: providerId } },
    }),
  );
  return {
    resources: raw.resources.map(toLocalInventoryResource),
    providerEnabled: raw.provider_enabled ?? undefined,
  };
}

/** One catalogue entry by its canonical source entity, or `null` when absent. */
export async function getLocalInventoryResource(
  entity: string,
): Promise<LocalInventoryResource | null> {
  let raw: components["schemas"]["LocalInventoryResourceResponse"];
  try {
    raw = unwrap(
      await apiClient.GET("/api/v1/sync/targets/local-inventory/resources/{entity}", {
        params: { path: { entity } },
      }),
    );
  } catch (error) {
    if (error instanceof ApiError && error.status === 404) return null;
    throw error;
  }
  return toLocalInventoryResource(raw.resource);
}

function toOperatorMeta(raw: BackendOperatorMeta): OperatorMeta {
  return {
    name: raw.name as OperatorName,
    label: raw.label,
    description: raw.description,
    category: raw.category as OperatorMeta["category"],
    invertibility_hint: raw.invertibility_hint as InvertibilityHint,
    args_schema: raw.args_schema,
    // suggested_reverse is a free-form object the backend emits as
    // `Record<string, never>`; narrowing to the real shape needs a structural
    // cast. Fixable only by backend schema work.
    suggested_reverse: raw.suggested_reverse
      ? (raw.suggested_reverse as unknown as OperatorMeta["suggested_reverse"])
      : undefined,
  };
}

// ── List ─────────────────────────────────────────────────────────────────────

export interface ListMappingConfigurationsQuery {
  provider_id?: string;
  logical_id?: string;
  search?: string;
  direction?: string[];
  sort?: "name" | "versions" | "direction" | "provider_id";
  order?: "asc" | "desc";
  limit?: number;
  offset?: number;
}

export interface MappingConfigurationListResponse {
  items: MappingConfiguration[];
  total: number;
  limit: number;
  offset: number;
}

export async function listMappingConfigurations(
  q: ListMappingConfigurationsQuery,
): Promise<MappingConfigurationListResponse> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/sync/mapping-configurations", {
      params: {
        query: {
          provider_id: q.provider_id,
          logical_id: q.logical_id,
          search: q.search,
          direction: q.direction?.join(","),
          sort: q.sort,
          order: q.order,
          limit: q.limit,
          offset: q.offset,
        },
      },
    }),
  );
  return {
    items: (raw.items ?? []).map(toMappingConfiguration),
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

// ── Get ──────────────────────────────────────────────────────────────────────

export async function getMappingConfiguration(id: string): Promise<MappingConfiguration> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/sync/mapping-configurations/{id}", {
      params: { path: { id } },
    }),
  );
  return toMappingConfiguration(raw);
}

// ── Create ───────────────────────────────────────────────────────────────────

export interface CreateMappingConfigurationRequest {
  logical_id: string;
  name: string;
  provider_id: string;
  /** Explicit target; the backend rejects a configuration without one. */
  target: MappingTarget;
  direction: MappingConfiguration["direction"];
  conflict_mode: MappingConfiguration["conflict_mode"];
  entity_mappings: EntityMapping[];
}

export async function createMappingConfiguration(
  body: CreateMappingConfigurationRequest,
): Promise<MappingConfiguration> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/sync/mapping-configurations", { body }),
  );
  return toMappingConfiguration(raw);
}

// ── Update (creates new version) ─────────────────────────────────────────────

export interface UpdateMappingConfigurationRequest {
  name?: string;
  direction?: MappingConfiguration["direction"];
  conflict_mode?: MappingConfiguration["conflict_mode"];
  entity_mappings?: EntityMapping[];
}

export async function updateMappingConfiguration(
  id: string,
  body: UpdateMappingConfigurationRequest,
): Promise<MappingConfiguration> {
  const raw = unwrap(
    await apiClient.PATCH("/api/v1/sync/mapping-configurations/{id}", {
      params: { path: { id } },
      body,
    }),
  );
  return toMappingConfiguration(raw);
}

// ── Delete ───────────────────────────────────────────────────────────────────

export async function deleteMappingConfiguration(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/sync/mapping-configurations/{id}", {
    params: { path: { id } },
  });
}

// ── Sample record ─────────────────────────────────────────────────────────────

export async function getSampleRecord(
  providerId: string,
  entity: string,
): Promise<Record<string, unknown> | null> {
  return unwrap(
    await apiClient.GET("/api/v1/providers/{id}/sample-record", {
      params: {
        path: { id: providerId },
        query: { model: entity },
      },
    }),
  );
}

// ── Preview ───────────────────────────────────────────────────────────────────

export interface PreviewRequest {
  entity_mapping: EntityMapping;
  sample_record: Record<string, unknown>;
}

/** Record-level preview result: the evaluated `result` plus blocking `errors` and non-blocking `warnings`. */
export type PreviewResponse = components["schemas"]["PreviewMappingResultResponse"];

export async function previewMapping(body: PreviewRequest): Promise<PreviewResponse> {
  return unwrap(
    // PreviewMappingRequest.mapping_config / sample_record are free-form objects
    // the backend emits as `Record<string, never>` / `unknown`; mapping the real
    // entity_mapping payload needs a structural cast. Fixable only by backend schema work.
    await apiClient.POST("/api/v1/sync/mapping-configurations/preview", {
      body: body as unknown as BackendPreviewRequest,
    }),
  );
}

// ── Operator registry ─────────────────────────────────────────────────────────

export interface OperatorMeta {
  name: OperatorName;
  label: string;
  description: string;
  category:
    | "identity_literal"
    | "string"
    | "type"
    | "lookup"
    | "reference"
    | "normalization";
  invertibility_hint: InvertibilityHint;
  args_schema: Record<string, unknown>;
  suggested_reverse?: {
    operator: OperatorName;
    args_transform: string;
  };
}

export async function listOperators(): Promise<OperatorMeta[]> {
  const raw = unwrap(await apiClient.GET("/api/v1/sync/operators", {}));
  return raw.map(toOperatorMeta);
}

// ── Validation ────────────────────────────────────────────────────────────────

// ── Suggest reverse ───────────────────────────────────────────────────────────

export interface SuggestReverseRequest {
  forward_transform: Transform;
  source_entity: string;
  source_field: string;
}

export interface SuggestReverseResponse {
  reverse_transform: Transform | null;
  note?: string;
}

export async function suggestReverse(
  body: SuggestReverseRequest,
): Promise<SuggestReverseResponse> {
  // SuggestReverseRequest.forward_transform is a free-form object the backend
  // emits as `Record<string, never>`; the real Transform payload and the
  // free-form `reverse_transform` response both need structural casts.
  // Fixable only by backend schema work.
  return unwrap(
    await apiClient.POST("/api/v1/sync/mapping-configurations/suggest-reverse", {
      body: body as unknown as BackendSuggestReverseRequest,
    }),
  ) as SuggestReverseResponse;
}
