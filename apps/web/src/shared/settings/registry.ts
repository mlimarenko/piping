import { DEFAULT_UI_SCALE, sanitizeUiScale } from "@/shared/ui-scale";

export type ThemeMode = "light" | "dark" | "system";
export type LanguageCode = "en" | "ru";

export interface ThemeSettingValue {
  mode: ThemeMode;
}

export interface LanguageSettingValue {
  lng: LanguageCode;
}

export type SettingCategory =
  | "appearance"
  | "workspace"
  | "connections"
  | "assistant"
  | "data";
export type SettingPersistence = "server" | "browser" | "session";
export type SettingCachePolicy = "none" | "bootstrap";
export type SettingControl = "select" | "text" | "boolean" | "number" | "json";
export type SettingVisibility = "all" | "admin";
export type ConnectionOpenInNewTabProtocol = "ssh" | "rdp" | "vnc";

export interface ConnectionsOpenInNewTabSettingValue {
  enabled: boolean;
  protocols: Record<ConnectionOpenInNewTabProtocol, boolean>;
}

export interface SettingDefinition<T = unknown> {
  key: string;
  titleKey: string;
  descriptionKey: string;
  category: SettingCategory;
  persistence: SettingPersistence;
  cachePolicy: SettingCachePolicy;
  defaultValue: T;
  normalize: (value: unknown) => T;
  control: SettingControl;
  visibility?: SettingVisibility;
  options?: readonly { value: unknown; labelKey: string }[];
}

export const TABLE_PAGE_SIZE_OPTIONS = [50, 100, 250, 500, 1000] as const;
const MIN_TABLE_PAGE_SIZE = 10;
const MAX_TABLE_PAGE_SIZE = Math.max(...TABLE_PAGE_SIZE_OPTIONS);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function normalizeThemeSetting(value: unknown): ThemeSettingValue {
  if (value === "light" || value === "dark" || value === "system") return { mode: value };
  if (isRecord(value)) {
    const mode = value.mode;
    if (mode === "light" || mode === "dark" || mode === "system") return { mode };
  }
  return { mode: "system" };
}

export function normalizeLanguageSetting(value: unknown): LanguageSettingValue {
  if (value === "en" || value === "ru") return { lng: value };
  if (isRecord(value)) {
    const lng = value.lng;
    if (lng === "en" || lng === "ru") return { lng };
  }
  return { lng: "en" };
}

export function normalizeTablePageSize(value: unknown): number {
  const numeric = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(numeric)) return 50;
  return Math.max(MIN_TABLE_PAGE_SIZE, Math.min(MAX_TABLE_PAGE_SIZE, Math.trunc(numeric)));
}

export interface TableColumnVisibilityEntry {
  hidden: string[];
}
export type TableColumnVisibilityValue = Record<string, TableColumnVisibilityEntry>;

/**
 * Per-table DEFAULT hidden columns, applied when a user has no stored override
 * for that `tableId`. This is policy DATA (like `connections.tree.config`'s
 * hardcoded order/protocols below): every column a table declares stays present
 * and toggleable in the column-settings picker, but the long-tail attributes
 * listed here start hidden so a wide entity does not render dozens of columns by
 * default. The user can reveal any of them; `reset()` re-applies this default.
 *
 * Inventory policy: the resource table renders a column for EVERY tabular field
 * an entity declares (full attribute set). To keep the default table readable,
 * each entity keeps the same visible window it had before (the first N data
 * fields: N=3 for Prefixes, 4 elsewhere) and hides the rest by default. The keys
 * below are the table column keys AFTER `columnKeyOverrides` for the
 * sort/filter-configured entities (`device_type` not `deviceType`, `rack_role`
 * not `rackRoleName`, …) and the raw field keys for entities with no override.
 * Only entities whose field list exceeds the window have an entry; the rest show
 * all their (few) columns. A drifted key (e.g. a missed override) fails the
 * `columnDefaults.test.ts` guard in `features/inventory` instead of silently
 * showing the column.
 */
