import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { GROUPS_TABLE_CONFIG, GRANTS_TABLE_CONFIG, POLICIES_TABLE_CONFIG, ROLES_TABLE_CONFIG, USERS_TABLE_CONFIG } from "@/features/access-control/tableConfig";
import { AUDIT_TABLE_CONFIG } from "@/features/audit/tableConfig";
import { ACTIVE_CONNECTIONS_TABLE_CONFIG } from "@/features/connections/activeConnectionsTableConfig";
import { CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG, CONNECTION_CONFIG_RECORDS_TABLE_CONFIG } from "@/features/connections/connection-config/tableConfig";
import { PROXY_ASSIGNMENTS_TABLE_CONFIG, PROXY_NODES_TABLE_CONFIG } from "@/features/connections/proxyTableConfig";
import { CONNECTION_TARGETS_TABLE_CONFIG } from "@/features/connections/tableConfig";
import { CONFIG_CONTEXT_BINDINGS_TABLE_CONFIG } from "@/features/inventory/configContextBindingsTableConfig";
import { CONFIG_CONTEXTS_TABLE_CONFIG } from "@/features/inventory/configContextsTableConfig";
import { tabConfig } from "@/features/inventory/components/inventory/inventoryTypes";
import { CUSTOM_FIELD_CHOICE_SETS_TABLE_CONFIG, CUSTOM_FIELD_DEFINITIONS_TABLE_CONFIG } from "@/features/inventory/customFieldsTableConfig";
import { SYNC_MAPPINGS_TABLE_CONFIG } from "@/features/inventory/syncMappingsTableConfig";
import { SYNC_PROVIDERS_TABLE_CONFIG } from "@/features/inventory/syncProvidersTableConfig";
import { inventoryTableConfigFor } from "@/features/inventory/tableConfig";
import { INVENTORY_TEMPLATE_RUNS_TABLE_CONFIG, INVENTORY_TEMPLATES_TABLE_CONFIG } from "@/features/inventory/templatesTableConfig";
import { EXPORTER_ASSIGNMENTS_TABLE_CONFIG } from "@/features/observability/tableConfig";
import { PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG, PLAYBOOK_RUN_GROUPS_TABLE_CONFIG, PLAYBOOK_SCHEDULES_TABLE_CONFIG, PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG, PLAYBOOK_TARGET_SETS_TABLE_CONFIG } from "@/features/playbooks/tableConfig";
import { USER_CREDENTIALS_TABLE_CONFIG, USER_TOKENS_TABLE_CONFIG } from "@/features/user-settings/tableConfig";
import { WEBHOOKS_TABLE_CONFIG } from "@/features/webhooks/tableConfig";
import { defaultHiddenColumnsFor } from "@/shared/settings/registry";
import {
  BACKEND_ONLY_INVENTORY_REGISTRIES,
  BACKEND_ONLY_NON_RESOURCE_REGISTRIES,
  CLIENT_ONLY_STANDARDIZED_TABLE_IDS,
  INVENTORY_RESOURCE_TABLE_IDS,
  INVENTORY_RESOURCE_API_IMPORT,
  INVENTORY_RESOURCE_TABLE_OWNER_FILE,
  INVENTORY_CHILD_TABLE_CONTRACTS,
  INVENTORY_CHILD_TABLE_IDS,
  INVENTORY_CHILD_TABLE_OWNER_FILE,
  type NonResourceTableId,
  type NonResourceTableContract,
  NON_RESOURCE_TABLE_IDS,
  NON_RESOURCE_TABLE_CONTRACTS,
  STANDARDIZED_TABLE_IDS,
} from "./tableContracts";
import { TABLE_CONTRACT_SCENARIO_IDS } from "./tableContractScenarios";

const WEB_SRC_ROOT = path.resolve(import.meta.dirname, "..");
const APP_ROOT = path.resolve(WEB_SRC_ROOT, "..");
const API_TESTS_ROOT = path.resolve(APP_ROOT, "..", "api", "tests", "compliance");

function collectSourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "__tests__") return [];
      return collectSourceFiles(fullPath);
    }
    if (!/\.(ts|tsx)$/.test(entry.name)) return [];
    if (entry.name.endsWith(".test.ts") || entry.name.endsWith(".test.tsx")) return [];
    return [fullPath];
  });
}

function collectRustSourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) return collectRustSourceFiles(fullPath);
    if (!entry.name.endsWith(".rs")) return [];
    return [fullPath];
  });
}

function collectDataTableRenderFiles(): string[] {
  return collectSourceFiles(WEB_SRC_ROOT)
    .filter((filePath) => /<DataTable(?=[\s>])/.test(loadSource(filePath)))
    .sort();
}

function loadTableIdUsage() {
  const literalIds = new Set<string>();
  const filePathsByLiteralId = new Map<string, Set<string>>();
  const dynamicExpressions = new Set<string>();
  const filePathsByDynamicExpression = new Map<string, Set<string>>();
  const unexpectedDynamicExpressions = new Set<string>();

  for (const filePath of collectSourceFiles(WEB_SRC_ROOT)) {
    const source = fs.readFileSync(filePath, "utf8");
    for (const match of source.matchAll(/tableId="([^"]+)"/g)) {
      const tableId = match[1];
      if (tableId) {
        literalIds.add(tableId);
        const filePaths = filePathsByLiteralId.get(tableId) ?? new Set<string>();
        filePaths.add(filePath);
        filePathsByLiteralId.set(tableId, filePaths);
      }
    }
    if (source.includes("tableId={connectionsTableId}")) {
      dynamicExpressions.add("connectionsTableId");
      const filePaths = filePathsByDynamicExpression.get("connectionsTableId") ?? new Set<string>();
      filePaths.add(filePath);
      filePathsByDynamicExpression.set("connectionsTableId", filePaths);
    }
    if (source.includes("tableId={childTableId}")) {
      dynamicExpressions.add("childTableId");
      const filePaths = filePathsByDynamicExpression.get("childTableId") ?? new Set<string>();
      filePaths.add(filePath);
      filePathsByDynamicExpression.set("childTableId", filePaths);
    }
    if (source.includes("tableId={`inventory.${activeTab}`}")) {
      dynamicExpressions.add("`inventory.${activeTab}`");
      const filePaths =
        filePathsByDynamicExpression.get("`inventory.${activeTab}`") ?? new Set<string>();
      filePaths.add(filePath);
      filePathsByDynamicExpression.set("`inventory.${activeTab}`", filePaths);
    }
    for (const match of source.matchAll(/tableId=\{[^\n]+\}/g)) {
      const expression = match[0]
        .replace(/^tableId=\{/, "")
        .replace(/\}$/, "")
        .trim();
      if (
        expression === "connectionsTableId" ||
        expression === "childTableId" ||
        expression === "`inventory.${activeTab}`" ||
        expression === "tableId"
      ) {
        continue;
      }
      unexpectedDynamicExpressions.add(expression);
    }
  }

  return {
    literalIds: [...literalIds].sort(),
    filePathsByLiteralId: new Map(
      [...filePathsByLiteralId.entries()].map(([tableId, filePaths]) => [
        tableId,
        [...filePaths].sort(),
      ]),
    ),
    dynamicExpressions: [...dynamicExpressions].sort(),
    filePathsByDynamicExpression: new Map(
      [...filePathsByDynamicExpression.entries()].map(([expression, filePaths]) => [
        expression,
        [...filePaths].sort(),
      ]),
    ),
    unexpectedDynamicExpressions: [...unexpectedDynamicExpressions].sort(),
  };
}

function loadSource(filePath: string): string {
  return fs.readFileSync(filePath, "utf8");
}

function relativeSourcePath(filePath: string): string {
  return path.relative(WEB_SRC_ROOT, filePath).replaceAll(path.sep, "/");
}

