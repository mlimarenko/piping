import type { InventoryEntityKey } from "@/shared/api/types";
import { fieldsByTab, inventoryModelCapabilities } from "./inventoryTypes";

/**
 * Completeness layer for the read-only inspector. The curated
 * {@link inventoryInspectorFields} only enumerates the CRUD-form (editable)
 * subset; this module surfaces every REMAINING field the backend GET returns
 * (slug, external_id, sync timestamps, tags, asns, custom fields, …) so the
 * inspector can render the entity's full field set.
 *
 * It reads the RAW snake_case backend record (not the mapped `Resource`, which
 * drops these keys), de-duplicating against the curated rows so a field is never
 * rendered twice.
 */

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** camelCase Resource key → snake_case backend key (`regionId` → `region_id`). */
export function snakeKey(key: string): string {
  return key.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
}

/** Human-readable fallback label for an unmapped snake_case key. */
export function humanizeKey(key: string): string {
  return key
    .replace(/_id$/, "")
    .split("_")
    .filter(Boolean)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

/**
 * Backend keys the inspector already renders OUTSIDE the metadata-driven field
 * loop (header, status row, icon plumbing, alternate custom-field carriers).
 */
const STRUCTURAL_KEYS: ReadonlySet<string> = new Set([
  "id",
  "name",
  "status",
  "type",
  "display",
  "url",
  "icon",
  "icon_name",
  "icon_data_url",
  "custom_fields",
  "custom_field_data",
  "role_custom_fields",
]);

/**
 * Snake keys rendered by the host-specific overview/detail header
 * (mirrors `HOST_OVERVIEW_FIELDS` + `HOST_DETAIL_OMIT_KEYS` + the primary-IP row
 * in `TreeNodeInspector`). Kept here so the generic pass never double-renders
 * device/VM rows already shown above.
 */
const HOST_RENDERED_KEYS: ReadonlySet<string> = new Set([
  "role",
  "role_id",
  "platform",
  "platform_id",
  "device_type",
  "device_type_id",
  "region",
  "region_id",
  "site",
  "site_id",
  "tenant",
  "tenant_id",
  "cluster",
  "cluster_id",
  "primary_ip",
  "primary_ip4_id",
  "primary_ip6_id",
]);

/**
 * The set of snake_case backend keys already covered by the curated
 * (form-derived) inspector rows for `entity`, so the generic completeness pass
 * can skip them. Includes each curated field's display key AND its FK id key.
 */
export function curatedSnakeKeys(entity: InventoryEntityKey, host: boolean): Set<string> {
  const keys = new Set<string>(STRUCTURAL_KEYS);
  const capabilities = inventoryModelCapabilities(entity);
  if (capabilities?.tags === false) keys.add("tags");
  if (capabilities?.change_dates === false) {
    keys.add("created_at");
    keys.add("updated_at");
    keys.add("created");
    keys.add("last_updated");
  }
  for (const field of fieldsByTab[entity] ?? []) {
    // `nested-children` is a host-rendered inline editor, not a backend scalar
    // column — it contributes no curated snake key.
    if (field.type === "nested-children") continue;
    keys.add(snakeKey(field.key));
    if (field.idKey) keys.add(snakeKey(field.idKey));
  }
  if (host) {
    for (const key of HOST_RENDERED_KEYS) keys.add(key);
  }
  return keys;
}

/** Known FK id keys whose value is a UUID we can deep-link to an inventory tab. */
const FK_KEY_ENTITY: Readonly<Record<string, InventoryEntityKey>> = {
  manufacturer_id: "Manufacturers",
  rir_id: "RIRs",
  vrf_id: "VRFs",
  group_id: "Tenant Groups",
  parent_id: "Regions",
  provider_id: "Providers",
  cluster_type_id: "Cluster Types",
  cluster_group_id: "Cluster Groups",
};

/** i18n keys for read-only backend fields the curated form spec never carries. */
const KNOWN_LABEL_I18N: Readonly<Record<string, string>> = {
  slug: "inventory.resources.fields.slug",
  external_id: "inventory.resources.fields.externalId",
  piping_synced_at: "inventory.resources.fields.pipingSyncedAt",
  created_at: "inventory.resources.fields.createdAt",
  updated_at: "inventory.resources.fields.updatedAt",
  comments: "inventory.resources.fields.comments",
  description: "inventory.resources.fields.description",
  tags: "inventory.resources.fields.tags",
  asns: "inventory.resources.fields.asns",
};

/** Display priority for known keys; lower sorts first. Unknown keys sort last. */
const KNOWN_ORDER: readonly string[] = [
  "slug",
  "comments",
  "tags",
  "asns",
  "external_id",
  "piping_synced_at",
  "created_at",
  "updated_at",
];

export type GenericFieldKind = "scalar" | "longtext" | "link";

export interface GenericFieldRow {
  /** Raw snake_case key (stable React key). */
  key: string;
  /** i18n key when a known label exists; otherwise undefined. */
  i18nKey?: string;
  /** Humanized literal fallback (use when `i18nKey` is absent). */
  label: string;
  value: string;
  kind: GenericFieldKind;
  /** For `link` rows: the inventory entity the FK points at. */
  relatedTab?: InventoryEntityKey;
  /** For `link` rows: the FK UUID. */
  linkId?: string;
  isEmpty: boolean;
}

export interface CustomFieldRow {
  /** Raw custom-field name as stored on the backend record. */
  key: string;
  /** Human-readable fallback label derived from the raw key. */
  label: string;
  /** Display-ready value. Can contain line breaks for object-like values. */
  value: string;
}

function scalarString(value: unknown): string {
  if (value == null) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "bigint" || typeof value === "boolean") {
    return value.toString();
  }
  if (typeof value === "object") return JSON.stringify(value);
  // symbol / function never appear in a JSON backend record; ignore defensively.
  return "";
}