export const TABLE_DEFAULT_HIDDEN_COLUMNS: Record<string, readonly string[]> = {
  "connection-config.records": ["description", "createdAt", "updatedAt"],
  "connection-config.assignments": ["createdAt"],
  "inventory.Sites": ["time_zone", "latitude", "longitude"],
  "inventory.Devices": ["primary_ip", "serial", "asset_tag", "position", "face", "airflow"],
  "inventory.Virtual Machines": ["primary_ip", "vcpus", "memory", "disk"],
  "inventory.Virtual Machine Types": ["default_memory"],
  "inventory.Contacts": ["link"],
  "inventory.Racks": ["facility_id", "serial", "asset_tag", "width", "u_height", "outer_width", "outer_depth"],
  "inventory.Cables": ["length"],
  "inventory.Rack Types": [
    "starting_unit",
    "outer_width",
    "outer_depth",
    "outer_height",
    "mounting_depth",
    "weight",
    "max_weight",
  ],
  "inventory.Modules": ["asset_tag"],
  "inventory.Circuits": ["termination_date", "commit_rate", "distance", "distance_unit"],
  "inventory.IKE Proposals": ["sa_lifetime"],
  "inventory.Circuit Terminations": ["port_speed", "upstream_speed"],
  "inventory.Power Feeds": ["voltage", "amperage", "max_utilization"],
};

/** Default hidden column keys for a table, or an empty array when the table has
 *  no default-hidden policy (every column visible until the user hides one). */
export function defaultHiddenColumnsFor(tableId: string): readonly string[] {
  return TABLE_DEFAULT_HIDDEN_COLUMNS[tableId] ?? [];
}

export function normalizeTableColumnVisibility(value: unknown): TableColumnVisibilityValue {
  if (!isRecord(value)) return {};
  const result: TableColumnVisibilityValue = {};
  for (const [tableId, entry] of Object.entries(value)) {
    const normalizedTableId = tableId.trim();
    if (!normalizedTableId) continue;
    if (!isRecord(entry)) continue;
    const hidden = Array.isArray(entry.hidden)
      ? Array.from(
          new Set(
            entry.hidden
              .filter((item): item is string => typeof item === "string")
              .map((item) => item.trim())
              .filter((item) => item.length > 0),
          ),
        )
      : [];
    result[normalizedTableId] = { hidden };
  }
  return result;
}

export function normalizeBooleanSetting(value: unknown): boolean {
  if (typeof value === "boolean") return value;
  if (typeof value === "string") {
    return ["true", "1", "yes", "on"].includes(value.trim().toLowerCase());
  }
  if (isRecord(value) && "enabled" in value) return normalizeBooleanSetting(value.enabled);
  return false;
}

export const CONNECTION_OPEN_IN_NEW_TAB_PROTOCOLS: readonly ConnectionOpenInNewTabProtocol[] = [
  "ssh",
  "rdp",
  "vnc",
];

export const DEFAULT_CONNECTIONS_OPEN_IN_NEW_TAB_SETTING: ConnectionsOpenInNewTabSettingValue = {
  enabled: false,
  protocols: {
    ssh: false,
    rdp: false,
    vnc: false,
  },
};

export function normalizeConnectionsOpenInNewTabSetting(
  value: unknown,
): ConnectionsOpenInNewTabSettingValue {
  if (typeof value === "boolean") {
    return {
      ...DEFAULT_CONNECTIONS_OPEN_IN_NEW_TAB_SETTING,
      enabled: value,
    };
  }
  if (!isRecord(value)) return DEFAULT_CONNECTIONS_OPEN_IN_NEW_TAB_SETTING;
  const rawProtocols = isRecord(value.protocols) ? value.protocols : {};
  return {
    enabled: normalizeBooleanSetting(value.enabled),
    protocols: {
      ssh: normalizeBooleanSetting(rawProtocols.ssh),
      rdp: normalizeBooleanSetting(rawProtocols.rdp),
      vnc: normalizeBooleanSetting(rawProtocols.vnc),
    },
  };
}

export function shouldOpenConnectionInNewTab(
  setting: ConnectionsOpenInNewTabSettingValue,
  protocol: ConnectionOpenInNewTabProtocol,
): boolean {
  return setting.enabled || setting.protocols[protocol];
}

export function normalizeStringSetting(value: unknown): string {
  if (typeof value === "string") return value.trim();
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (isRecord(value)) {
    const nested = value.value ?? value.url ?? value.model;
    return normalizeStringSetting(nested);
  }
  return "";
}

