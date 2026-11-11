import type { WebhookEventCatalogItem } from "@/features/webhooks/api";

/**
 * Pure helpers for the webhook event picker and conditions editor.
 *
 * Everything here is side-effect free so it can be unit-tested in isolation and
 * reused by the dialog without dragging React state along. Catalog grouping and
 * the quick-preset bundles are derived dynamically from the runtime
 * event-catalog (the backend list can drift), never hardcoded — the same
 * discipline as the token-scope packs.
 */

// ---------------------------------------------------------------------------
// Event catalog: grouping + quick presets
// ---------------------------------------------------------------------------

/** Stable display ordering for groups; unknown groups fall back to alpha. */
const GROUP_ORDER = ["inventory", "sync", "connections", "templates", "system"];

/** Stable display ordering for actions inside an object row. */
const ACTION_ORDER = [
  "created",
  "updated",
  "deleted",
  "activated",
  "disconnected",
  "failed",
  "terminated",
];

export interface ObjectRow {
  /** Domain object, e.g. `tenant`. */
  object: string;
  /** Catalog items for this object, action-sorted. */
  items: WebhookEventCatalogItem[];
}

export interface EventGroup {
  /** High-level grouping, e.g. `inventory`. */
  group: string;
  /** Object rows within the group, object-name-sorted. */
  objects: ObjectRow[];
  /** Every event value in the group (flattened), for the per-group preset. */
  values: string[];
}

function orderIndex(order: string[], key: string): number {
  const idx = order.indexOf(key);
  return idx === -1 ? order.length : idx;
}

function sortActions(items: WebhookEventCatalogItem[]): WebhookEventCatalogItem[] {
  return [...items].sort((a, b) => {
    const byAction = orderIndex(ACTION_ORDER, a.action) - orderIndex(ACTION_ORDER, b.action);
    return byAction !== 0 ? byAction : a.action.localeCompare(b.action);
  });
}

/**
 * Group the catalog by `group`, then by `object` within each group, in a
 * stable, human-friendly order. Pure: same input → same output.
 */
export function groupCatalog(catalog: WebhookEventCatalogItem[]): EventGroup[] {
  const byGroup = new Map<string, WebhookEventCatalogItem[]>();
  for (const item of catalog) {
    const existing = byGroup.get(item.group);
    if (existing) existing.push(item);
    else byGroup.set(item.group, [item]);
  }

  return [...byGroup.entries()]
    .sort(([a], [b]) => {
      const byOrder = orderIndex(GROUP_ORDER, a) - orderIndex(GROUP_ORDER, b);
      return byOrder !== 0 ? byOrder : a.localeCompare(b);
    })
    .map(([group, groupItems]) => {
      const byObject = new Map<string, WebhookEventCatalogItem[]>();
      for (const item of groupItems) {
        const existing = byObject.get(item.object);
        if (existing) existing.push(item);
        else byObject.set(item.object, [item]);
      }
      const objects: ObjectRow[] = [...byObject.entries()]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([object, items]) => ({ object, items: sortActions(items) }));
      const values = objects.flatMap((o) => o.items.map((i) => i.value));
      return { group, objects, values };
    });
}

/**
 * Filter the catalog by a free-text term, matching object, action, group, or
 * the raw event value (case-insensitive). Empty term returns the catalog as-is.
 */
export function filterCatalog(
  catalog: WebhookEventCatalogItem[],
  term: string,
): WebhookEventCatalogItem[] {
  const needle = term.trim().toLowerCase();
  if (!needle) return catalog;
  return catalog.filter(
    (i) =>
      i.object.toLowerCase().includes(needle) ||
      i.action.toLowerCase().includes(needle) ||
      i.group.toLowerCase().includes(needle) ||
      i.value.toLowerCase().includes(needle),
  );
}

/** A quick-preset bundle: clicking it toggles every `values` event at once. */
export interface EventPreset {
  /** Stable identifier, used as React key. */
  id: string;
  /** What kind of preset, drives the i18n label. */
  kind: "all" | "action" | "group" | "object";
  /** Display token interpolated into the label (action / group / object name). */
  token: string;
  /** Concrete event values this preset toggles. Never empty for a returned preset. */
  values: string[];
}