function resolveApiModulePath(ownerFile: string, apiImport: string): string {
  if (apiImport.startsWith("@/")) {
    return path.resolve(WEB_SRC_ROOT, `${apiImport.replace("@/", "")}.ts`);
  }
  return path.resolve(path.dirname(path.resolve(WEB_SRC_ROOT, ownerFile)), `${apiImport}.ts`);
}

function loadBackendRegistryNames(fileName: string): string[] {
  const source = loadSource(path.resolve(API_TESTS_ROOT, fileName));
  return Array.from(
    source.matchAll(/Registry\s*\{[\s\S]*?name:\s*"([^"]+)"[\s\S]*?(?:const_name|route_path):/g),
    (match) => match[1],
  )
    .filter((name): name is string => typeof name === "string")
    .sort();
}

function loadBackendRegistryConstNames(fileName: string): Map<string, string> {
  const source = loadSource(path.resolve(API_TESTS_ROOT, fileName));
  return new Map(
    Array.from(
      source.matchAll(
        /Registry\s*\{[\s\S]*?name:\s*"([^"]+)"[\s\S]*?const_name:\s*"([^"]+)"/g,
      ),
      (match): [string, string] => [match[1] ?? "", match[2] ?? ""],
    ).filter(([name, constName]) => Boolean(name) && Boolean(constName)),
  );
}

function loadBackendRegistryColumnExpressions(fileName: string): Map<string, string> {
  const source = loadSource(path.resolve(API_TESTS_ROOT, fileName));
  const expressions = new Map<string, string>();

  for (const match of source.matchAll(/Registry\s*\{([\s\S]*?)\}/g)) {
    const body = match[1] ?? "";
    const name = body.match(/name:\s*"([^"]+)"/)?.[1];
    const columns = body.match(/columns:\s*([^,\n]+)/)?.[1]?.trim();
    if (name && columns) {
      expressions.set(name, columns);
    }
  }

  return expressions;
}

function loadInventoryBackendRegistryByTableId(): Map<string, string> {
  const source = loadSource(
    path.resolve(WEB_SRC_ROOT, "features", "inventory", "api.ts"),
  );
  const body =
    source.match(/const ENTITY_LIST_PATH:[\s\S]*?= \{([\s\S]*?)\n\};/)?.[1] ?? "";
  const registries = new Map(
    Array.from(
      body.matchAll(/(?:"([^"]+)"|([A-Za-z]+)):\s*"([^"]+)"/g),
      (match): [string, string] => {
        const entity = (match[1] ?? match[2] ?? "").trim();
        const registry = (match[3] ?? "").split("/").pop()?.replaceAll("-", "_") ?? "";
        return [`inventory.${entity}`, registry];
      },
    ),
  );
  for (const contract of INVENTORY_CHILD_TABLE_CONTRACTS) {
    const registry = registries.get(`inventory.${contract.childEntity}`);
    if (registry) registries.set(contract.tableId, registry);
  }
  return registries;
}

function readBalancedSource(
  source: string,
  openIndex: number,
  openChar: string,
  closeChar: string,
): string {
  let depth = 0;
  let inString = false;
  let escaped = false;

  for (let index = openIndex; index < source.length; index += 1) {
    const char = source[index];

    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === "\"") {
        inString = false;
      }
      continue;
    }

    if (char === "\"") {
      inString = true;
      continue;
    }

    if (char === openChar) {
      depth += 1;
    } else if (char === closeChar) {
      depth -= 1;
      if (depth === 0) {
        return source.slice(openIndex + 1, index);
      }
    }
  }

  throw new Error(`Unbalanced ${openChar}${closeChar} expression`);
}

function readListColumnArrayBody(source: string, constName: string): string | null {
  const constMatch = new RegExp(`pub(?:\\(crate\\))? const ${constName}:`).exec(source);
  const constIndex = constMatch?.index ?? -1;
  if (constIndex === -1) return null;

  const initializerIndex = source.indexOf("= &[", constIndex);
  if (initializerIndex === -1) return null;

  const arrayIndex = initializerIndex + 2;

  return readBalancedSource(source, arrayIndex + 1, "[", "]");
}

function splitTopLevelArguments(callBody: string): string[] {
  const args: string[] = [];
  let start = 0;
  let depth = 0;
  let inString = false;
  let escaped = false;

  for (let index = 0; index < callBody.length; index += 1) {
    const char = callBody[index];

    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === "\"") {
        inString = false;
      }
      continue;
    }

    if (char === "\"") {
      inString = true;
      continue;
    }

    if (char === "(" || char === "[" || char === "{") {
      depth += 1;
      continue;
    }
    if (char === ")" || char === "]" || char === "}") {
      depth -= 1;
      continue;
    }
    if (char === "," && depth === 0) {
      args.push(callBody.slice(start, index).trim());
      start = index + 1;
    }
  }

  args.push(callBody.slice(start).trim());
  return args;
}

function readStringLiteralArgument(argument: string): string | null {
  const match = argument.match(/^"((?:\\.|[^"\\])*)"$/);
  return match ? (JSON.parse(match[0]) as string) : null;
}

type ListColumnFilterMethod = "enum_filter" | "enum_filter_with_param" | "fk_filter";

function listColumnFilterParamArgumentIndex(methodName: ListColumnFilterMethod): number {
  return methodName === "enum_filter" ? 0 : 2;
}

function readListColumnFilterParams(
  source: string,
  methodName: ListColumnFilterMethod,
): string[] {
  const callPrefix = `ListColumn::${methodName}(`;
  const params: string[] = [];
  let searchIndex = 0;

  while (searchIndex < source.length) {
    const callIndex = source.indexOf(callPrefix, searchIndex);
    if (callIndex === -1) break;

    const openIndex = callIndex + callPrefix.length - 1;
    const callBody = readBalancedSource(source, openIndex, "(", ")");
    const args = splitTopLevelArguments(callBody);
    const paramArgIndex = listColumnFilterParamArgumentIndex(methodName);
    const param = readStringLiteralArgument(args[paramArgIndex] ?? "");
    if (param) params.push(param);
    searchIndex = openIndex + callBody.length;
  }

  return params;
}

function readListColumnSortKeys(source: string): string[] {
  return Array.from(
    source.matchAll(
      /ListColumn::(?:sortable|enum_filter|enum_filter_with_param|fk_filter)\(\s*"((?:\\.|[^"\\])*)"/g,
    ),
    (match) => JSON.parse(`"${match[1] ?? ""}"`) as string,
  );
}

function collectListColumnConstBodies(): Array<{
  filePath: string;
  constName: string;
  body: string;
}> {
  const apiSourceRoot = path.resolve(
    APP_ROOT,
    "..",
    "api",
    "src",
  );
  const bodies: Array<{ filePath: string; constName: string; body: string }> = [];

  for (const filePath of collectRustSourceFiles(apiSourceRoot)) {
    const source = loadSource(filePath);
    for (const match of source.matchAll(/pub(?:\(crate\))? const ([A-Z0-9_]+):\s*&\[ListColumn\]/g)) {
      const constName = match[1];
      if (!constName) continue;

      const body = readListColumnArrayBody(source, constName);
      if (!body) continue;

      bodies.push({ filePath, constName, body });
    }
  }

  return bodies;
}

function listColumnConstLookupKeys(
  filePath: string,
  constName: string,
  isBareNameUnique: boolean,
): string[] {
  const normalizedPath = filePath.replaceAll(path.sep, "/");
  const moduleName = path.basename(filePath, ".rs");
  const keys = new Set<string>([`${moduleName}::${constName}`]);

  if (isBareNameUnique) {
    keys.add(constName);
  }
  if (normalizedPath.includes("/contexts/inventory/infrastructure/persistence/repositories/")) {
    keys.add(`repos::${moduleName}::${constName}`);
    const marker = "/contexts/inventory/infrastructure/persistence/repositories/";
    const rest = normalizedPath.split(marker)[1] ?? "";
    const segments = rest.split("/");
    if (segments.length >= 2) {
      const group = segments[segments.length - 2];
      if (group) {
        keys.add(`repos::${group}::${moduleName}::${constName}`);
      }
    }
  }
  if (normalizedPath.includes("/api/rest/routes/monitoring/")) {
    keys.add(`repository::${constName}`);
  }

  return [...keys];
}