function boundedInteger(fallback: number, min: number, max: number): (value: unknown) => number {
  return (value) => {
    const raw = isRecord(value) && "value" in value ? value.value : value;
    const numeric = typeof raw === "number" ? raw : Number(raw);
    if (!Number.isFinite(numeric)) return fallback;
    return Math.max(min, Math.min(max, Math.trunc(numeric)));
  };
}

export type AssistantWindowDock = "right" | "left" | "bottom" | "floating";

export interface AssistantWindowSettingValue {
  open: boolean;
  dock: AssistantWindowDock;
  width: number;
  height: number;
}

export const DEFAULT_ASSISTANT_WINDOW_SETTING: AssistantWindowSettingValue = {
  open: false,
  dock: "right",
  width: 420,
  height: 560,
};

export function normalizeAssistantWindowSetting(value: unknown): AssistantWindowSettingValue {
  if (!isRecord(value)) return DEFAULT_ASSISTANT_WINDOW_SETTING;
  const dock =
    value.dock === "right" ||
    value.dock === "left" ||
    value.dock === "bottom" ||
    value.dock === "floating"
      ? value.dock
      : DEFAULT_ASSISTANT_WINDOW_SETTING.dock;
  const width = boundedInteger(DEFAULT_ASSISTANT_WINDOW_SETTING.width, 320, 900)(value.width);
  const height = boundedInteger(DEFAULT_ASSISTANT_WINDOW_SETTING.height, 360, 860)(value.height);
  return {
    open: normalizeBooleanSetting(value.open),
    dock,
    width,
    height,
  };
}

function identityJson<T>(fallback: T): (value: unknown) => T {
  return (value) => (value === undefined || value === null ? fallback : (value as T));
}

/** Display format for the rendered config context shown in inspectors/forms. */
export type ConfigContextFormat = "json" | "yaml" | "toml";

export const CONFIG_CONTEXT_FORMATS: readonly ConfigContextFormat[] = ["json", "yaml", "toml"];
export const DEFAULT_CONFIG_CONTEXT_FORMAT: ConfigContextFormat = "json";
export const CONFIG_CONTEXT_FORMAT_SETTING_KEY = "inventory.config_context.format";

export function normalizeConfigContextFormat(value: unknown): ConfigContextFormat {
  if (value === "json" || value === "yaml" || value === "toml") return value;
  if (isRecord(value) && typeof value.value === "string") {
    return normalizeConfigContextFormat(value.value);
  }
  return DEFAULT_CONFIG_CONTEXT_FORMAT;
}