/**
 * Compute quick presets from the catalog, mirroring the token-scope packs:
 *   - "all": every event
 *   - per action across all groups (e.g. every `created`)
 *   - per group (every event in `inventory`)
 *   - per object (every event for `tenant`)
 *
 * A preset is only returned when it covers at least one catalog event, and only
 * when it is a non-trivial bundle (more than one event, except "all" which is
 * always meaningful). Presets are returned in a stable order: all → actions →
 * groups → objects.
 */
export function computeEventPresets(catalog: WebhookEventCatalogItem[]): EventPreset[] {
  if (catalog.length === 0) return [];
  const presets: EventPreset[] = [];

  const allValues = uniqueValues(catalog.map((i) => i.value));
  presets.push({ id: "all", kind: "all", token: "", values: allValues });

  // Per-action across the whole catalog.
  const actions = uniqueValues(catalog.map((i) => i.action));
  const orderedActions = actions.sort(
    (a, b) => orderIndex(ACTION_ORDER, a) - orderIndex(ACTION_ORDER, b) || a.localeCompare(b),
  );
  for (const action of orderedActions) {
    const values = uniqueValues(catalog.filter((i) => i.action === action).map((i) => i.value));
    if (values.length > 1) {
      presets.push({ id: `action:${action}`, kind: "action", token: action, values });
    }
  }

  // Per group.
  for (const grp of groupCatalog(catalog)) {
    if (grp.values.length > 1) {
      presets.push({ id: `group:${grp.group}`, kind: "group", token: grp.group, values: grp.values });
    }
  }

  // Per object (qualified by group to stay unique across groups).
  const byObject = new Map<string, { token: string; values: string[] }>();
  for (const item of catalog) {
    const key = `${item.group}/${item.object}`;
    const existing = byObject.get(key);
    if (existing) existing.values.push(item.value);
    else byObject.set(key, { token: item.object, values: [item.value] });
  }
  for (const [key, { token, values }] of byObject) {
    const uniq = uniqueValues(values);
    if (uniq.length > 1) {
      presets.push({ id: `object:${key}`, kind: "object", token, values: uniq });
    }
  }

  return presets;
}

function uniqueValues(values: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const v of values) {
    if (!seen.has(v)) {
      seen.add(v);
      out.push(v);
    }
  }
  return out;
}

/** True when every value in `values` is selected. Empty `values` → false. */
export function isBundleActive(values: string[], selected: Set<string>): boolean {
  return values.length > 0 && values.every((v) => selected.has(v));
}

/**
 * Toggle a bundle of event values against the current selection. If every value
 * is already selected, remove them all (toggle off); otherwise add the missing
 * ones, preserving everything else. Returns a new ordered array.
 */
export function toggleEventBundle(values: string[], selected: string[]): string[] {
  const selectedSet = new Set(selected);
  if (isBundleActive(values, selectedSet)) {
    const removing = new Set(values);
    return selected.filter((v) => !removing.has(v));
  }
  const next = [...selected];
  for (const v of values) {
    if (!selectedSet.has(v)) {
      selectedSet.add(v);
      next.push(v);
    }
  }
  return next;
}

// ---------------------------------------------------------------------------
// Conditions editor: structured rows <-> NetBox-style ConditionSet
// ---------------------------------------------------------------------------

export const CONDITION_OPS = [
  "eq",
  "ne",
  "gt",
  "gte",
  "lt",
  "lte",
  "in",
  "contains",
  "regex",
] as const;

export type ConditionOp = (typeof CONDITION_OPS)[number];

export type ConditionLogic = "and" | "or";

/** One structured condition row in the editor. `value` is the raw text the user
 *  typed; it is parsed to JSON at serialization time. */
export interface ConditionRow {
  /** Local id for React keys; never serialized. */
  id: string;
  attr: string;
  op: ConditionOp;
  /** Raw text — `"42"`, `"true"`, `"[\"a\",\"b\"]"`, or a bare string. */
  value: string;
}

/** The serialized leaf shape: `{ attr, op, value }` with a JSON `value`. */
interface SerializedCondition {
  attr: string;
  op: ConditionOp;
  value: unknown;
}

/** The serialized ConditionSet: `{ "and" | "or": SerializedCondition[] }`. */
export type ConditionSet = Record<ConditionLogic, SerializedCondition[]>;

function isConditionOp(value: string): value is ConditionOp {
  return (CONDITION_OPS as readonly string[]).includes(value);
}

