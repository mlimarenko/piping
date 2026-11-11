import fs from "node:fs";
import path from "node:path";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { parse } from "yaml";
import { z } from "zod";
import type * as InventoryApi from "./api";
import { tabConfig } from "./components/inventory/inventoryTypes";
import { inventoryTableConfigFor } from "./tableConfig";

const parameterSchema = z.object({
  name: z.string().min(1),
  in: z.enum(["query", "header", "path", "cookie"]),
});
const parameterOrReferenceSchema = z.union([
  parameterSchema,
  z.object({ $ref: z.string().startsWith("#/components/parameters/") }),
]);
const parametersSchema = z.array(parameterOrReferenceSchema).default([]);
const contractSchema = z.object({
  openapi: z.string().startsWith("3."),
  paths: z.record(z.string(), z.object({
    parameters: parametersSchema,
    get: z.object({ parameters: parametersSchema }).optional(),
  })),
  components: z.object({
    parameters: z.record(z.string(), parameterSchema).default({}),
  }).default({ parameters: {} }),
});

const contractPath = path.resolve(import.meta.dirname, "../../../../api/contracts/piping.openapi.yaml");
const contract = contractSchema.parse(parse(fs.readFileSync(contractPath, "utf8")));

let inventoryApi: typeof InventoryApi;
const transport = vi.fn<(request: Request) => Promise<Response>>();
const NativeRequest = Request;

beforeAll(async () => {
  vi.resetModules();
  vi.stubEnv("VITE_API_BASE_URL", "http://localhost");
  // openapi-fetch captures Request when its client is created. Node requires
  // absolute URLs, unlike the browser's same-origin request constructor.
  vi.stubGlobal("Request", class extends NativeRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(typeof input === "string" ? new URL(input, window.location.origin) : input, init);
    }
  });
  inventoryApi = await import("./api");
});

beforeEach(() => {
  // Keep the real openapi-fetch client, query serialization and error middleware
  // in the path. Only the HTTP transport is replaced; no backend is contacted.
  transport.mockReset();
  transport.mockImplementation(request => Promise.resolve(Response.json(
    new URL(request.url).pathname === "/api/v1/inventory/content-types"
      ? [] : { items: [], total: 0, limit: 25, offset: 0 },
  )));
  vi.stubGlobal("fetch", transport);
});

afterEach(() => vi.unstubAllGlobals());
afterAll(() => vi.unstubAllEnvs());

function queryParametersFor(routePath: string): Set<string> {
  const route = contract.paths[routePath];
  if (!route?.get) throw new Error(`Missing inventory list GET operation: ${routePath}`);

  const parameters = [...route.parameters, ...route.get.parameters].map((parameter) => {
    if (!("$ref" in parameter)) return parameter;
    const name = parameter.$ref.slice("#/components/parameters/".length)
      .replaceAll("~1", "/").replaceAll("~0", "~");
    const resolved = contract.components.parameters[name];
    if (!resolved) throw new Error(`Unresolved OpenAPI parameter: ${parameter.$ref}`);
    return resolved;
  });
  return new Set(parameters.filter((parameter) => parameter.in === "query").map((parameter) => parameter.name));
}

describe("inventory filter backend parity", () => {
  it.each(tabConfig)("lists $key through its public collection operation", async ({ key: entity }) => {
    const routePath = inventoryApi.inventoryTableListPathFor(entity);
    expect(routePath, `${entity} needs a public list route`).toBeDefined();
    if (!routePath) throw new Error(`Missing frontend inventory list route: ${entity}`);
    const accepted = queryParametersFor(routePath);
    const config = inventoryTableConfigFor(entity);
    const filters = Object.fromEntries(config.filterColumns.map(column => [column.param, ["first", "second"]]));

    const page = await inventoryApi.listResourcesForTable(entity, {
      limit: 25, offset: 0, search: "  lookup  ",
      ...(config.sortKeys.length > 0 ? { sort: config.sortKeys[0], order: "desc" as const } : {}),
      filters,
    });

    expect(page).toEqual({ items: [], total: 0, limit: 25, offset: 0 });
    const requests = transport.mock.calls.map(([request]) => request);
    const request = requests.find(request => new URL(request.url).pathname === routePath);
    expect(request, entity).toBeDefined();
    if (!request) throw new Error(`No collection request for ${entity}`);
    expect(request.method).toBe("GET");
    const query = new URL(request.url).searchParams;
    for (const name of query.keys()) expect(accepted.has(name), `${entity} sends undeclared ${name}`).toBe(true);
    expect(query.get("search")).toBe("lookup");
    expect(query.get("limit")).toBe("25");
    expect(query.get("offset")).toBe("0");
    for (const column of config.filterColumns) expect(query.get(column.param)).toBe("first,second");
    if (config.sortKeys.length > 0) {
      expect(query.get("sort")).toBe(config.sortKeys[0]);
      expect(query.get("order")).toBe("desc");
    } else {
      expect(query.has("sort")).toBe(false);
      expect(query.has("order")).toBe(false);
    }
  });

  it("omits inactive filters and unsupported sort controls for module bay templates", async () => {
    await inventoryApi.listResourcesForTable("Module Bay Templates", {
      limit: 25, offset: 0, search: "  ", filters: { device_type_id: [], module_type_id: [] },
    });
    expect(transport).toHaveBeenCalledTimes(1);
    const request = transport.mock.calls[0]?.[0];
    if (!request) throw new Error("Missing module bay template request");
    const query = new URL(request.url).searchParams;
    expect([...query.keys()].sort()).toEqual(["limit", "offset"]);
  });

  it("propagates a failed collection response instead of presenting an empty page", async () => {
    transport.mockResolvedValueOnce(Response.json({ message: "Invalid list query" }, { status: 400 }));
    await expect(inventoryApi.listResourcesForTable("Devices", { limit: 25, offset: 0 })).rejects.toMatchObject({ status: 400 });
    expect(transport).toHaveBeenCalledTimes(1);
  });

  it("declares every frontend inventory filter in the public list query contract", () => {
    const missing: string[] = [];
    const entities = inventoryApi.inventoryTableListPathEntities();
    expect(entities.length).toBeGreaterThan(0);

    for (const entity of entities) {
      const routePath = inventoryApi.inventoryTableListPathFor(entity);
      if (!routePath) throw new Error(`Missing frontend inventory list route: ${entity}`);
      const queryParameters = queryParametersFor(routePath);

      for (const filterColumn of inventoryTableConfigFor(entity).filterColumns) {
        if (!queryParameters.has(filterColumn.param)) {
          missing.push(`${entity}.${filterColumn.colKey} sends undeclared query parameter ${filterColumn.param}; route=${routePath}`);
        }
      }
    }

    expect(missing).toEqual([]);
  });

  it("declares search on every frontend inventory list route", () => {
    const missing: string[] = [];
    const entities = inventoryApi.inventoryTableListPathEntities();
    expect(entities.length).toBeGreaterThan(0);

    for (const entity of entities) {
      const routePath = inventoryApi.inventoryTableListPathFor(entity);
      if (!routePath) throw new Error(`Missing frontend inventory list route: ${entity}`);
      if (!queryParametersFor(routePath).has("search")) missing.push(`${entity}: ${routePath}`);
    }

    expect(missing).toEqual([]);
  });
});
