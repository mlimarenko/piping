/**
 * Structured model for a dynamic target-set selector.
 *
 * The backend accepts an opaque JSON `selector` object; this module gives the UI
 * a typed, ergonomic shape over the subset of selector keys the first release
 * understands, plus lossless round-tripping to/from the raw JSON so power users
 * can still hand-edit fields the builder does not surface.
 */

export type SelectorSortDirection = "asc" | "desc";

/** Multi-value selector facets that filter resolved connection targets. */
export const SELECTOR_FACETS = [
  "region",
  "site",
  "tenant",
  "role",
  "platform",
  "cluster",
  "status",
] as const;

export type SelectorFacet = (typeof SELECTOR_FACETS)[number];

export interface SelectorModel {
  /** Multi-value facets: facet -> selected names (matched server-side by name). */
  facets: Record<SelectorFacet, string[]>;
  /** Require a primary address bound to the target (anti-SSRF safe). */
  addressBound: boolean;
  sort: string;
  sortDirection: SelectorSortDirection;
  /**
   * Selector keys the builder does not surface, preserved verbatim so editing a
   * structured selector never silently drops an advanced key.
   */
  passthrough: Record<string, unknown>;
}

const TARGET_KIND = ["connection_target"];
const SOURCE_KIND = ["device", "virtual_machine"];

const STRUCTURED_KEYS = new Set<string>([
  "target_kind",
  "source_kind",
  "address_bound",
  "sort",
  "sort_direction",
  ...SELECTOR_FACETS,
]);

function asStringArray(value: unknown): string[] {
  if (Array.isArray(value)) {
    return value.filter((v): v is string => typeof v === "string");
  }
  if (typeof value === "string" && value.trim()) return [value];
  return [];
}

export function emptySelectorModel(): SelectorModel {
  return {
    facets: {
      region: [],
      site: [],
      tenant: [],
      role: [],
      platform: [],
      cluster: [],
      status: ["active"],
    },
    addressBound: true,
    sort: "name",
    sortDirection: "asc",
    passthrough: {},
  };
}

/** Parse a raw selector object into the structured model, preserving unknown keys. */
export function selectorToModel(raw: unknown): SelectorModel {
  const model = emptySelectorModel();
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return model;
  const obj = raw as Record<string, unknown>;

  for (const facet of SELECTOR_FACETS) {
    model.facets[facet] = asStringArray(obj[facet]);
  }
  if (typeof obj.address_bound === "boolean") model.addressBound = obj.address_bound;
  if (typeof obj.sort === "string") model.sort = obj.sort;
  if (obj.sort_direction === "asc" || obj.sort_direction === "desc") {
    model.sortDirection = obj.sort_direction;
  }

  const passthrough: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(obj)) {
    if (!STRUCTURED_KEYS.has(key)) passthrough[key] = value;
  }
  model.passthrough = passthrough;
  return model;
}

/** Serialize the structured model back into the raw selector object. */
export function modelToSelector(model: SelectorModel): Record<string, unknown> {
  const selector: Record<string, unknown> = {
    ...model.passthrough,
    target_kind: TARGET_KIND,
    source_kind: SOURCE_KIND,
    address_bound: model.addressBound,
    sort: model.sort,
    sort_direction: model.sortDirection,
  };
  for (const facet of SELECTOR_FACETS) {
    const values = model.facets[facet];
    if (values.length > 0) selector[facet] = values;
  }
  return selector;
}