/**
 * Parse the raw value text into JSON when it looks like JSON (number, boolean,
 * null, array, or object), otherwise keep it as a plain string. This lets users
 * type `42`, `true`, or `["a","b"]` and get the right JSON type, while bare
 * words like `pending` stay strings without needing quotes.
 */
export function parseConditionValue(raw: string): unknown {
  const trimmed = raw.trim();
  if (trimmed === "") return "";
  if (trimmed === "true") return true;
  if (trimmed === "false") return false;
  if (trimmed === "null") return null;
  // Numbers (incl. negatives / decimals / exponents), but not things like "1.2.3".
  if (/^-?\d+(\.\d+)?([eE][+-]?\d+)?$/.test(trimmed)) {
    const num = Number(trimmed);
    if (Number.isFinite(num)) return num;
  }
  // Arrays / objects — only when it actually parses as JSON.
  if (
    (trimmed.startsWith("[") && trimmed.endsWith("]")) ||
    (trimmed.startsWith("{") && trimmed.endsWith("}")) ||
    (trimmed.startsWith('"') && trimmed.endsWith('"'))
  ) {
    try {
      return JSON.parse(trimmed) as unknown;
    } catch {
      return raw;
    }
  }
  return raw;
}

/** Render a JSON value back to the editable raw text shown in a row input. */
export function stringifyConditionValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === null || typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  return JSON.stringify(value);
}

let rowCounter = 0;
/** Fresh local id for a condition row (module-scoped, monotonic). */
export function newConditionRowId(): string {
  rowCounter += 1;
  return `cond-${rowCounter}`;
}

/** A blank row with a default `eq` operator. */
export function emptyConditionRow(): ConditionRow {
  return { id: newConditionRowId(), attr: "", op: "eq", value: "" };
}

/** A row is meaningful (serializable) once it has a non-empty `attr`. */
export function isRowComplete(row: ConditionRow): boolean {
  return row.attr.trim() !== "";
}

/**
 * Serialize the structured editor into a ConditionSet, or `undefined` when no
 * row is complete (no filter → omit `conditions` entirely). Incomplete rows
 * (blank attr) are dropped.
 */
export function serializeConditions(
  logic: ConditionLogic,
  rows: ConditionRow[],
): ConditionSet | undefined {
  const complete = rows.filter(isRowComplete);
  if (complete.length === 0) return undefined;
  const conditions: SerializedCondition[] = complete.map((r) => ({
    attr: r.attr.trim(),
    op: r.op,
    value: parseConditionValue(r.value),
  }));
  return { [logic]: conditions } as ConditionSet;
}

export interface DeserializedConditions {
  logic: ConditionLogic;
  rows: ConditionRow[];
}

/**
 * Best-effort parse of an arbitrary backend `conditions` value into structured
 * rows. Recognizes `{ and: [...] }` / `{ or: [...] }` with `{ attr, op, value }`
 * leaves. Anything unexpected yields an empty AND set so the UI degrades to the
 * raw-JSON escape hatch rather than throwing.
 */
export function deserializeConditions(raw: unknown): DeserializedConditions {
  const empty: DeserializedConditions = { logic: "and", rows: [] };
  if (raw === null || raw === undefined || typeof raw !== "object") return empty;

  const obj = raw as Record<string, unknown>;
  const logic: ConditionLogic = Array.isArray(obj.or) ? "or" : "and";
  const list = obj[logic];
  if (!Array.isArray(list)) return empty;

  const rows: ConditionRow[] = [];
  for (const entry of list) {
    if (entry === null || typeof entry !== "object") continue;
    const leaf = entry as Record<string, unknown>;
    const attr = typeof leaf.attr === "string" ? leaf.attr : "";
    const op = typeof leaf.op === "string" && isConditionOp(leaf.op) ? leaf.op : "eq";
    rows.push({
      id: newConditionRowId(),
      attr,
      op,
      value: stringifyConditionValue(leaf.value),
    });
  }
  return { logic, rows };
}

/** True when a parsed value is a structured ConditionSet the editor can host. */
export function isStructuredConditionSet(raw: unknown): boolean {
  if (raw === null || typeof raw !== "object") return false;
  const obj = raw as Record<string, unknown>;
  const list = obj.and ?? obj.or;
  if (!Array.isArray(list)) return false;
  return list.every(
    (entry) =>
      entry !== null &&
      typeof entry === "object" &&
      typeof (entry as Record<string, unknown>).attr === "string",
  );
}
