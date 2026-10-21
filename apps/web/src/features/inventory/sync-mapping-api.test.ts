import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  DELETE: vi.fn(),
  GET: vi.fn(),
  POST: vi.fn(),
  PATCH: vi.fn(),
}));

vi.mock("@/shared/api/client", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>();
  return {
    ...actual,
    apiClient: apiMocks,
    unwrap: (result: { data?: unknown; error?: unknown }) => {
      if (result.error) {
        if (result.error instanceof Error) throw result.error;
        throw new Error(typeof result.error === "string" ? result.error : "API mock error");
      }
      return result.data;
    },
  };
});

import { ApiError } from "@/shared/api/client";
import {
  getLocalInventoryResource,
  listLocalInventoryResources,
  listMappingConfigurations,
} from "./sync-mapping-api";
import { isRunnableMappingConfiguration } from "./sync-mapping-api";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("listMappingConfigurations", () => {
  it("can request the canonical unscoped list", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [],
        total: 0,
        limit: 500,
        offset: 0,
      },
    });

    await listMappingConfigurations({ limit: 500, offset: 0 });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/sync/mapping-configurations", {
      params: {
        query: {
          provider_id: undefined,
          logical_id: undefined,
          search: undefined,
          direction: undefined,
          sort: undefined,
          order: undefined,
          limit: 500,
          offset: 0,
        },
      },
    });
  });

  it("forwards standardized search, enum filter, and sort params", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [],
        total: 0,
        limit: 500,
        offset: 0,
      },
    });

    await listMappingConfigurations({
      provider_id: "provider-1",
      search: "cash registers",
      direction: ["pull", "bidirectional"],
      sort: "versions",
      order: "desc",
      limit: 500,
      offset: 0,
    });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/sync/mapping-configurations", {
      params: {
        query: {
          provider_id: "provider-1",
          logical_id: undefined,
          search: "cash registers",
          direction: "pull,bidirectional",
          sort: "versions",
          order: "desc",
          limit: 500,
          offset: 0,
        },
      },
    });
  });

  it("forwards logical_id when drilling into a single mapping group", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        items: [],
        total: 0,
        limit: 50,
        offset: 0,
      },
    });

    await listMappingConfigurations({
      provider_id: "provider-1",
      logical_id: "11111111-1111-4111-8111-111111111111",
      sort: "versions",
      order: "asc",
      limit: 50,
      offset: 0,
    });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/sync/mapping-configurations", {
      params: {
        query: {
          provider_id: "provider-1",
          logical_id: "11111111-1111-4111-8111-111111111111",
          search: undefined,
          direction: undefined,
          sort: "versions",
          order: "asc",
          limit: 50,
          offset: 0,
        },
      },
    });
  });
});

describe("isRunnableMappingConfiguration", () => {
  const runnable = {
    status: "active" as const,
    direction: "pull" as const,
    conflict_mode: "netbox_wins" as const,
    target: { kind: "local_inventory" as const },
  };

  it("accepts only the supported pull-into-local shape", () => {
    expect(isRunnableMappingConfiguration(runnable)).toBe(true);
  });

  it("rejects a provider target rather than assuming the local inventory", () => {
    expect(
      isRunnableMappingConfiguration({
        ...runnable,
        target: { kind: "provider", provider_id: "11111111-1111-4111-8111-111111111111" },
      }),
    ).toBe(false);
  });

  it("rejects a draft version, a push direction and a non-NetBox-wins mode", () => {
    expect(isRunnableMappingConfiguration({ ...runnable, status: "draft" })).toBe(false);
    expect(isRunnableMappingConfiguration({ ...runnable, direction: "push" })).toBe(false);
    expect(isRunnableMappingConfiguration({ ...runnable, conflict_mode: "piping_wins" })).toBe(false);
  });
});

describe("local inventory target catalogue", () => {
  it("projects the catalogue resources and their typed fields", async () => {
    apiMocks.GET.mockResolvedValue({
      data: {
        resources: [
          {
            source_entity: "dcim.devices",
            target_entity: "devices",
            fields: [
              { name: "name", field_type: "string", required: true },
              { name: "serial", field_type: "string", required: false },
            ],
          },
        ],
        provider_enabled: true,
      },
    });

    const catalogue = await listLocalInventoryResources("provider-1");

    expect(apiMocks.GET).toHaveBeenCalledWith(
      "/api/v1/sync/targets/local-inventory/resources",
      { params: { query: { provider_id: "provider-1" } } },
    );
    expect(catalogue.providerEnabled).toBe(true);
    expect(catalogue.resources).toEqual([
      {
        sourceEntity: "dcim.devices",
        targetEntity: "devices",
        fields: [
          { name: "name", fieldType: "string", required: true },
          { name: "serial", fieldType: "string", required: false },
        ],
      },
    ]);
  });

  it("returns null for an unknown catalogue entity instead of throwing", async () => {
    apiMocks.GET.mockResolvedValue({
      error: new ApiError({ status: 404, message: "Unknown entity" }),
    });

    await expect(getLocalInventoryResource("nope.nope")).resolves.toBeNull();
  });
});
