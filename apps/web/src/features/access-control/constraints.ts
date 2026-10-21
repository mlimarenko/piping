/**
 * Constraint-expression (de)serialization for the policy editor.
 *
 * A policy's `constraints` is free-form JSON evaluated by the backend. The
 * grammar the editor authors is the attribute-equality subset:
 *
 *   { "attributes": { "<key>": "<value>" } }   // ALL keys in the block must match
 *   { "and": [ <node>, ... ] }                 // every child must match
 *   { "or":  [ <node>, ... ] }                 // any child matches
 *
 * plus opaque nodes the editor does not model (`ip_range`, `time_window`, deeper
 * nesting) which are preserved verbatim so round-tripping a policy through the
 * editor never silently drops them.
 *
 * The editor presents a flat list of `{ key, value }` equality rows. This module
 * maps that list to/from the grammar:
 *   - rows sharing a key collapse into an `or` (that key may be any of the values)
 *   - distinct keys are `and`-ed together (all must match)
 * so "role_id in {A,B} AND protocol=vnc" is expressible from a flat row list
 * without a bespoke boolean-tree UI.
 */

/** One attribute-equality condition authored in the builder. */
export interface AttributeConditionRow {
  /** Stable UI id (React key / removal); not serialized. */
  id: string;
  /** Attribute key (from the resource's `constraint_attribute_keys`). */
  key: string;
  /** Exact-match value (UUID for id keys, plain string otherwise). */
  value: string;
}

/** Result of parsing an existing policy's constraints for editing. */
export interface ParsedConstraints {
  /** Editable attribute-equality rows. */
  rows: AttributeConditionRow[];
  /** Non-attribute nodes preserved verbatim and re-emitted on save. */
  extra: unknown[];
}

type Json = Record<string, unknown>;

const isPlainObject = (v: unknown): v is Json =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** `{ attributes: { k: v, ... } }` with every value a string, at least one key. */
const asAttributesRecord = (node: unknown): Record<string, string> | null => {
  if (!isPlainObject(node)) return null;
  const keys = Object.keys(node);
  if (keys.length !== 1 || keys[0] !== "attributes") return null;
  const attrs = node.attributes;
  if (!isPlainObject(attrs)) return null;
  const entries = Object.entries(attrs);
  if (entries.length === 0) return null;
  if (!entries.every(([, val]) => typeof val === "string")) return null;
  return Object.fromEntries(entries) as Record<string, string>;
};

/** `{ or: [...] }` where every child is a single-key attributes node on the SAME
 *  key — the shape {@link buildConstraints} emits for a multi-value row group. */
const asSameKeyOr = (node: unknown): { key: string; values: string[] } | null => {
  if (!isPlainObject(node)) return null;
  const keys = Object.keys(node);
  if (keys.length !== 1 || keys[0] !== "or") return null;
  const arr = node.or;
  if (!Array.isArray(arr) || arr.length === 0) return null;
  let sharedKey: string | null = null;
  const values: string[] = [];
  for (const child of arr) {
    const record = asAttributesRecord(child);
    if (!record) return null;
    const recEntries = Object.entries(record);
    if (recEntries.length !== 1) return null;
    const [k, v] = recEntries[0]!;
    if (sharedKey === null) sharedKey = k;
    else if (sharedKey !== k) return null;
    values.push(v);
  }
  return sharedKey === null ? null : { key: sharedKey, values };
};

let rowSeq = 0;
const nextRowId = (): string => `cr-${(rowSeq++).toString(36)}-${crypto.randomUUID().slice(0, 4)}`;

/**
 * Parse a policy's `constraints` into editable attribute rows plus any opaque
 * nodes to preserve. Unrecognised shapes are kept in `extra` verbatim rather
 * than dropped, so editing a policy authored elsewhere is non-destructive.
 */
export function parseConstraints(constraints: unknown): ParsedConstraints {
  const rows: AttributeConditionRow[] = [];
  const extra: unknown[] = [];
  if (constraints === undefined || constraints === null) return { rows, extra };

  // Top level is either an `and` of clauses or a single clause.
  const clauses: unknown[] =
    isPlainObject(constraints) && Array.isArray(constraints.and) && Object.keys(constraints).length === 1
      ? constraints.and
      : [constraints];

  for (const clause of clauses) {
    const attrs = asAttributesRecord(clause);
    if (attrs) {
      for (const [key, value] of Object.entries(attrs)) {
        rows.push({ id: nextRowId(), key, value });
      }
      continue;
    }
    const orGroup = asSameKeyOr(clause);
    if (orGroup) {
      for (const value of orGroup.values) {
        rows.push({ id: nextRowId(), key: orGroup.key, value });
      }
      continue;
    }
    extra.push(clause);
  }
  return { rows, extra };
}

/**
 * Compose attribute rows (plus any preserved `extra` nodes) into the constraint
 * grammar. Rows with an empty key or value are ignored. Same-key rows become an
 * `or`; distinct keys are `and`-ed. Returns `undefined` when nothing is set so
 * an unconstrained policy carries no `constraints` field.
 */
export function buildConstraints(
  rows: readonly AttributeConditionRow[],
  extra: readonly unknown[] = [],
): unknown {
  // Group non-empty rows by key, preserving first-seen order and de-duplicating
  // identical values within a key.
  const order: string[] = [];
  const byKey = new Map<string, string[]>();
  for (const { key, value } of rows) {
    if (!key || value === "") continue;
    if (!byKey.has(key)) {
      byKey.set(key, []);
      order.push(key);
    }
    const values = byKey.get(key)!;
    if (!values.includes(value)) values.push(value);
  }

  const nodes: unknown[] = order.map((key) => {
    const values = byKey.get(key)!;
    return values.length === 1
      ? { attributes: { [key]: values[0]! } }
      : { or: values.map((v) => ({ attributes: { [key]: v } })) };
  });
  nodes.push(...extra);

  if (nodes.length === 0) return undefined;
  if (nodes.length === 1) return nodes[0];
  return { and: nodes };
}
