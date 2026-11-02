import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  DELETE: vi.fn(),
  GET: vi.fn(),
  POST: vi.fn(),
  PUT: vi.fn(),
}));

vi.mock("@/shared/api/client", () => ({
  apiClient: apiMocks,
  unwrap: (result: { data?: unknown; error?: unknown }) => {
    if (result.error) {
      if (result.error instanceof Error) throw result.error;
      throw new Error(typeof result.error === "string" ? result.error : "API mock error");
    }
    return result.data;
  },
}));

// listServicePrincipals pulls from the playbooks feature; stub it so importing
// the module under test does not drag in unrelated network surface.
vi.mock("@/features/playbooks/api", () => ({
  listPlaybookServicePrincipals: vi.fn(),
}));

import {
  buildAccessListParams,
  changeUserPassword,
  createPolicy,
  listAllUsers,
  listUsers,
  listGrants,
} from "./api";
import type { Policy } from "@/shared/api/types";

beforeEach(() => {
  Object.values(apiMocks).forEach((mock) => mock.mockReset());
});

describe("buildAccessListParams", () => {
  it("passes limit/offset/search and omits absent dimensions", () => {
    expect(buildAccessListParams({ limit: 25, offset: 50, search: "ops" })).toEqual({
      limit: 25,
      offset: 50,
      search: "ops",
    });
  });

  it("forwards sort with default asc order", () => {
    expect(buildAccessListParams({ limit: 25, offset: 0, sort: "username" })).toMatchObject({
      sort: "username",
      order: "asc",
    });
  });

  it("forwards explicit desc order", () => {
    expect(
      buildAccessListParams({ limit: 25, offset: 0, sort: "email", order: "desc" }),
    ).toMatchObject({ sort: "email", order: "desc" });
  });

  it("never emits an order without a sort key", () => {
    const params = buildAccessListParams({ limit: 25, offset: 0, order: "desc" });
    expect(params).not.toHaveProperty("sort");
    expect(params).not.toHaveProperty("order");
  });

  it("comma-joins multi-value filters and drops empty lists", () => {
    expect(
      buildAccessListParams({
        limit: 25,
        offset: 0,
        filters: { status: ["active", "suspended"], auth_source: ["oidc"], role_id: [] },
      }),
    ).toEqual({
      limit: 25,
      offset: 0,
      status: "active,suspended",
      auth_source: "oidc",
    });
  });
});

describe("listUsers", () => {
  it("sends sort + comma-joined IN filters to the users endpoint", async () => {
    apiMocks.GET.mockResolvedValue({
      data: { items: [{ id: "u1", username: "admin", email: "a@x", status: "active", auth_source: "local" }], total: 1, limit: 25, offset: 0 },
    });

    await listUsers({
      limit: 25,
      offset: 0,
      sort: "auth_source",
      order: "desc",
      filters: { status: ["active"], auth_source: ["local", "oidc"] },
    });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/admin/access-control/users", {
      params: {
        query: {
          limit: 25,
          offset: 0,
          sort: "auth_source",
          order: "desc",
          status: "active",
          auth_source: "local,oidc",
        },
      },
    });
  });
});

describe("listAllUsers", () => {
  it("pages to completion and concatenates every server page", async () => {
    // Two pages behind a total of 3: the loop must fetch page 2 (offset 2) and
    // stop once the running offset reaches total — otherwise a subject on a
    // later page resolves to a bare short-id instead of its name.
    apiMocks.GET
      .mockResolvedValueOnce({
        data: {
          items: [
            { id: "u1", username: "a", email: "a@x", status: "active", auth_source: "local" },
            { id: "u2", username: "b", email: "b@x", status: "active", auth_source: "local" },
          ],
          total: 3,
          limit: 500,
          offset: 0,
        },
      })
      .mockResolvedValueOnce({
        data: {
          items: [
            { id: "u3", username: "c", email: "c@x", status: "active", auth_source: "local" },
          ],
          total: 3,
          limit: 500,
          offset: 2,
        },
      });

    const users = await listAllUsers();

    expect(users.map((u) => u.id)).toEqual(["u1", "u2", "u3"]);
    expect(apiMocks.GET).toHaveBeenCalledTimes(2);
    expect(apiMocks.GET).toHaveBeenNthCalledWith(1, "/api/v1/admin/access-control/users", {
      params: { query: { limit: 500, offset: 0 } },
    });
    expect(apiMocks.GET).toHaveBeenNthCalledWith(2, "/api/v1/admin/access-control/users", {
      params: { query: { limit: 500, offset: 2 } },
    });
  });

  it("stops on an empty page even if total is over-reported", async () => {
    // Runaway guard: a server that reports a total larger than the rows it
    // actually returns must not loop forever.
    apiMocks.GET.mockResolvedValue({
      data: { items: [], total: 99, limit: 500, offset: 0 },
    });

    const users = await listAllUsers();

    expect(users).toEqual([]);
    expect(apiMocks.GET).toHaveBeenCalledTimes(1);
  });
});

