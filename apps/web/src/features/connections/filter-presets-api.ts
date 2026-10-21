import { apiClient, unwrap, type components } from "@/shared/api/client";
import type {
  SavedFilterPreset,
  SerializableFilterState,
} from "@/features/connections/components/connections/filters-utils";

type BackendPreset = components["schemas"]["ConnectionFilterPresetResponse"];

function fromBackendPreset(raw: BackendPreset): SavedFilterPreset {
  return {
    id: raw.id,
    name: raw.name,
    visibility: "public",
    filters: raw.filters,
    createdBy: raw.created_by,
    createdAt: raw.created_at,
    updatedAt: raw.updated_at,
  };
}

export async function listPublicConnectionFilterPresets(): Promise<SavedFilterPreset[]> {
  const response = unwrap(await apiClient.GET("/api/v1/connections/filter-presets", {}));
  return response.items.map(fromBackendPreset);
}

export async function createPublicConnectionFilterPreset(input: {
  name: string;
  filters: SerializableFilterState;
}): Promise<SavedFilterPreset> {
  const response = unwrap(
    await apiClient.POST("/api/v1/connections/filter-presets", {
      body: {
        name: input.name,
        filters: input.filters,
      },
    }),
  );
  return fromBackendPreset(response);
}

export async function deletePublicConnectionFilterPreset(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/connections/filter-presets/{id}", {
    params: { path: { id } },
  });
}