export const SERVER_SETTING_DEFINITIONS: readonly SettingDefinition[] = [
  {
    key: "app.theme",
    titleKey: "settingsRegistry.appTheme.title",
    descriptionKey: "settingsRegistry.appTheme.description",
    category: "appearance",
    persistence: "server",
    cachePolicy: "bootstrap",
    defaultValue: { mode: "system" } satisfies ThemeSettingValue,
    normalize: normalizeThemeSetting,
    control: "select",
    options: [
      { value: "system", labelKey: "settingsRegistry.theme.system" },
      { value: "dark", labelKey: "settingsRegistry.theme.dark" },
      { value: "light", labelKey: "settingsRegistry.theme.light" },
    ],
  },
  {
    key: "app.language",
    titleKey: "settingsRegistry.appLanguage.title",
    descriptionKey: "settingsRegistry.appLanguage.description",
    category: "appearance",
    persistence: "server",
    cachePolicy: "bootstrap",
    defaultValue: { lng: "en" } satisfies LanguageSettingValue,
    normalize: normalizeLanguageSetting,
    control: "select",
    options: [
      { value: "ru", labelKey: "language.russian" },
      { value: "en", labelKey: "language.english" },
    ],
  },
  {
    key: "ui.scale",
    titleKey: "settingsRegistry.uiScale.title",
    descriptionKey: "settingsRegistry.uiScale.description",
    category: "appearance",
    persistence: "server",
    cachePolicy: "bootstrap",
    defaultValue: DEFAULT_UI_SCALE,
    normalize: sanitizeUiScale,
    control: "select",
    options: [0.9, 1, 1.1, 1.25, 1.4, 1.5].map((value) => ({
      value,
      labelKey: `settingsRegistry.uiScaleOptions.${Math.round(value * 100)}`,
    })),
  },
  {
    key: "table.page_size",
    titleKey: "settingsRegistry.tablePageSize.title",
    descriptionKey: "settingsRegistry.tablePageSize.description",
    category: "data",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: 50,
    normalize: normalizeTablePageSize,
    control: "select",
    options: TABLE_PAGE_SIZE_OPTIONS.map((value) => ({
      value,
      labelKey: `settingsRegistry.tablePageSizeOptions.${value}`,
    })),
  },
  {
    key: "table.column_visibility",
    titleKey: "settingsRegistry.tableColumnVisibility.title",
    descriptionKey: "settingsRegistry.tableColumnVisibility.description",
    category: "data",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: {} satisfies TableColumnVisibilityValue,
    normalize: normalizeTableColumnVisibility,
    control: "json",
  },
  {
    key: CONFIG_CONTEXT_FORMAT_SETTING_KEY,
    titleKey: "settingsRegistry.configContextFormat.title",
    descriptionKey: "settingsRegistry.configContextFormat.description",
    category: "appearance",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: DEFAULT_CONFIG_CONTEXT_FORMAT,
    normalize: normalizeConfigContextFormat,
    control: "select",
    options: [
      { value: "json", labelKey: "settingsRegistry.configContextFormat.json" },
      { value: "yaml", labelKey: "settingsRegistry.configContextFormat.yaml" },
      { value: "toml", labelKey: "settingsRegistry.configContextFormat.toml" },
    ],
  },
  {
    key: "connections.tree.config",
    titleKey: "settingsRegistry.treeConfig.title",
    descriptionKey: "settingsRegistry.treeConfig.description",
    category: "connections",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: {
      mode: "auto",
      order: ["site", "tenant", "role"],
      includeUnknown: true,
      quickAccess: {
        enabled: true,
        protocols: ["ssh", "playbook"],
      },
    },
    normalize: identityJson({
      mode: "auto",
      order: ["site", "tenant", "role"],
      includeUnknown: true,
      quickAccess: {
        enabled: true,
        protocols: ["ssh", "playbook"],
      },
    }),
    control: "json",
  },
  {
    key: "connections.tree.collapsed",
    titleKey: "settingsRegistry.treeCollapsed.title",
    descriptionKey: "settingsRegistry.treeCollapsed.description",
    category: "connections",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: { version: 1, byScope: {} },
    normalize: identityJson({ version: 1, byScope: {} }),
    control: "json",
  },
  {
    key: "connections.open_in_new_tab",
    titleKey: "settingsRegistry.connectionsOpenInNewTab.title",
    descriptionKey: "settingsRegistry.connectionsOpenInNewTab.description",
    category: "connections",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: DEFAULT_CONNECTIONS_OPEN_IN_NEW_TAB_SETTING,
    normalize: normalizeConnectionsOpenInNewTabSetting,
    control: "json",
  },
  {
    key: "connections.workspace.filters",
    titleKey: "settingsRegistry.workspaceFilters.title",
    descriptionKey: "settingsRegistry.workspaceFilters.description",
    category: "workspace",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: {},
    normalize: identityJson({}),
    control: "json",
  },
  {
    key: "connections.workspace.panel",
    titleKey: "settingsRegistry.workspacePanel.title",
    descriptionKey: "settingsRegistry.workspacePanel.description",
    category: "workspace",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: {},
    normalize: identityJson({}),
    control: "json",
  },
  {
    key: "connections.workspace.playbook_tabs",
    titleKey: "settingsRegistry.workspacePlaybookTabs.title",
    descriptionKey: "settingsRegistry.workspacePlaybookTabs.description",
    category: "workspace",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: { version: 1, tabs: [] },
    normalize: identityJson({ version: 1, tabs: [] }),
    control: "json",
  },
  {
    key: "connections.workspace.playbooks_dock",
    titleKey: "settingsRegistry.workspacePlaybooksDock.title",
    descriptionKey: "settingsRegistry.workspacePlaybooksDock.description",
    category: "workspace",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: { expanded: false },
    normalize: identityJson({ expanded: false }),
    control: "json",
  },
  {
    key: "automation.runs.detail_width",
    titleKey: "settingsRegistry.automationRunsDetailWidth.title",
    descriptionKey: "settingsRegistry.automationRunsDetailWidth.description",
    category: "workspace",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: { width: 0 },
    normalize: identityJson({ width: 0 }),
    control: "json",
  },
  {
    key: "assistant.instance.enabled",
    titleKey: "settingsRegistry.assistantEnabled.title",
    descriptionKey: "settingsRegistry.assistantEnabled.description",
    category: "assistant",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: false,
    normalize: normalizeBooleanSetting,
    control: "boolean",
    visibility: "admin",
  },
  {
    key: "assistant.instance.provider",
    titleKey: "settingsRegistry.assistantProvider.title",
    descriptionKey: "settingsRegistry.assistantProvider.description",
    category: "assistant",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: "openrouter",
    normalize: normalizeStringSetting,
    control: "select",
    visibility: "admin",
    options: [
      { value: "openrouter", labelKey: "settingsRegistry.assistantProviders.openrouter" },
      { value: "litellm", labelKey: "settingsRegistry.assistantProviders.litellm" },
    ],
  },
  {
    key: "assistant.instance.base_url",
    titleKey: "settingsRegistry.assistantBaseUrl.title",
    descriptionKey: "settingsRegistry.assistantBaseUrl.description",
    category: "assistant",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: "",
    normalize: normalizeStringSetting,
    control: "text",
    visibility: "admin",
  },
  {
    key: "assistant.instance.model",
    titleKey: "settingsRegistry.assistantModel.title",
    descriptionKey: "settingsRegistry.assistantModel.description",
    category: "assistant",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: "",
    normalize: normalizeStringSetting,
    control: "text",
    visibility: "admin",
  },
  {
    key: "assistant.instance.portal_context_enabled",
    titleKey: "settingsRegistry.assistantPortalContext.title",
    descriptionKey: "settingsRegistry.assistantPortalContext.description",
    category: "assistant",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: false,
    normalize: normalizeBooleanSetting,
    control: "boolean",
    visibility: "admin",
  },
  {
    key: "assistant.instance.max_sessions_per_user",
    titleKey: "settingsRegistry.assistantMaxSessions.title",
    descriptionKey: "settingsRegistry.assistantMaxSessions.description",
    category: "assistant",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: 5,
    normalize: boundedInteger(5, 1, 25),
    control: "number",
    visibility: "admin",
  },
  {
    key: "assistant.instance.archive_retention_days",
    titleKey: "settingsRegistry.assistantArchiveRetention.title",
    descriptionKey: "settingsRegistry.assistantArchiveRetention.description",
    category: "assistant",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: 30,
    normalize: boundedInteger(30, 1, 365),
    control: "number",
    visibility: "admin",
  },
  {
    key: "assistant.window",
    titleKey: "settingsRegistry.assistantWindow.title",
    descriptionKey: "settingsRegistry.assistantWindow.description",
    category: "assistant",
    persistence: "server",
    cachePolicy: "none",
    defaultValue: DEFAULT_ASSISTANT_WINDOW_SETTING,
    normalize: normalizeAssistantWindowSetting,
    control: "json",
  },
];

