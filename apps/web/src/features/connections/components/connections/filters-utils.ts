import type { ConnectionTarget } from "@/shared/api/types";

export function matchesTargetSearch(target: ConnectionTarget, query: string): boolean {
  const q = query.trim().toLowerCase();
  return !q || [target.name, target.region, target.site, target.tenant, target.role, target.platform, target.deviceType, target.cluster, target.primaryIp]
    .some(value => value?.toLowerCase().includes(q));
}

export type FilterKey =
  | "hostName"
  | "status"
  | "targetKind"
  | "primaryIp"
  | "ipFamily"
  | "ipSegment"
  | "addressState"
  | "addressBinding"
  | "region"
  | "site"
  | "tenant"
  | "role"
  | "platform"
  | "deviceType"
  | "cluster";

export type FilterState = Record<FilterKey, Set<string>>;
export type SerializableFilterState = Partial<Record<FilterKey, string[]>>;

export type FilterPresetVisibility = "private" | "public";

export interface SavedFilterPreset {
  id: string;
  name: string;
  visibility: FilterPresetVisibility;
  filters: SerializableFilterState;
  createdAt: string;
  updatedAt: string;
  createdBy?: string;
}

export const FILTER_KEYS: FilterKey[] = [
  "hostName",
  "status",
  "targetKind",
  "primaryIp",
  "ipFamily",
  "ipSegment",
  "addressState",
  "addressBinding",
  "region",
  "site",
  "tenant",
  "role",
  "platform",
  "deviceType",
  "cluster",
];

export function emptyFilterState(): FilterState {
  return {
    hostName: new Set(),
    status: new Set(),
    targetKind: new Set(),
    primaryIp: new Set(),
    ipFamily: new Set(),
    ipSegment: new Set(),
    addressState: new Set(),
    addressBinding: new Set(),
    region: new Set(),
    site: new Set(),
    tenant: new Set(),
    role: new Set(),
    platform: new Set(),
    deviceType: new Set(),
    cluster: new Set(),
  };
}

export function getFilterValues(host: ConnectionTarget, key: FilterKey): string[] {
  switch (key) {
    case "hostName":
      return compactFilterValues(host.name);
    case "targetKind":
      return compactFilterValues(host.targetKind);
    case "primaryIp":
      return compactFilterValues(host.primaryIp);
    case "ipFamily":
      return compactFilterValues(ipFamily(host.primaryIp));
    case "ipSegment":
      return compactFilterValues(ipSegment(host.primaryIp));
    case "addressState":
      return [host.primaryIp ? "has_address" : "missing_address"];
    case "addressBinding":
      if (!host.primaryIp) return [];
      return [host.addressBound === false ? "unbound" : "bound"];
    default: {
      const value = host[key as keyof ConnectionTarget];
      return compactFilterValues(typeof value === "string" ? value : undefined);
    }
  }
}

function compactFilterValues(...values: Array<string | undefined>): string[] {
  return values
    .map((value) => value?.trim())
    .filter((value): value is string => Boolean(value));
}

function normalizeAddress(address?: string): string | undefined {
  const trimmed = address?.trim();
  if (!trimmed) return undefined;
  const withoutCidr = trimmed.split("/", 1)[0]?.trim();
  return withoutCidr || undefined;
}

function ipFamily(address?: string): "ipv4" | "ipv6" | undefined {
  const normalized = normalizeAddress(address);
  if (!normalized) return undefined;
  if (/^\d{1,3}(?:\.\d{1,3}){3}$/.test(normalized)) return "ipv4";
  if (normalized.includes(":")) return "ipv6";
  return undefined;
}

function ipSegment(address?: string): string | undefined {
  const normalized = normalizeAddress(address);
  if (!normalized) return undefined;
  if (/^\d{1,3}(?:\.\d{1,3}){3}$/.test(normalized)) {
    const octets = normalized.split(".");
    return `${octets[0]}.${octets[1]}.${octets[2]}.0/24`;
  }
  if (normalized.includes(":")) {
    const hextets = normalized.split(":").filter(Boolean).slice(0, 4);
    return hextets.length > 0 ? `${hextets.join(":")}::/64` : undefined;
  }
  return undefined;
}