function loadBackendColumnFilterParamsByConstName(): Map<string, Set<string>> {
  const constBodies = collectListColumnConstBodies();
  const constNameCounts = constBodies.reduce<Record<string, number>>((counts, { constName }) => {
    counts[constName] = (counts[constName] ?? 0) + 1;
    return counts;
  }, {});
  const paramsByConstName = new Map<string, Set<string>>();

  for (const { filePath, constName, body } of constBodies) {
    const params = new Set<string>();
    for (const param of readListColumnFilterParams(body, "enum_filter")) params.add(param);
    for (const param of readListColumnFilterParams(body, "enum_filter_with_param")) params.add(param);
    for (const param of readListColumnFilterParams(body, "fk_filter")) params.add(param);

    for (const key of listColumnConstLookupKeys(
      filePath,
      constName,
      constNameCounts[constName] === 1,
    )) {
      paramsByConstName.set(key, params);
    }
  }

  return paramsByConstName;
}

function loadBackendColumnSortKeysByConstName(): Map<string, Set<string>> {
  const constBodies = collectListColumnConstBodies();
  const constNameCounts = constBodies.reduce<Record<string, number>>((counts, { constName }) => {
    counts[constName] = (counts[constName] ?? 0) + 1;
    return counts;
  }, {});
  const sortKeysByConstName = new Map<string, Set<string>>();

  for (const { filePath, constName, body } of constBodies) {
    const sortKeys = new Set(readListColumnSortKeys(body));
    for (const key of listColumnConstLookupKeys(
      filePath,
      constName,
      constNameCounts[constName] === 1,
    )) {
      sortKeysByConstName.set(key, sortKeys);
    }
  }

  return sortKeysByConstName;
}

type FilterParamCarrier = {
  colKey: string;
  param?: string;
  buildParams?: (
    values: string[],
    context: { subjectUserIds?: Set<string> },
  ) => Record<string, string[]>;
};

function filterParamsFromColumns(
  filterColumns: readonly FilterParamCarrier[],
): string[] {
  const params = new Set<string>();
  for (const column of filterColumns) {
    if (column.param) params.add(column.param);
    if (column.buildParams) {
      for (const param of Object.keys(
        column.buildParams(["user-id", "group-id"], {
          subjectUserIds: new Set(["user-id"]),
        }),
      )) {
        params.add(param);
      }
    }
  }
  return [...params].sort();
}

function filterColumnKeysFromColumns(
  filterColumns: readonly FilterParamCarrier[],
): string[] {
  return [...new Set(filterColumns.map((column) => column.colKey))].sort();
}

function connectionTargetFilterParamsFor(
  view: "desktop" | "mobile",
): string[] {
  return CONNECTION_TARGETS_TABLE_CONFIG.viewColumns[view].flatMap((key) => {
    const param = CONNECTION_TARGETS_TABLE_CONFIG.columns[key]?.filter?.param;
    return param ? [param] : [];
  });
}

function connectionTargetFilterColumnKeysFor(
  view: "desktop" | "mobile",
): string[] {
  return CONNECTION_TARGETS_TABLE_CONFIG.viewColumns[view].filter(
    (key) => CONNECTION_TARGETS_TABLE_CONFIG.columns[key]?.filter?.param,
  );
}

type SortKeyCarrier = {
  sortKeys: readonly string[];
  sortKeyByColumn?: Record<string, string>;
};

function sortParamsFromDataTableConfig(config: SortKeyCarrier): string[] {
  return config.sortKeys
    .map((sortKey) => config.sortKeyByColumn?.[sortKey] ?? sortKey)
    .sort();
}

function connectionTargetSortParamsFor(
  view: "desktop" | "mobile",
): string[] {
  return CONNECTION_TARGETS_TABLE_CONFIG.viewColumns[view].flatMap((key) => {
    const column = CONNECTION_TARGETS_TABLE_CONFIG.columns[key];
    if (!column?.sortable) return [];
    return column.sortParam ? [column.sortParam] : [];
  });
}

function connectionTargetSortColumnKeysFor(
  view: "desktop" | "mobile",
): string[] {
  return CONNECTION_TARGETS_TABLE_CONFIG.viewColumns[view]
    .filter((key) => CONNECTION_TARGETS_TABLE_CONFIG.columns[key]?.sortable)
    .sort();
}

type NonInventoryFeTableContract = {
  sortColumnKeys: readonly string[];
  sortParams: readonly string[];
  filterColumnKeys: readonly string[];
  filterParams: readonly string[];
};

function nonInventoryFeTableContractFromConfig(config: SortKeyCarrier & {
  filterColumns: readonly FilterParamCarrier[];
}): NonInventoryFeTableContract {
  return {
    sortColumnKeys: [...config.sortKeys].sort(),
    sortParams: sortParamsFromDataTableConfig(config),
    filterColumnKeys: filterColumnKeysFromColumns(config.filterColumns),
    filterParams: filterParamsFromColumns(config.filterColumns),
  };
}

function connectionTargetFeTableContract(
  view: "desktop" | "mobile",
): NonInventoryFeTableContract {
  return {
    sortColumnKeys: connectionTargetSortColumnKeysFor(view),
    sortParams: connectionTargetSortParamsFor(view),
    filterColumnKeys: connectionTargetFilterColumnKeysFor(view),
    filterParams: connectionTargetFilterParamsFor(view),
  };
}

function backendFilterParamsForConst(
  backendFilterParams: Map<string, Set<string>>,
  constName: string,
): Set<string> | undefined {
  return backendFilterParams.get(constName) ?? backendFilterParams.get(constName.split("::").at(-1) ?? constName);
}

function backendCapabilitiesForRegistry(
  capabilitiesByConstName: Map<string, Set<string>>,
  registryConstNames: Map<string, string>,
  registryColumnExpressions: Map<string, string>,
  registryName: string,
): Set<string> | undefined {
  const columnsExpression = registryColumnExpressions.get(registryName);
  if (columnsExpression) {
    const capabilities = capabilitiesByConstName.get(columnsExpression);
    if (capabilities) return capabilities;
  }

  const constName = registryConstNames.get(registryName);
  return constName
    ? backendFilterParamsForConst(capabilitiesByConstName, constName)
    : undefined;
}

const EMPTY_FE_TABLE_CONTRACT: NonInventoryFeTableContract = {
  sortColumnKeys: [],
  sortParams: [],
  filterColumnKeys: [],
  filterParams: [],
};

const NON_INVENTORY_FE_TABLE_CONTRACTS: Record<
  NonResourceTableId,
  NonInventoryFeTableContract
