/**
 * Custom-fields REST surface (definitions, choice sets, per-entity secret
 * reveal). DTOs are derived from the generated OpenAPI schema — no hand-written
 * mirrors — and the typed `apiClient` + `unwrap` pattern is used throughout.
 *
 * The backend owns the contract; this module is a thin typed wrapper plus the
 * small amount of domain vocabulary the UI shares across the admin page and the
 * entity-detail custom-field section (field types, ui-visibility/editability
 * enums). `object_types` are inventory content-type UUIDs (see the note in
 * {@link CustomFieldDefinition}).
 */
import { apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";
import type { ListQuery, Paginated } from "@/shared/api/types";

// ── Generated DTO aliases ────────────────────────────────────────────────────

export type CustomFieldDefinition = components["schemas"]["CustomFieldDefinitionResponse"];
export type CustomFieldDefinitionCreatePayload =
  components["schemas"]["CustomFieldDefinitionCreatePayload"];
export type CustomFieldDefinitionUpdatePayload =
  components["schemas"]["CustomFieldDefinitionUpdatePayload"];
export type ChoiceSet = components["schemas"]["ChoiceSetResponse"];
export type ChoiceSetCreatePayload = components["schemas"]["ChoiceSetCreatePayload"];
export type ChoiceSetUpdatePayload = components["schemas"]["ChoiceSetUpdatePayload"];
export type Choice = components["schemas"]["ChoiceDto"];
export type ContentType = components["schemas"]["ContentTypeResponse"];

export interface CustomFieldDefinitionListParams extends ListQuery {
  sort?: string;
  order?: "asc" | "desc";
  search?: string;
  field_type?: string;
  group_name?: string;
  ui_visible?: string;
  ui_editable?: string;
  filter_logic?: string;
  choice_set_id?: string;
  related_object_type_id?: string;
  object_type_id?: string;
}

export interface ChoiceSetListParams extends ListQuery {
  sort?: string;
  order?: "asc" | "desc";
  search?: string;
}

// ── Field-type domain (shared vocabulary) ────────────────────────────────────

/** The 13 NetBox custom-field types, in the canonical backend order. */
export const FIELD_TYPES = [
  "text",
  "longtext",
  "integer",
  "decimal",
  "boolean",
  "date",
  "datetime",
  "url",
  "json",
  "select",
  "multiselect",
  "object",
  "multiobject",
] as const;

export type FieldType = (typeof FIELD_TYPES)[number];

export const UI_VISIBLE_OPTIONS = ["always", "if-set", "hidden"] as const;

export const UI_EDITABLE_OPTIONS = ["yes", "no", "hidden"] as const;

/** Field types that draw their value from a choice set. */
export function isChoiceFieldType(type: string): boolean {
  return type === "select" || type === "multiselect";
}

/** Field types that reference another inventory object (need a related type). */
export function isObjectFieldType(type: string): boolean {
  return type === "object" || type === "multiobject";
}

/** Numeric field types that support min/max validation bounds. */
export function isNumericFieldType(type: string): boolean {
  return type === "integer" || type === "decimal";
}

/** Field types where a regex pattern is meaningful. */
export function supportsRegexValidation(type: string): boolean {
  return type === "text" || type === "longtext" || type === "url";
}

/** Field types that hold multiple values. */
export function isMultiFieldType(type: string): boolean {
  return type === "multiselect" || type === "multiobject";
}

// ── Definitions ──────────────────────────────────────────────────────────────

export async function listCustomFields(
  params?: CustomFieldDefinitionListParams,
): Promise<Paginated<CustomFieldDefinition>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/custom-fields", {
      params: { query: params },
    }),
  );
  return {
    items: raw.items ?? [],
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

export async function createCustomField(
  body: CustomFieldDefinitionCreatePayload,
): Promise<CustomFieldDefinition> {
  return unwrap(await apiClient.POST("/api/v1/custom-fields", { body }));
}

export async function updateCustomField(
  id: string,
  body: CustomFieldDefinitionUpdatePayload,
): Promise<CustomFieldDefinition> {
  return unwrap(
    await apiClient.PATCH("/api/v1/custom-fields/{id}", { params: { path: { id } }, body }),
  );
}

export async function deleteCustomField(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/custom-fields/{id}", { params: { path: { id } } });
}

// ── Content types ───────────────────────────────────────────────────────────

export async function listContentTypes(): Promise<ContentType[]> {
  return unwrap(await apiClient.GET("/api/v1/inventory/content-types"));
}

// ── Choice sets ────────────────────────────────────────────────────────────

export async function listChoiceSets(params?: ChoiceSetListParams): Promise<Paginated<ChoiceSet>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/custom-field-choice-sets", {
      params: { query: params },
    }),
  );
  return {
    items: raw.items ?? [],
    total: raw.total,
    limit: raw.limit,
    offset: raw.offset,
  };
}

export async function createChoiceSet(body: ChoiceSetCreatePayload): Promise<ChoiceSet> {
  return unwrap(await apiClient.POST("/api/v1/custom-field-choice-sets", { body }));
}

export async function updateChoiceSet(
  id: string,
  body: ChoiceSetUpdatePayload,
): Promise<ChoiceSet> {
  return unwrap(
    await apiClient.PATCH("/api/v1/custom-field-choice-sets/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteChoiceSet(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/custom-field-choice-sets/{id}", {
    params: { path: { id } },
  });
}

// ── Per-entity secret reveal ────────────────────────────────────────────────

/** Entity-kind path segment for the reveal endpoint and entity-scoped logic. */
export type CustomFieldEntityKind =
  | "sites"
  | "devices"
  | "regions"
  | "clusters"
  | "tenants"
  | "virtual-machines"
  | "virtual-machine-types";

/**
 * Reveal the cleartext of an entity's secret custom fields. Succeeds only when
 * the caller may read that entity's secrets (the backend runs a per-entity
 * `Action::Secret` check and audit-logs the access). Returns a `name -> value`
 * map; non-secret values stay masked, secret values are resolved to cleartext
 * (or `null` when unset).
 */
export async function revealEntityCustomFields(
  entity: CustomFieldEntityKind,
  id: string,
): Promise<Record<string, unknown>> {
  const result = unwrap(
    await apiClient.POST("/api/v1/inventory/{entity}/{id}/custom-fields/reveal", {
      params: { path: { entity, id } },
    }),
  );
  const map = result.custom_fields;
  return map && typeof map === "object" && !Array.isArray(map)
    ? (map as Record<string, unknown>)
    : {};
}