export const LOCAL_SETTING_DEFINITIONS: readonly SettingDefinition[] = [
  {
    key: "connections.sshFileBrowser",
    titleKey: "settingsRegistry.sshFileBrowser.title",
    descriptionKey: "settingsRegistry.sshFileBrowser.description",
    category: "connections",
    persistence: "browser",
    cachePolicy: "none",
    defaultValue: {},
    normalize: identityJson({}),
    control: "json",
  },
  {
    key: "connections.workspace.sessionTabs",
    titleKey: "settingsRegistry.sessionTabs.title",
    descriptionKey: "settingsRegistry.sessionTabs.description",
    category: "workspace",
    persistence: "session",
    cachePolicy: "none",
    defaultValue: [],
    normalize: identityJson([]),
    control: "json",
  },
];

export const SETTING_DEFINITIONS = [
  ...SERVER_SETTING_DEFINITIONS,
  ...LOCAL_SETTING_DEFINITIONS,
] as const;

export function getSettingDefinition(key: string): SettingDefinition | undefined {
  return SETTING_DEFINITIONS.find((definition) => definition.key === key);
}

export function getServerSettingDefinition(key: string): SettingDefinition | undefined {
  return SERVER_SETTING_DEFINITIONS.find((definition) => definition.key === key);
}