> = {
  "access.users": nonInventoryFeTableContractFromConfig(USERS_TABLE_CONFIG),
  "access.groups": nonInventoryFeTableContractFromConfig(GROUPS_TABLE_CONFIG),
  "access.roles": nonInventoryFeTableContractFromConfig(ROLES_TABLE_CONFIG),
  "access.grants": nonInventoryFeTableContractFromConfig(GRANTS_TABLE_CONFIG),
  "access.policies": nonInventoryFeTableContractFromConfig(POLICIES_TABLE_CONFIG),
  "audit.log": nonInventoryFeTableContractFromConfig(AUDIT_TABLE_CONFIG),
  // Client-side sort over the full unpaginated secret list; sortable keys live
  // in features/automation-secrets/tableConfig.ts, no backend list registry.
  "automation-secrets.list": EMPTY_FE_TABLE_CONTRACT,
  "backup.preview": EMPTY_FE_TABLE_CONTRACT,
  "connection-config.assignments": nonInventoryFeTableContractFromConfig(
    CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG,
  ),
  "connection-config.records": nonInventoryFeTableContractFromConfig(
    CONNECTION_CONFIG_RECORDS_TABLE_CONFIG,
  ),
  "connection-config.effective-preview": EMPTY_FE_TABLE_CONTRACT,
  "connections.active": nonInventoryFeTableContractFromConfig(
    ACTIVE_CONNECTIONS_TABLE_CONFIG,
  ),
  "connections.proxies": nonInventoryFeTableContractFromConfig(PROXY_NODES_TABLE_CONFIG),
  "connections.proxyAssignments": nonInventoryFeTableContractFromConfig(
    PROXY_ASSIGNMENTS_TABLE_CONFIG,
  ),
  "connections.targets.desktop": connectionTargetFeTableContract("desktop"),
  "connections.targets.mobile": connectionTargetFeTableContract("mobile"),
  "inventory.config-context-bindings": nonInventoryFeTableContractFromConfig(
    CONFIG_CONTEXT_BINDINGS_TABLE_CONFIG,
  ),
  "inventory.config-contexts": nonInventoryFeTableContractFromConfig(
    CONFIG_CONTEXTS_TABLE_CONFIG,
  ),
  "inventory.customFields.choiceSets": nonInventoryFeTableContractFromConfig(
    CUSTOM_FIELD_CHOICE_SETS_TABLE_CONFIG,
  ),
  "inventory.customFields.definitions": nonInventoryFeTableContractFromConfig(
    CUSTOM_FIELD_DEFINITIONS_TABLE_CONFIG,
  ),
  "inventory.ipamWorkspace.prefixes": EMPTY_FE_TABLE_CONTRACT,
  "inventory.syncProviders": nonInventoryFeTableContractFromConfig(
    SYNC_PROVIDERS_TABLE_CONFIG,
  ),
  "inventory.syncMappings": nonInventoryFeTableContractFromConfig(
    SYNC_MAPPINGS_TABLE_CONFIG,
  ),
  "inventory.templateRuns": nonInventoryFeTableContractFromConfig(
    INVENTORY_TEMPLATE_RUNS_TABLE_CONFIG,
  ),
  "inventory.templates": nonInventoryFeTableContractFromConfig(
    INVENTORY_TEMPLATES_TABLE_CONFIG,
  ),
  "observability.exporters": nonInventoryFeTableContractFromConfig(
    EXPORTER_ASSIGNMENTS_TABLE_CONFIG,
  ),
  "observability.inspector.alert_assignments": EMPTY_FE_TABLE_CONTRACT,
  "observability.inspector.alert_sources": EMPTY_FE_TABLE_CONTRACT,
  "observability.inspector.metric_cards": EMPTY_FE_TABLE_CONTRACT,
  "observability.inspector.metric_groups": EMPTY_FE_TABLE_CONTRACT,
  "observability.inspector.profiles": EMPTY_FE_TABLE_CONTRACT,
  "observability.inspector.sources": EMPTY_FE_TABLE_CONTRACT,
  "playbooks.assignments": nonInventoryFeTableContractFromConfig(
    PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG,
  ),
  "playbooks.executionEnvironments": EMPTY_FE_TABLE_CONTRACT,
  "playbooks.jobTemplates": EMPTY_FE_TABLE_CONTRACT,
  "playbooks.projects": EMPTY_FE_TABLE_CONTRACT,
  "playbooks.runs": nonInventoryFeTableContractFromConfig(
    PLAYBOOK_RUN_GROUPS_TABLE_CONFIG,
  ),
  "playbooks.schedules": nonInventoryFeTableContractFromConfig(
    PLAYBOOK_SCHEDULES_TABLE_CONFIG,
  ),
  "playbooks.servicePrincipals": nonInventoryFeTableContractFromConfig(
    PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG,
  ),
  "playbooks.targetSets": nonInventoryFeTableContractFromConfig(
    PLAYBOOK_TARGET_SETS_TABLE_CONFIG,
  ),
  "userSettings.credentials": nonInventoryFeTableContractFromConfig(
    USER_CREDENTIALS_TABLE_CONFIG,
  ),
  "userSettings.tokens": nonInventoryFeTableContractFromConfig(
    USER_TOKENS_TABLE_CONFIG,
  ),
  "webhooks.list": nonInventoryFeTableContractFromConfig(WEBHOOKS_TABLE_CONFIG),
};

const NON_INVENTORY_FE_FILTER_PARAMS: Record<
  NonResourceTableId,
  readonly string[]
> = Object.fromEntries(
  Object.entries(NON_INVENTORY_FE_TABLE_CONTRACTS).map(([tableId, contract]) => [
    tableId,
    contract.filterParams,
  ]),
) as Record<NonResourceTableId, readonly string[]>;

const NON_INVENTORY_FE_FILTER_COLUMN_KEYS: Record<
  NonResourceTableId,
  readonly string[]
> = Object.fromEntries(
  Object.entries(NON_INVENTORY_FE_TABLE_CONTRACTS).map(([tableId, contract]) => [
    tableId,
    contract.filterColumnKeys,
  ]),
) as Record<NonResourceTableId, readonly string[]>;

const NON_INVENTORY_FE_SORT_PARAMS: Record<
  NonResourceTableId,
  readonly string[]
> = Object.fromEntries(
  Object.entries(NON_INVENTORY_FE_TABLE_CONTRACTS).map(([tableId, contract]) => [
    tableId,
    contract.sortParams,
  ]),
) as Record<NonResourceTableId, readonly string[]>;