/** Best-effort human label for a collection item (tag/asn objects or scalars). */
function itemLabel(item: unknown): string {
  if (item != null && typeof item === "object") {
    const record = item as Record<string, unknown>;
    const candidate = record.name ?? record.asn ?? record.display ?? record.slug ?? record.id;
    if (candidate != null) return scalarString(candidate);
    return JSON.stringify(item);
  }
  return scalarString(item);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value != null && typeof value === "object" && !Array.isArray(value);
}

function customFieldRecord(
  raw?: Record<string, unknown>,
  fallback?: Record<string, unknown> | null,
): Record<string, unknown> | undefined {
  const candidates = [raw?.custom_fields, raw?.custom_field_data, fallback];
  for (const candidate of candidates) {
    if (isRecord(candidate) && Object.keys(candidate).length > 0) return candidate;
  }
  return undefined;
}

interface FormattedValue {
  value: string;
  isEmpty: boolean;
  long: boolean;
}

export function formatInventoryValue(value: unknown): FormattedValue {
  if (value == null) return { value: "", isEmpty: true, long: false };
  if (Array.isArray(value)) {
    if (value.length === 0) return { value: "", isEmpty: true, long: false };
    const joined = value.map(itemLabel).join(", ");
    return { value: joined, isEmpty: joined === "", long: joined.length > 60 };
  }
  if (typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>).filter(
      ([, raw]) => raw != null && raw !== "",
    );
    if (entries.length === 0) return { value: "", isEmpty: true, long: false };
    const lines = entries.map(([key, raw]) => `${humanizeKey(key)}: ${scalarString(raw)}`);
    return { value: lines.join("\n"), isEmpty: false, long: true };
  }
  const text = scalarString(value);
  return { value: text, isEmpty: text === "", long: text.length > 80 || text.includes("\n") };
}

/**
 * Extract entity user-defined custom fields into standalone rows. The generic
 * completeness pass intentionally skips these backend maps so the inspector can
 * render them as a dedicated collapsible section instead of one multiline
 * "custom fields" blob.
 */
export function customFieldRows(
  raw?: Record<string, unknown>,
  fallback?: Record<string, unknown> | null,
): CustomFieldRow[] {
  const fields = customFieldRecord(raw, fallback);
  if (!fields) return [];

  const rows: CustomFieldRow[] = [];
  for (const [key, rawValue] of Object.entries(fields)) {
    const formatted = formatInventoryValue(rawValue);
    if (formatted.isEmpty) continue;
    rows.push({
      key,
      label: humanizeKey(key),
      value: formatted.value,
    });
  }
  return rows;
}

/**
 * Build the read-only "remaining fields" rows for an entity from its raw backend
 * record, excluding everything already rendered by the curated inspector rows.
 * Rows are returned populated-first, each in a stable order; empties carry
 * `isEmpty: true` so the caller can collapse them.
 */
export function genericInspectorRows(
  raw: Record<string, unknown>,
  alreadyRendered: ReadonlySet<string>,
): GenericFieldRow[] {
  const rows: GenericFieldRow[] = [];
  for (const [key, value] of Object.entries(raw)) {
    if (alreadyRendered.has(key)) continue;

    // FK UUID: deep-link when the target tab is known; skip raw UUIDs otherwise
    // (an opaque UUID is noise, and real FKs are usually covered by curated rows).
    if (key.endsWith("_id") && typeof value === "string" && UUID_RE.test(value)) {
      const relatedTab = FK_KEY_ENTITY[key];
      if (!relatedTab) continue;
      rows.push({
        key,
        i18nKey: KNOWN_LABEL_I18N[key],
        label: humanizeKey(key),
        value: `${value.slice(0, 8)}…`,
        kind: "link",
        relatedTab,
        linkId: value,
        isEmpty: false,
      });
      continue;
    }

    const formatted = formatInventoryValue(value);
    rows.push({
      key,
      i18nKey: KNOWN_LABEL_I18N[key],
      label: humanizeKey(key),
      value: formatted.value,
      kind: formatted.long ? "longtext" : "scalar",
      isEmpty: formatted.isEmpty,
    });
  }

  const orderOf = (key: string): number => {
    const index = KNOWN_ORDER.indexOf(key);
    return index === -1 ? KNOWN_ORDER.length : index;
  };
  rows.sort((a, b) => {
    if (a.isEmpty !== b.isEmpty) return a.isEmpty ? 1 : -1;
    const byKnown = orderOf(a.key) - orderOf(b.key);
    if (byKnown !== 0) return byKnown;
    return a.key.localeCompare(b.key);
  });
  return rows;
}
