import { useSetting } from "@/shared/settings";

export type TreeGroupKey =
  | "region"
  | "site"
  | "tenant"
  | "role"
  | "platform"
  | "deviceType"
  | "cluster";

export type TreeQuickAccessProtocol = "ssh" | "rdp" | "vnc" | "app" | "playbook";

export interface TreeQuickAccessConfig {
  enabled: boolean;
  protocols: TreeQuickAccessProtocol[];
}

export interface TreeConfig {
  mode: "auto" | "custom";
  order: TreeGroupKey[];
  includeUnknown: boolean;
  quickAccess: TreeQuickAccessConfig;
}

export const ALL_GROUPS: TreeGroupKey[] = [
  "region",
  "site",
  "tenant",
  "role",
  "platform",
  "deviceType",
  "cluster",
];

export const DEFAULT_TREE_CONFIG: TreeConfig = {
  mode: "auto",
  order: ["site", "tenant", "role"],
  includeUnknown: true,
  quickAccess: {
    enabled: true,
    protocols: ["ssh", "playbook"],
  },
};

export const TREE_QUICK_ACCESS_PROTOCOLS: TreeQuickAccessProtocol[] = [
  "ssh",
  "rdp",
  "vnc",
  "app",
  "playbook",
];

/** Auto mode prefers dimensions that actually have values on loaded hosts. */
export const AUTO_PREFERRED_ORDER: TreeGroupKey[] = [
  "region",
  "site",
  "tenant",
  "role",
  "platform",
  "deviceType",
  "cluster",
];

/** Compute auto-mode order by filtering dimensions with no non-empty values. */
export function computeAutoOrder<T extends Record<string, unknown>>(
  hosts: T[],
  preferred: TreeGroupKey[] = AUTO_PREFERRED_ORDER,
): TreeGroupKey[] {
  return preferred.filter((k) =>
    hosts.some((h) => {
      const v = h[k];
      return typeof v === "string" && v.length > 0;
    }),
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function normalizeTreeQuickAccessConfig(raw: unknown): TreeQuickAccessConfig {
  if (!isRecord(raw)) return DEFAULT_TREE_CONFIG.quickAccess;

  const seen = new Set<TreeQuickAccessProtocol>();
  const protocols = Array.isArray(raw.protocols)
    ? raw.protocols.filter((value): value is TreeQuickAccessProtocol => {
      if (
        typeof value !== "string" ||
        !(TREE_QUICK_ACCESS_PROTOCOLS as string[]).includes(value) ||
        seen.has(value as TreeQuickAccessProtocol)
      ) {
        return false;
      }
      seen.add(value as TreeQuickAccessProtocol);
      return true;
    })
    : DEFAULT_TREE_CONFIG.quickAccess.protocols;

  return {
    enabled: typeof raw.enabled === "boolean"
      ? raw.enabled
      : DEFAULT_TREE_CONFIG.quickAccess.enabled,
    protocols: protocols.length ? protocols : DEFAULT_TREE_CONFIG.quickAccess.protocols,
  };
}

export function normalizeTreeConfig(raw: unknown): TreeConfig {
  if (raw === null || raw === undefined || typeof raw !== "object") {
    return DEFAULT_TREE_CONFIG;
  }
  const parsed = raw as Partial<TreeConfig>;
  const rawOrder = Array.isArray(parsed.order) ? parsed.order : [];
  const order = rawOrder.filter((k): k is TreeGroupKey =>
    (ALL_GROUPS as string[]).includes(k as string),
  );
  return {
    mode: parsed.mode === "custom" ? "custom" : "auto",
    order: order.length ? order : DEFAULT_TREE_CONFIG.order,
    includeUnknown: isRecord(raw) && typeof raw.includeUnknown === "boolean"
      ? raw.includeUnknown
      : DEFAULT_TREE_CONFIG.includeUnknown,
    quickAccess: normalizeTreeQuickAccessConfig(
      isRecord(raw) ? raw.quickAccess : undefined,
    ),
  };
}

export function useTreeConfig(): {
  config: TreeConfig;
  setConfig: (next: TreeConfig) => void;
  isLoading: boolean;
  isFetching: boolean;
} {
  const { value, setValue, isLoading } = useSetting<unknown>(
    "connections.tree.config",
    DEFAULT_TREE_CONFIG,
    { normalize: normalizeTreeConfig },
  );

  const config = normalizeTreeConfig(value);

  const setConfig = (next: TreeConfig) => {
    setValue(next);
  };

  return {
    config,
    setConfig,
    isLoading,
    isFetching: isLoading,
  };
}