describe("table contract registry", () => {
  it("keeps registry ids unique", () => {
    expect(new Set(STANDARDIZED_TABLE_IDS).size).toBe(STANDARDIZED_TABLE_IDS.length);
    expect(new Set(INVENTORY_CHILD_TABLE_IDS).size).toBe(INVENTORY_CHILD_TABLE_IDS.length);
  });

  it("covers every literal tableId rendered by the app", () => {
    const { literalIds } = loadTableIdUsage();
    const registeredIds = new Set(STANDARDIZED_TABLE_IDS);

    expect(literalIds.filter((tableId) => !registeredIds.has(tableId))).toEqual([]);
  });

  it("requires every rendered DataTable screen to declare a tableId", () => {
    const missingTableIds = collectDataTableRenderFiles().filter(
      (filePath) => !loadSource(filePath).includes("tableId="),
    );

    expect(missingTableIds).toEqual([]);
  });

  it("assigns every literal tableId to a single source owner", () => {
    const { filePathsByLiteralId } = loadTableIdUsage();

    for (const [tableId, filePaths] of filePathsByLiteralId.entries()) {
      expect(
        filePaths,
        `${tableId} is rendered by multiple source files: ${filePaths.join(", ")}`,
      ).toHaveLength(1);
    }
  });

  it("only uses the documented dynamic tableId expressions", () => {
    const { dynamicExpressions, unexpectedDynamicExpressions } = loadTableIdUsage();

    expect(dynamicExpressions).toEqual(["`inventory.${activeTab}`", "childTableId", "connectionsTableId"]);
    expect(unexpectedDynamicExpressions).toEqual([]);
  });

  it("keeps generated inventory table ids aligned with the resource table contract", () => {
    expect(INVENTORY_RESOURCE_TABLE_IDS).toContain("inventory.IP Addresses");
    expect(INVENTORY_RESOURCE_TABLE_IDS).toContain("inventory.Devices");
    expect(INVENTORY_RESOURCE_TABLE_IDS.length).toBeGreaterThan(40);
  });

  it("contains both literal and generated table ids in the global registry", () => {
    for (const tableId of NON_RESOURCE_TABLE_IDS) {
      expect(STANDARDIZED_TABLE_IDS).toContain(tableId);
    }
    for (const tableId of INVENTORY_RESOURCE_TABLE_IDS) {
      expect(STANDARDIZED_TABLE_IDS).toContain(tableId);
    }
    for (const tableId of INVENTORY_CHILD_TABLE_IDS) {
      expect(STANDARDIZED_TABLE_IDS).toContain(tableId);
    }
  });

  it("keeps non-inventory FE table capability contracts complete and internally consistent", () => {
    expect(Object.keys(NON_INVENTORY_FE_TABLE_CONTRACTS).sort()).toEqual([
      ...NON_RESOURCE_TABLE_IDS,
    ].sort());

    for (const [tableId, contract] of Object.entries(
      NON_INVENTORY_FE_TABLE_CONTRACTS,
    ) as Array<[NonResourceTableId, NonInventoryFeTableContract]>) {
      const sortColumnKeys = [...contract.sortColumnKeys];
      const sortParams = [...contract.sortParams];
      const filterColumnKeys = [...contract.filterColumnKeys];
      const filterParams = [...contract.filterParams];

      expect(new Set(sortColumnKeys).size, `${tableId} sort column keys`).toBe(
        sortColumnKeys.length,
      );
      expect(new Set(sortParams).size, `${tableId} backend sort params`).toBe(
        sortParams.length,
      );
      expect(new Set(filterColumnKeys).size, `${tableId} filter column keys`).toBe(
        filterColumnKeys.length,
      );
      expect(new Set(filterParams).size, `${tableId} backend filter params`).toBe(
        filterParams.length,
      );

      for (const columnKey of filterColumnKeys) {
        expect(
          sortColumnKeys,
          `${tableId}.${columnKey} is filterable but not sortable in the same FE contract`,
        ).toContain(columnKey);
      }

      if (NON_RESOURCE_TABLE_CONTRACTS[tableId].mode === "client-only") {
        expect(contract, `${tableId} is client-only and must not claim server params`).toEqual(
          EMPTY_FE_TABLE_CONTRACT,
        );
      }
    }
  });

  it("keeps the browser parity smoke aligned with every standardized non-inventory table id", () => {
    const scenarioIds = [...TABLE_CONTRACT_SCENARIO_IDS];

    expect(scenarioIds).toContain("inventory.IP Addresses");
    expect(scenarioIds).toContain("inventory.config-contexts");
    expect(scenarioIds).toContain("inventory.config-context-bindings");
    expect(scenarioIds).toContain("connections.targets.desktop");
    expect(scenarioIds).toContain("connections.targets.mobile");

    expect(
      NON_RESOURCE_TABLE_IDS.filter((tableId) => !scenarioIds.includes(tableId)),
    ).toEqual([]);
  });

  it("declares the exact set of client-only standardized tables", () => {
    const clientOnly = [...CLIENT_ONLY_STANDARDIZED_TABLE_IDS].sort();

    expect(clientOnly).toEqual([
      "automation-secrets.list",
      "backup.preview",
      "connection-config.effective-preview",
      "inventory.ipamWorkspace.prefixes",
      "observability.inspector.alert_assignments",
      "observability.inspector.alert_sources",
      "observability.inspector.metric_cards",
      "observability.inspector.metric_groups",
      "observability.inspector.profiles",
      "observability.inspector.sources",
      "playbooks.executionEnvironments",
      "playbooks.jobTemplates",
      "playbooks.projects",
    ]);
  });

  it("maps every server-backed standardized table id to a backend compliance registry", () => {
    const inventoryRegistries = new Set(
      loadBackendRegistryNames("inventory_table_columns.rs"),
    );
    const nonInventoryRegistries = new Set(
      loadBackendRegistryNames("non_inventory_table_columns.rs"),
    );
    const inventoryTableMap = loadInventoryBackendRegistryByTableId();
    const clientOnlyTableIds = new Set<string>(CLIENT_ONLY_STANDARDIZED_TABLE_IDS);

    for (const tableId of STANDARDIZED_TABLE_IDS) {
      if (clientOnlyTableIds.has(tableId)) {
        continue;
      }

      const registryName =
        inventoryTableMap.get(tableId) ??
        NON_RESOURCE_TABLE_CONTRACTS[
          tableId as NonResourceTableId
        ]?.backendRegistry;

      expect(registryName, `${tableId} is missing a backend registry mapping`).toBeTruthy();
      if (!registryName) continue;
      expect(
        inventoryRegistries.has(registryName) || nonInventoryRegistries.has(registryName),
        `${tableId} points at missing backend compliance registry ${registryName}`,
      ).toBe(true);
      const childContract = INVENTORY_CHILD_TABLE_CONTRACTS.find(contract => contract.tableId === tableId);
      if (childContract) {
        expect(childContract.listRoute.split("/").pop()?.replaceAll("-", "_"), `${tableId} must use its child entity's inventory route`).toBe(registryName);
      }
    }
  });

  it("keeps inventory filter params accepted by backend ListColumn registries", () => {
    const inventoryTableMap = loadInventoryBackendRegistryByTableId();
    const registryConstNames = loadBackendRegistryConstNames("inventory_table_columns.rs");
    const registryColumnExpressions = loadBackendRegistryColumnExpressions("inventory_table_columns.rs");
    const backendFilterParams = loadBackendColumnFilterParamsByConstName();
    const missingFilterParams: string[] = [];

    for (const tab of tabConfig) {
      const tableId = `inventory.${tab.key}`;
      const registryName = inventoryTableMap.get(tableId);
      const constName = registryName ? registryConstNames.get(registryName) : undefined;
      const acceptedParams = registryName
        ? backendCapabilitiesForRegistry(
            backendFilterParams,
            registryConstNames,
            registryColumnExpressions,
            registryName,
          )
        : undefined;

      for (const filterColumn of inventoryTableConfigFor(tab.key).filterColumns) {
        if (!registryName || !constName || !acceptedParams?.has(filterColumn.param)) {
          missingFilterParams.push(
            `${tableId}.${filterColumn.colKey} sends ${filterColumn.param}; backend registry=${registryName ?? "missing"} const=${constName ?? "missing"}`,
          );
        }
      }
    }

    expect(missingFilterParams).toEqual([]);
  });

  it("keeps inventory sort keys accepted by backend ListColumn registries", () => {
    const inventoryTableMap = loadInventoryBackendRegistryByTableId();
    const registryConstNames = loadBackendRegistryConstNames("inventory_table_columns.rs");
    const registryColumnExpressions = loadBackendRegistryColumnExpressions("inventory_table_columns.rs");
    const backendSortKeys = loadBackendColumnSortKeysByConstName();
    const missingSortKeys: string[] = [];

    for (const tab of tabConfig) {
      const tableId = `inventory.${tab.key}`;
      const registryName = inventoryTableMap.get(tableId);
      const constName = registryName ? registryConstNames.get(registryName) : undefined;
      const acceptedKeys = registryName
        ? backendCapabilitiesForRegistry(
            backendSortKeys,
            registryConstNames,
            registryColumnExpressions,
            registryName,
          )
        : undefined;

      for (const sortKey of inventoryTableConfigFor(tab.key).sortKeys) {
        if (!registryName || !constName || !acceptedKeys?.has(sortKey)) {
          missingSortKeys.push(
            `${tableId} sends sort=${sortKey}; backend registry=${registryName ?? "missing"} const=${constName ?? "missing"}`,
          );
        }
      }
    }

    expect(missingSortKeys).toEqual([]);
  });

  it("keeps non-inventory filter params accepted by backend ListColumn registries", () => {
    const registryConstNames = new Map([
      ...loadBackendRegistryConstNames("inventory_table_columns.rs"),
      ...loadBackendRegistryConstNames("non_inventory_table_columns.rs"),
    ]);
    const registryColumnExpressions = new Map([
      ...loadBackendRegistryColumnExpressions("inventory_table_columns.rs"),
      ...loadBackendRegistryColumnExpressions("non_inventory_table_columns.rs"),
    ]);
    const backendFilterParams = loadBackendColumnFilterParamsByConstName();
    const missingFilterParams: string[] = [];

    for (const [tableId, contract] of Object.entries(
      NON_RESOURCE_TABLE_CONTRACTS,
    ) as Array<[NonResourceTableId, NonResourceTableContract]>) {
      if (contract.mode === "client-only") continue;

      const feParams = NON_INVENTORY_FE_FILTER_PARAMS[tableId];
      expect(feParams, `${tableId} is missing FE filter param coverage`).toBeDefined();
      if (!feParams) continue;

      const registryName = contract.backendRegistry;
      const constName = registryName ? registryConstNames.get(registryName) : undefined;
      const acceptedParams = registryName
        ? backendCapabilitiesForRegistry(
            backendFilterParams,
            registryConstNames,
            registryColumnExpressions,
            registryName,
          )
        : undefined;

      for (const param of feParams) {
        if (!registryName || !constName || !acceptedParams?.has(param)) {
          missingFilterParams.push(
            `${tableId} sends ${param}; backend registry=${registryName ?? "missing"} const=${constName ?? "missing"}`,
          );
        }
      }
    }

    expect(missingFilterParams).toEqual([]);
  });

  it("does not hide filterable non-inventory columns by default", () => {
    const failures: string[] = [];

    for (const [tableId, filterColumnKeys] of Object.entries(NON_INVENTORY_FE_FILTER_COLUMN_KEYS) as Array<
      [NonResourceTableId, readonly string[]]
    >) {
      const hidden = new Set(defaultHiddenColumnsFor(tableId));
      for (const columnKey of filterColumnKeys) {
        if (hidden.has(columnKey)) failures.push(`${tableId}.${columnKey}`);
      }
    }

    expect(failures).toEqual([]);
  });

  it("keeps non-inventory sort keys accepted by backend ListColumn registries", () => {
    const registryConstNames = new Map([
      ...loadBackendRegistryConstNames("inventory_table_columns.rs"),
      ...loadBackendRegistryConstNames("non_inventory_table_columns.rs"),
    ]);
    const registryColumnExpressions = new Map([
      ...loadBackendRegistryColumnExpressions("inventory_table_columns.rs"),
      ...loadBackendRegistryColumnExpressions("non_inventory_table_columns.rs"),
    ]);
    const backendSortKeys = loadBackendColumnSortKeysByConstName();
    const missingSortKeys: string[] = [];

    for (const [tableId, contract] of Object.entries(
      NON_RESOURCE_TABLE_CONTRACTS,
    ) as Array<[NonResourceTableId, NonResourceTableContract]>) {
      if (contract.mode === "client-only") continue;

      const feSortKeys = NON_INVENTORY_FE_SORT_PARAMS[tableId];
      expect(feSortKeys, `${tableId} is missing FE sort-key coverage`).toBeDefined();
      if (!feSortKeys) continue;

      const registryName = contract.backendRegistry;
      const constName = registryName ? registryConstNames.get(registryName) : undefined;
      const acceptedKeys = registryName
        ? backendCapabilitiesForRegistry(
            backendSortKeys,
            registryConstNames,
            registryColumnExpressions,
            registryName,
          )
        : undefined;

      for (const sortKey of feSortKeys) {
        if (!registryName || !constName || !acceptedKeys?.has(sortKey)) {
          missingSortKeys.push(
            `${tableId} sends sort=${sortKey}; backend registry=${registryName ?? "missing"} const=${constName ?? "missing"}`,
          );
        }
      }
    }

    expect(missingSortKeys).toEqual([]);
  });

  it("documents which backend inventory registries are still not surfaced through standardized FE tables", () => {
    const inventoryRegistries = loadBackendRegistryNames("inventory_table_columns.rs");
    const coveredRegistryNames = new Set<string>([
      ...loadInventoryBackendRegistryByTableId().values(),
      ...Object.values(NON_RESOURCE_TABLE_CONTRACTS)
        .map((contract) => contract.backendRegistry)
        .filter((registryName): registryName is string =>
          typeof registryName === "string" && inventoryRegistries.includes(registryName),
        ),
    ]);
    const backendOnlyRegistries = BACKEND_ONLY_INVENTORY_REGISTRIES.map(
      ({ registry }) => registry,
    ).sort();

    expect(
      inventoryRegistries.filter((registryName) => !coveredRegistryNames.has(registryName)),
    ).toEqual(backendOnlyRegistries);
    expect(new Set(backendOnlyRegistries).size).toBe(backendOnlyRegistries.length);
    for (const { registry, reason } of BACKEND_ONLY_INVENTORY_REGISTRIES) {
      expect(registry, "backend-only inventory registry needs a stable id").toBeTruthy();
      expect(reason, `${registry} needs an explicit backend-only rationale`).toBeTruthy();
    }
  });

  it("documents which backend non-inventory registries are not user-facing table surfaces", () => {
    const nonInventoryRegistries = loadBackendRegistryNames(
      "non_inventory_table_columns.rs",
    );
    const coveredRegistryNames = new Set(
      Object.values(NON_RESOURCE_TABLE_CONTRACTS)
        .map((contract) => contract.backendRegistry)
        .filter((registryName): registryName is string => typeof registryName === "string"),
    );
    const backendOnlyRegistries = BACKEND_ONLY_NON_RESOURCE_REGISTRIES.map(
      ({ registry }) => registry,
    ).sort();

    expect(
      nonInventoryRegistries.filter(
        (registryName) => !coveredRegistryNames.has(registryName),
      ),
    ).toEqual(backendOnlyRegistries);
    expect(new Set(backendOnlyRegistries).size).toBe(backendOnlyRegistries.length);
    for (const { registry, route, reason } of BACKEND_ONLY_NON_RESOURCE_REGISTRIES) {
      expect(registry, "backend-only non-inventory registry needs a stable id").toBeTruthy();
      expect(route, `${registry} needs the backend route recorded`).toMatch(/^\/api\/v1\//);
      expect(reason, `${registry} needs an explicit backend-only rationale`).toBeTruthy();
    }
  });

  it("keeps the browser parity smoke aligned with every standardized table id", () => {
    const scenarioIds = [...TABLE_CONTRACT_SCENARIO_IDS];

    expect(
      STANDARDIZED_TABLE_IDS.filter((tableId) => !scenarioIds.includes(tableId)),
    ).toEqual([]);
    expect(
      scenarioIds.filter((tableId) => !STANDARDIZED_TABLE_IDS.includes(tableId)),
    ).toEqual([]);
  });

  it("binds every standardized table to a canonical owner screen and API module", () => {
    const { filePathsByLiteralId, filePathsByDynamicExpression } = loadTableIdUsage();

    const inventoryOwnerPaths = (
      filePathsByDynamicExpression.get("`inventory.${activeTab}`") ?? []
    ).map(relativeSourcePath);
    expect(inventoryOwnerPaths).toEqual([INVENTORY_RESOURCE_TABLE_OWNER_FILE]);

    const inventoryOwnerSource = loadSource(
      path.resolve(WEB_SRC_ROOT, INVENTORY_RESOURCE_TABLE_OWNER_FILE),
    );
    expect(inventoryOwnerSource.includes(INVENTORY_RESOURCE_API_IMPORT)).toBe(true);
    const childOwnerPaths = (filePathsByDynamicExpression.get("childTableId") ?? []).map(relativeSourcePath);
    expect(childOwnerPaths).toEqual([INVENTORY_CHILD_TABLE_OWNER_FILE]);
    const childOwnerSource = loadSource(path.resolve(WEB_SRC_ROOT, INVENTORY_CHILD_TABLE_OWNER_FILE));
    expect(childOwnerSource.includes("inventoryResourceChildTableId")).toBe(true);
    const childAdapterSource = loadSource(path.resolve(WEB_SRC_ROOT, "features/inventory/components/inventory/inventoryResourceChildLists.ts"));
    expect(childAdapterSource.includes(INVENTORY_RESOURCE_API_IMPORT)).toBe(true);

    for (const [tableId, contract] of Object.entries(
      NON_RESOURCE_TABLE_CONTRACTS,
    ) as Array<[NonResourceTableId, NonResourceTableContract]>) {
      const ownerFileCandidates =
        tableId === "connections.targets.desktop" || tableId === "connections.targets.mobile"
          ? (filePathsByDynamicExpression.get("connectionsTableId") ?? []).map(relativeSourcePath)
          : (filePathsByLiteralId.get(tableId) ?? []).map(relativeSourcePath);
      expect(
        ownerFileCandidates,
        `${tableId} should be rendered only by ${contract.ownerFile}`,
      ).toEqual([contract.ownerFile]);

      const ownerSource = loadSource(path.resolve(WEB_SRC_ROOT, contract.ownerFile));
      if (contract.apiImports) {
        expect(
          contract.apiImports.some((apiImport) => ownerSource.includes(apiImport)),
          `${tableId} owner ${contract.ownerFile} must import one of: ${contract.apiImports.join(", ")}`,
        ).toBe(true);
      }

      if (contract.apiImports && contract.listRoute) {
        const apiModulePath = path.resolve(
          resolveApiModulePath(contract.ownerFile, contract.apiImports[0]!),
        );
        const apiModuleSource = loadSource(apiModulePath);
        expect(
          apiModuleSource.includes(contract.listRoute),
          `${tableId} API module ${relativeSourcePath(apiModulePath)} must use ${contract.listRoute}`,
        ).toBe(true);

        for (const forbiddenRoute of contract.forbiddenListRoutes ?? []) {
          expect(
            apiModuleSource.includes(forbiddenRoute),
            `${tableId} API module ${relativeSourcePath(apiModulePath)} must not use sibling route ${forbiddenRoute}`,
          ).toBe(false);
        }
      }
    }
  });

  it("keeps standardized table screens on the shared sort-state contract", () => {
    const { filePathsByLiteralId, filePathsByDynamicExpression } = loadTableIdUsage();
    const filePaths = new Set<string>();
    for (const paths of filePathsByLiteralId.values()) {
      for (const filePath of paths) filePaths.add(filePath);
    }
    for (const paths of filePathsByDynamicExpression.values()) {
      for (const filePath of paths) filePaths.add(filePath);
    }

    const staticTableFiles = new Set([
      path.resolve(
        WEB_SRC_ROOT,
        "features/connections/connection-config/EffectivePreview.tsx",
      ),
      path.resolve(
        WEB_SRC_ROOT,
        "features/inventory/components/ipam-workspace/IpamWorkspace.tsx",
      ),
      // Inspector metric workbench tables sort in-memory client data; like the
      // effective-preview table they don't map sort to a backend query.
      path.resolve(WEB_SRC_ROOT, "features/observability/InspectorMetricsScreen.tsx"),
      // The restore preview renders the in-memory dry-run plan; it has no
      // sortable columns and no server list query to map sort-state onto.
      path.resolve(WEB_SRC_ROOT, "features/backup/BackupPage.tsx"),
      // The three playbooks setup surfaces are client-only collections
      // (DataTable local sort, no server sort/filter params); their list state
      // lives in PlaybooksPage, not in the panel files.
      path.resolve(WEB_SRC_ROOT, "features/playbooks/ProjectsPanel.tsx"),
      path.resolve(WEB_SRC_ROOT, "features/playbooks/JobTemplatesPanel.tsx"),
      path.resolve(WEB_SRC_ROOT, "features/playbooks/ExecutionEnvironmentsPanel.tsx"),
    ]);

    for (const filePath of filePaths) {
      if (staticTableFiles.has(filePath)) continue;

      const source = loadSource(filePath);
      const hasSharedSortContract =
        source.includes("sanitizeDataTableSortState") ||
        source.includes("useAccessTableControls") ||
        // Access-control directory tables route through the shared list hook /
        // renderer, which owns the sort-state sanitization via `useAccessTableControls`.
        source.includes("useAccessListTable") ||
        source.includes("AccessEntityList") ||
        source.includes("useServerTable");

      expect(
        hasSharedSortContract,
        `${path.relative(WEB_SRC_ROOT, filePath)} must use shared sort-state sanitization`,
      ).toBe(true);
    }
  });

  it("keeps DataTable screen contracts in dedicated modules instead of screen-local allowlists", () => {
    const staticTableFiles = new Set([
      path.resolve(
        WEB_SRC_ROOT,
        "features/connections/connection-config/EffectivePreview.tsx",
      ),
      // The restore preview renders the in-memory dry-run plan; like the
      // effective-preview table it has no sortable or filterable columns and
      // therefore no sort/filter contract to move into a dedicated module.
      path.resolve(WEB_SRC_ROOT, "features/backup/BackupPage.tsx"),
      // The three playbooks setup surfaces are client-only collections without
      // a server sort/filter contract; their declarative list state stays in
      // the page-level tableConfig rather than a per-panel module.
      path.resolve(WEB_SRC_ROOT, "features/playbooks/ProjectsPanel.tsx"),
      path.resolve(WEB_SRC_ROOT, "features/playbooks/JobTemplatesPanel.tsx"),
      path.resolve(WEB_SRC_ROOT, "features/playbooks/ExecutionEnvironmentsPanel.tsx"),
    ]);

    const screenLocalAllowlistPattern =
      /const\s+[A-Z0-9_]+_(?:SORT_KEYS|SORTABLE_COLUMN_KEYS|FILTER(?:ABLE)?_(?:KEYS|COLUMNS))\s*=/;
    const dedicatedContractImportPattern = /from\s+["'][^"']*[Tt]ableConfig[^"']*["']/;

    const missingDedicatedContracts: string[] = [];
    const screenLocalAllowlists: string[] = [];

    for (const filePath of collectDataTableRenderFiles()) {
      if (staticTableFiles.has(filePath)) continue;

      const source = loadSource(filePath);
      const hasDedicatedContract =
        dedicatedContractImportPattern.test(source) ||
        source.includes("inventoryTableConfigFor") ||
        source.includes("useAccessTableControls");

      if (!hasDedicatedContract) {
        missingDedicatedContracts.push(path.relative(WEB_SRC_ROOT, filePath));
      }

      if (screenLocalAllowlistPattern.test(source)) {
        screenLocalAllowlists.push(path.relative(WEB_SRC_ROOT, filePath));
      }
    }

    expect(missingDedicatedContracts).toEqual([]);
    expect(screenLocalAllowlists).toEqual([]);
  });

  it("does not reintroduce bespoke column-filter updater helpers", () => {
    for (const filePath of collectSourceFiles(WEB_SRC_ROOT)) {
      const source = loadSource(filePath);
      expect(
        source.includes("function updateColumnFilters("),
        `${path.relative(WEB_SRC_ROOT, filePath)} defines a local updateColumnFilters`,
      ).toBe(false);
      expect(
        source.includes("const updateColumnFilters ="),
        `${path.relative(WEB_SRC_ROOT, filePath)} defines a local updateColumnFilters`,
      ).toBe(false);
    }
  });

  it("keeps standardized filterable table screens on the shared column-filter contract", () => {
    const { filePathsByLiteralId, filePathsByDynamicExpression } = loadTableIdUsage();
    const filePaths = new Set<string>();
    for (const paths of filePathsByLiteralId.values()) {
      for (const filePath of paths) filePaths.add(filePath);
    }
    for (const paths of filePathsByDynamicExpression.values()) {
      for (const filePath of paths) filePaths.add(filePath);
    }

    for (const filePath of filePaths) {
      const source = loadSource(filePath);
      const rendersFilterableTable =
        source.includes("columnFilters={") || source.includes("onColumnFilterChange={");
      if (!rendersFilterableTable) continue;

      const hasSharedFilterContract =
        source.includes("sanitizeDataTableColumnFilters") ||
        source.includes("useAccessTableControls") ||
        source.includes("useServerTable");

      expect(
        hasSharedFilterContract,
        `${path.relative(WEB_SRC_ROOT, filePath)} must use shared column-filter sanitization`,
      ).toBe(true);
    }
  });

  it("keeps declarative table configs for the remaining multi-table screens", () => {
    const playbooksSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/playbooks/PlaybooksPage.tsx"),
    );
    expect(playbooksSource.includes("PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG")).toBe(true);
    expect(playbooksSource.includes("PLAYBOOK_TARGET_SETS_TABLE_CONFIG")).toBe(true);
    expect(playbooksSource.includes("PLAYBOOK_SCHEDULES_TABLE_CONFIG")).toBe(true);
    expect(playbooksSource.includes("PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG")).toBe(true);
    expect(playbooksSource.includes("PLAYBOOK_RUN_GROUPS_TABLE_CONFIG")).toBe(true);
    expect(playbooksSource.includes("const PLAYBOOK_ASSIGNMENT_SORT_KEYS")).toBe(false);
    expect(playbooksSource.includes("const PLAYBOOK_TARGET_SET_SORT_KEYS")).toBe(false);
    expect(playbooksSource.includes("function csvFilterParam(")).toBe(false);

    const userSettingsSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/user-settings/UserSettings.tsx"),
    );
    expect(userSettingsSource.includes("USER_CREDENTIALS_TABLE_CONFIG")).toBe(true);
    expect(userSettingsSource.includes("USER_TOKENS_TABLE_CONFIG")).toBe(true);
    expect(userSettingsSource.includes("const USER_CREDENTIAL_SORT_KEYS")).toBe(false);
    expect(userSettingsSource.includes("const USER_TOKEN_SORT_KEYS")).toBe(false);

    const webhooksSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/webhooks/WebhooksPage.tsx"),
    );
    expect(webhooksSource.includes("WEBHOOKS_TABLE_CONFIG")).toBe(true);
    expect(webhooksSource.includes("const WEBHOOK_SORTABLE_COLUMN_KEYS")).toBe(false);
    expect(webhooksSource.includes("const WEBHOOK_FILTERABLE_COLUMN_KEYS")).toBe(false);

    const proxyNodesSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/connections/ProxyNodes.tsx"),
    );
    expect(proxyNodesSource.includes("PROXY_NODES_TABLE_CONFIG")).toBe(true);
    expect(proxyNodesSource.includes("PROXY_ASSIGNMENTS_TABLE_CONFIG")).toBe(true);
    expect(proxyNodesSource.includes("const PROXY_NODE_SORT_KEYS")).toBe(false);
    expect(proxyNodesSource.includes("const PROXY_ASSIGNMENT_SORT_KEYS")).toBe(false);

    const observabilitySource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/observability/ServiceDiscoveryScreen.tsx"),
    );
    expect(observabilitySource.includes("EXPORTER_ASSIGNMENTS_TABLE_CONFIG")).toBe(true);

    const activeConnectionsSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/connections/ActiveConnections.tsx"),
    );
    expect(activeConnectionsSource.includes("ACTIVE_CONNECTIONS_TABLE_CONFIG")).toBe(true);

    const connectionConfigSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/connections/connection-config/ConnectionConfig.tsx"),
    );
    expect(connectionConfigSource.includes("CONNECTION_CONFIG_RECORDS_TABLE_CONFIG")).toBe(true);

    const assignmentsSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/connections/connection-config/AssignmentsPanel.tsx"),
    );
    expect(assignmentsSource.includes("CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG")).toBe(true);

    const configContextsSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/inventory/ConfigContexts.tsx"),
    );
    expect(configContextsSource.includes("CONFIG_CONTEXTS_TABLE_CONFIG")).toBe(true);

    const syncMappingsSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/inventory/SyncMappingList.tsx"),
    );
    expect(syncMappingsSource.includes("SYNC_MAPPINGS_TABLE_CONFIG")).toBe(true);

    const auditSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/audit/AuditLog.tsx"),
    );
    expect(auditSource.includes("AUDIT_TABLE_CONFIG")).toBe(true);
    expect(auditSource.includes("AUDIT_SORT_KEY_BY_COLUMN")).toBe(false);

    const templatesSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/inventory/InventoryTemplates.tsx"),
    );
    expect(templatesSource.includes("INVENTORY_TEMPLATES_TABLE_CONFIG")).toBe(true);
    expect(templatesSource.includes("INVENTORY_TEMPLATE_RUNS_TABLE_CONFIG")).toBe(true);
    expect(templatesSource.includes("const TEMPLATE_RUN_SORT_KEYS")).toBe(false);
    expect(templatesSource.includes("const INVENTORY_TEMPLATE_SORT_KEYS")).toBe(false);

    const customFieldsSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/inventory/CustomFields.tsx"),
    );
    expect(customFieldsSource.includes("CUSTOM_FIELD_DEFINITIONS_TABLE_CONFIG")).toBe(true);
    expect(customFieldsSource.includes("CUSTOM_FIELD_CHOICE_SETS_TABLE_CONFIG")).toBe(true);
    expect(customFieldsSource.includes("const CUSTOM_FIELD_DEFINITION_SORT_KEYS")).toBe(false);
    expect(customFieldsSource.includes("const CUSTOM_FIELD_CHOICE_SET_SORT_KEYS")).toBe(false);
  });

  it("does not keep local filter-param routers in declarative table screens", () => {
    const configContextsSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/inventory/ConfigContexts.tsx"),
    );
    expect(configContextsSource.includes("buildConfigContextsFilterParams(sanitizedColumnFilters)")).toBe(
      true,
    );
    expect(configContextsSource.includes("const statusFilter = sanitizedColumnFilters.status ?? []")).toBe(
      false,
    );

    const syncMappingsSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/inventory/SyncMappingList.tsx"),
    );
    expect(syncMappingsSource.includes("buildSyncMappingsFilterParams(sanitizedColumnFilters)")).toBe(
      true,
    );
    expect(syncMappingsSource.includes("function syncMappingSortKey(")).toBe(false);

    const auditSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/audit/AuditLog.tsx"),
    );
    expect(auditSource.includes("buildAuditFilterParams(sanitizedColumnFilters)")).toBe(true);
    expect(auditSource.includes("const auditFilterKeys = useMemo")).toBe(true);
    expect(auditSource.includes("AUDIT_EVENT_FILTERS.map((o) => ({ value: o.value, label:")).toBe(
      true,
    );

    const templatesSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/inventory/InventoryTemplates.tsx"),
    );
    expect(templatesSource.includes("useServerTable")).toBe(true);
    expect(templatesSource.includes("buildInventoryTemplateFilterParams")).toBe(false);
    expect(templatesSource.includes("const statusFilter = sanitizedColumnFilters.status?.join")).toBe(
      false,
    );

    const customFieldsSource = loadSource(
      path.resolve(WEB_SRC_ROOT, "features/inventory/CustomFields.tsx"),
    );
    expect(customFieldsSource.includes("useServerTable")).toBe(true);
    expect(customFieldsSource.includes("buildCustomFieldDefinitionFilterParams")).toBe(false);
    expect(customFieldsSource.includes("function customFieldSortParams(")).toBe(false);
  });
});
