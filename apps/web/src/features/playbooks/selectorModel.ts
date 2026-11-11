/**
 * Structured model for a dynamic target-set selector.
 *
 * Facet controls store stable inventory identifiers. Other selector operators
 * remain explicit additional conditions and survive edits to these controls.
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
  "device_type",
  "tag",
  "status",
] as const;

export type SelectorFacet = (typeof SELECTOR_FACETS)[number];

export const SELECTOR_FACET_KEYS: Record<SelectorFacet, string> = {
  region: "region_ids", site: "site_ids", tenant: "tenant_ids", role: "role_ids",
  platform: "platform_ids", cluster: "cluster_ids", device_type: "device_type_ids",
  tag: "tag_ids", status: "status",
};

export interface SelectorModel {
  /** Inventory UUIDs; status uses the inventory status value. */
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
  /** Original selector and its parsed controls, for lossless edits. */
  original?: {
    raw: Record<string, unknown>;
    facets: Record<SelectorFacet, string[]>;
    addressBound: boolean;
    sort: string;
    sortDirection: SelectorSortDirection;
  };
}

const TARGET_KIND = ["connection_target"];
const SOURCE_KIND = ["device", "virtual_machine"];

const STRUCTURED_KEYS = new Set<string>([
  "target_kind",
  "source_kind",
  "address_bound",
  "sort",
  "sort_direction",
  ...Object.values(SELECTOR_FACET_KEYS),
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
      device_type: [],
      tag: [],
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
    model.facets[facet] = asStringArray(obj[SELECTOR_FACET_KEYS[facet]]);
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
  model.original = {
    raw: obj,
    facets: Object.fromEntries(SELECTOR_FACETS.map(facet => [facet, [...model.facets[facet]]])) as Record<SelectorFacet, string[]>,
    addressBound: model.addressBound, sort: model.sort, sortDirection: model.sortDirection,
  };
  return model;
}

/** Serialize the structured model back into the raw selector object. */
export function modelToSelector(model: SelectorModel): Record<string, unknown> {
  if (model.original) {
    const { original } = model;
    const selector: Record<string, unknown> = { ...original.raw, ...model.passthrough };
    for (const key of Object.keys(original.raw)) {
      if (!STRUCTURED_KEYS.has(key) && !(key in model.passthrough)) delete selector[key];
    }
    for (const facet of SELECTOR_FACETS) {
      if (JSON.stringify(model.facets[facet]) === JSON.stringify(original.facets[facet])) continue;
      if (model.facets[facet].length) selector[SELECTOR_FACET_KEYS[facet]] = model.facets[facet];
      else delete selector[SELECTOR_FACET_KEYS[facet]];
    }
    if (model.addressBound !== original.addressBound) selector.address_bound = model.addressBound;
    if (model.sort !== original.sort) selector.sort = model.sort;
    if (model.sortDirection !== original.sortDirection) selector.sort_direction = model.sortDirection;
    return selector;
  }
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
    if (values.length > 0) selector[SELECTOR_FACET_KEYS[facet]] = values;
  }
  return selector;
}