export function countActiveFilters(selected: FilterState): number {
  return FILTER_KEYS.reduce((count, key) => count + selected[key].size, 0);
}

export function serializeFilterState(selected: FilterState): SerializableFilterState {
  const filters: SerializableFilterState = {};
  for (const key of FILTER_KEYS) {
    const values = [...selected[key]]
      .map((value) => value.trim())
      .filter((value) => value.length > 0)
      .sort((a, b) => a.localeCompare(b));
    const uniqueValues = [...new Set(values)];
    if (uniqueValues.length > 0) filters[key] = uniqueValues;
  }
  return filters;
}

export function deserializeFilterState(filters: SerializableFilterState): FilterState {
  const state = emptyFilterState();
  for (const key of FILTER_KEYS) {
    const values = filters[key];
    if (!Array.isArray(values)) continue;
    state[key] = new Set(values.filter((value): value is string => typeof value === "string"));
  }
  return state;
}

export function isSerializableFilterStateEmpty(filters: SerializableFilterState): boolean {
  return FILTER_KEYS.every((key) => (filters[key]?.length ?? 0) === 0);
}

/**
 * A draft is a deep-enough copy of the selection whose group Sets are never the
 * applied state's Sets. The filters dialog edits the draft and only publishes it
 * on Apply, so a Cancel leaves the live host list untouched.
 */
export function cloneFilterState(selected: FilterState): FilterState {
  const next = emptyFilterState();
  for (const key of FILTER_KEYS) next[key] = new Set(selected[key]);
  return next;
}

export function filterStateEquals(a: FilterState, b: FilterState): boolean {
  return FILTER_KEYS.every((key) => {
    const left = a[key];
    const right = b[key];
    if (left.size !== right.size) return false;
    for (const value of left) if (!right.has(value)) return false;
    return true;
  });
}

export function normalizeFilterPresets(
  raw: unknown,
  visibility: FilterPresetVisibility,
): SavedFilterPreset[] {
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((item): SavedFilterPreset[] => {
    if (typeof item !== "object" || item === null) return [];
    const candidate = item as Partial<SavedFilterPreset>;
    if (typeof candidate.id !== "string" || typeof candidate.name !== "string") return [];
    const filters =
      typeof candidate.filters === "object" && candidate.filters !== null
        ? sanitizeSerializableFilters(candidate.filters)
        : {};
    if (isSerializableFilterStateEmpty(filters)) return [];
    return [{
      id: candidate.id,
      name: candidate.name,
      visibility,
      filters,
      createdAt: typeof candidate.createdAt === "string" ? candidate.createdAt : new Date(0).toISOString(),
      updatedAt: typeof candidate.updatedAt === "string" ? candidate.updatedAt : new Date(0).toISOString(),
      createdBy: typeof candidate.createdBy === "string" ? candidate.createdBy : undefined,
    }];
  });
}

function sanitizeSerializableFilters(filters: SerializableFilterState): SerializableFilterState {
  const sanitized: SerializableFilterState = {};
  for (const key of FILTER_KEYS) {
    const values = filters[key];
    if (!Array.isArray(values)) continue;
    const uniqueValues = [
      ...new Set(
        values
          .filter((value): value is string => typeof value === "string")
          .map((value) => value.trim())
          .filter(Boolean),
      ),
    ].sort((a, b) => a.localeCompare(b));
    if (uniqueValues.length > 0) sanitized[key] = uniqueValues;
  }
  return sanitized;
}

/**
 * Applies the current filter selection to a host list. A host matches when
 * every non-empty filter group contains the host's value (AND across groups,
 * OR within a group). Empty groups are ignored.
 */
export function applyFilters(hosts: ConnectionTarget[], selected: FilterState): ConnectionTarget[] {
  const hasAny = FILTER_KEYS.some((k) => selected[k].size > 0);
  if (!hasAny) return hosts;
  return hosts.filter((h) => {
    for (const key of FILTER_KEYS) {
      const picks = selected[key];
      if (picks.size === 0) continue;
      const values = getFilterValues(h, key);
      if (!values.some((value) => picks.has(value))) return false;
    }
    return true;
  });
}
