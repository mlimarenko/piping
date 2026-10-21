import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { inventoryTableListPathEntities, inventoryTableListPathFor } from "./api";
import { inventoryTableConfigFor } from "./tableConfig";

const WEB_SRC_ROOT = path.resolve(import.meta.dirname, "..", "..");
const API_ROOT = path.resolve(WEB_SRC_ROOT, "..", "..", "api");
const API_COMPLIANCE_ROOT = path.resolve(API_ROOT, "tests", "compliance");
const API_CONTEXTS_ROOT = path.resolve(API_ROOT, "src", "contexts");
const API_INVENTORY_ROUTES_ROOT = path.resolve(
  API_ROOT,
  "src",
  "api",
  "rest",
  "routes",
  "inventory",
);

function collectRustSourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) return collectRustSourceFiles(fullPath);
    if (!entry.name.endsWith(".rs")) return [];
    return [fullPath];
  });
}

function readSource(filePath: string): string {
  return fs.readFileSync(filePath, "utf8");
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
      escaped = char === "\\" && !escaped;
      if (char === "\"" && !escaped) inString = false;
      if (char !== "\\") escaped = false;
      continue;
    }

    if (char === "\"") {
      inString = true;
      continue;
    }
    if (char === openChar) depth += 1;
    if (char === closeChar) {
      depth -= 1;
      if (depth === 0) return source.slice(openIndex + 1, index);
    }
  }

  throw new Error(`Unbalanced ${openChar}${closeChar} source near ${openIndex}`);
}

function splitTopLevelArguments(source: string): string[] {
  const args: string[] = [];
  let start = 0;
  let depth = 0;
  let inString = false;
  let escaped = false;

  for (let index = 0; index < source.length; index += 1) {
    const char = source[index];

    if (inString) {
      escaped = char === "\\" && !escaped;
      if (char === "\"" && !escaped) inString = false;
      if (char !== "\\") escaped = false;
      continue;
    }

    if (char === "\"") {
      inString = true;
      continue;
    }
    if ("([{".includes(char ?? "")) depth += 1;
    if (")]}".includes(char ?? "")) depth -= 1;
    if (char === "," && depth === 0) {
      args.push(source.slice(start, index).trim());
      start = index + 1;
    }
  }

  args.push(source.slice(start).trim());
  return args.filter((arg) => arg.length > 0);
}

function readStringLiteralArgument(argument: string): string | null {
  const match = argument.match(/^"((?:\\.|[^"\\])*)"$/);
  return match ? (JSON.parse(match[0]) as string) : null;
}

function readListColumnArrayBody(source: string, constName: string): string | null {
  const constIndex = source.indexOf(`pub const ${constName}:`);
  if (constIndex === -1) return null;

  const initializerIndex = source.indexOf("= &[", constIndex);
  if (initializerIndex === -1) return null;

  const arrayOpenIndex = source.indexOf("[", initializerIndex);
  if (arrayOpenIndex === -1) return null;

  return readBalancedSource(source, arrayOpenIndex, "[", "]");
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

function loadInventoryRegistryConstNames(): Map<string, string> {
  const source = readSource(path.resolve(API_COMPLIANCE_ROOT, "inventory_table_columns.rs"));
  const constNamesByRegistryName = new Map<string, string>();

  for (const match of source.matchAll(
    /Registry\s*\{\s*name:\s*"([^"]+)",\s*const_name:\s*"([^"]+)"/g,
  )) {
    const registryName = match[1];
    const constName = match[2];
    if (registryName && constName) constNamesByRegistryName.set(registryName, constName);
  }

  return constNamesByRegistryName;
}

function loadFilterParamsByConstName(): Map<string, Set<string>> {
  const paramsByConstName = new Map<string, Set<string>>();

  for (const filePath of collectRustSourceFiles(API_CONTEXTS_ROOT)) {
    const source = readSource(filePath);
    for (const match of source.matchAll(/pub const ([A-Z0-9_]+):\s*&\[ListColumn\]/g)) {
      const constName = match[1];
      if (!constName) continue;

      const body = readListColumnArrayBody(source, constName);
      if (!body) continue;

      const params = new Set<string>();
      for (const param of readListColumnFilterParams(body, "enum_filter")) params.add(param);
      for (const param of readListColumnFilterParams(body, "enum_filter_with_param")) params.add(param);
      for (const param of readListColumnFilterParams(body, "fk_filter")) params.add(param);
      paramsByConstName.set(constName, params);
    }
  }

  return paramsByConstName;
}

function registryNameForRoute(routePath: string): string {
  const lastSegment = routePath.split("/").at(-1);
  if (!lastSegment) throw new Error(`Inventory route has no path segment: ${routePath}`);
  return lastSegment.replaceAll("-", "_");
}

function loadInventoryRouteSourcesByPath(): Map<string, string[]> {
  const routeSourcesByPath = new Map<string, string[]>();

  for (const filePath of collectRustSourceFiles(API_INVENTORY_ROUTES_ROOT)) {
    const source = readSource(filePath);
    for (const match of source.matchAll(/path\s*=\s*"([^"]+)"/g)) {
      const routePath = match[1];
      if (!routePath?.startsWith("/api/v1/inventory/")) continue;
      routeSourcesByPath.set(routePath, [...(routeSourcesByPath.get(routePath) ?? []), source]);
    }
  }

  return routeSourcesByPath;
}

describe("inventory filter backend parity", () => {
  it("keeps every FE inventory filter param declared by the backend ListColumn registry", () => {
    const constNamesByRegistryName = loadInventoryRegistryConstNames();
    const filterParamsByConstName = loadFilterParamsByConstName();
    const missing: string[] = [];

    for (const entity of inventoryTableListPathEntities()) {
      const routePath = inventoryTableListPathFor(entity);
      if (!routePath) continue;

      const registryName = registryNameForRoute(routePath);
      const constName = constNamesByRegistryName.get(registryName);
      const backendParams = constName ? filterParamsByConstName.get(constName) : undefined;

      for (const filterColumn of inventoryTableConfigFor(entity).filterColumns) {
        if (!backendParams?.has(filterColumn.param)) {
          missing.push(
            `${entity}.${filterColumn.colKey} sends ${filterColumn.param}; route=${routePath}; registry=${registryName}; const=${constName ?? "missing"}`,
          );
        }
      }
    }

    expect(missing).toEqual([]);
  });

  it("keeps every FE inventory table route wired to backend search", () => {
    const routeSourcesByPath = loadInventoryRouteSourcesByPath();
    const missing: string[] = [];

    for (const entity of inventoryTableListPathEntities()) {
      const routePath = inventoryTableListPathFor(entity);
      if (!routePath) continue;

      const routeSources = routeSourcesByPath.get(routePath);
      const routeHasSearch = routeSources?.some(
        (source) =>
          source.includes("pub search: Option<String>") &&
          // Trace the actual extractor binding, not a preferred local variable name.
          [...source.matchAll(/Query\(\s*(?:mut\s+)?([A-Za-z_]\w*)\s*\)/g)].some(
            (binding) => new RegExp(`\\b${binding[1]}\\s*\\.\\s*search\\b`).test(source),
          ),
      );
      if (!routeHasSearch) missing.push(`${entity}: ${routePath}`);
    }

    expect(missing).toEqual([]);
  });
});