describe("changeUserPassword", () => {
  it("sends a manual password rotation request for the selected user", async () => {
    apiMocks.PUT.mockResolvedValue({
      data: { user_id: "u1", password_changed_at: "2026-06-16T09:00:00Z" },
    });

    await changeUserPassword("u1", "new-password");

    expect(apiMocks.PUT).toHaveBeenCalledWith("/api/v1/admin/access-control/users/{id}/password", {
      params: { path: { id: "u1" } },
      body: { mode: "manual", password: "new-password" },
    });
  });
});

describe("policy resource serialization", () => {
  const basePolicy = (scopes: Policy["scopes"]): Omit<Policy, "id"> => ({
    name: "Secret grant",
    description: "",
    effect: "allow",
    enabled: true,
    actions: [{ id: "a1", action: "EXECUTE" }],
    subjects: [{ id: "u1", kind: "user", value: "user:u1", label: "u1" }],
    scopes,
  });

  it("serializes specific `resource` scopes into resources and keeps types separate", async () => {
    apiMocks.POST.mockResolvedValue({
      data: {
        id: "p1",
        name: "Secret grant",
        effect: "allow",
        enabled: true,
        actions: ["EXECUTE"],
        subjects: ["user:u1"],
        resource_types: ["secret_variable"],
        resources: ["secret_variable:11111111-1111-1111-1111-111111111111"],
      },
    });

    await createPolicy(
      basePolicy([
        { id: "s1", type: "resource_type", value: "secret_variable" },
        {
          id: "s2",
          type: "resource",
          value: "secret_variable:11111111-1111-1111-1111-111111111111",
        },
      ]),
    );

    expect(apiMocks.POST).toHaveBeenCalledWith(
      "/api/v1/admin/access-control/policies",
      expect.objectContaining({
        body: expect.objectContaining({
          resource_types: ["secret_variable"],
          resources: ["secret_variable:11111111-1111-1111-1111-111111111111"],
        }),
      }),
    );
  });

  it("falls back to the `*` wildcard when no specific resource is selected", async () => {
    apiMocks.POST.mockResolvedValue({
      data: {
        id: "p2",
        name: "Type grant",
        effect: "allow",
        enabled: true,
        actions: ["READ"],
        subjects: [],
        resource_types: ["secret_variable"],
        resources: ["*"],
      },
    });

    await createPolicy(basePolicy([{ id: "s1", type: "resource_type", value: "secret_variable" }]));

    const body = apiMocks.POST.mock.calls[0]?.[1]?.body;
    expect(body.resources).toEqual(["*"]);
  });

  it("reads specific resources back as `resource` scopes and drops the wildcard", async () => {
    apiMocks.POST.mockResolvedValue({
      data: {
        id: "p3",
        name: "Secret grant",
        effect: "allow",
        enabled: true,
        actions: ["EXECUTE"],
        subjects: [],
        resource_types: ["secret_variable"],
        resources: ["secret_variable:22222222-2222-2222-2222-222222222222", "*"],
      },
    });

    const policy = await createPolicy(basePolicy([]));

    expect(policy.scopes).toEqual([
      { id: "p3:rt:0", type: "resource_type", value: "secret_variable" },
      {
        id: "p3:r:0",
        type: "resource",
        value: "secret_variable:22222222-2222-2222-2222-222222222222",
      },
    ]);
  });
});

describe("listGrants", () => {
  it("forwards the caller-routed user_id/group_id IN filters", async () => {
    apiMocks.GET.mockResolvedValue({
      data: { items: [], total: 0, limit: 25, offset: 0 },
    });

    await listGrants({
      limit: 25,
      offset: 0,
      sort: "role",
      filters: { role_id: ["r1"], user_id: ["u1", "u2"], group_id: ["g1"] },
    });

    expect(apiMocks.GET).toHaveBeenCalledWith("/api/v1/admin/access-control/role-assignments", {
      params: {
        query: {
          limit: 25,
          offset: 0,
          sort: "role",
          order: "asc",
          role_id: "r1",
          user_id: "u1,u2",
          group_id: "g1",
        },
      },
    });
  });
});
