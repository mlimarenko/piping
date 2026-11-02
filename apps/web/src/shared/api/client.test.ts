import { afterEach, expect, it, vi } from "vitest";

afterEach(() => { vi.unstubAllGlobals(); vi.unstubAllEnvs(); vi.resetModules(); });

it("refreshes once and retries a protected request instead of exposing its 401", async () => {
  vi.resetModules();
  vi.stubEnv("VITE_API_BASE_URL", "http://localhost");
  const fetch = vi.fn()
    .mockResolvedValueOnce(Response.json({ message: "Expired session" }, { status: 401 }))
    .mockResolvedValueOnce(Response.json({ status: "refreshed" }))
    .mockResolvedValueOnce(Response.json({ items: [], total: 0 }));
  vi.stubGlobal("fetch", fetch);
  const { apiClient, API_BASE_URL } = await import("./client");
  const result = await apiClient.GET("/api/v1/inventory/sites", { baseUrl: API_BASE_URL || window.location.origin });
  expect(result.data).toMatchObject({ items: [], total: 0 });
  expect(fetch).toHaveBeenCalledTimes(3);
  expect(new URL((fetch.mock.calls[1]?.[0] as Request).url).pathname).toBe("/api/v1/auth/session-refresh");
});

async function clientWith(fetch: ReturnType<typeof vi.fn>) {
  vi.resetModules();
  vi.stubGlobal("fetch", fetch);
  return import("./client");
}

it("coalesces simultaneous 401s and preserves mutation bodies on the one retry", async () => {
  let refreshed = false;
  const bodies: string[] = [];
  const fetch = vi.fn(async (request: Request) => {
    if (new URL(request.url).pathname.endsWith("session-refresh")) {
      await Promise.resolve(); refreshed = true;
      return Response.json({ status: "refreshed" });
    }
    bodies.push(await request.text());
    return Response.json({}, { status: refreshed ? 200 : 401 });
  });
  const { fetchWithSession } = await clientWith(fetch);
  await Promise.all([1, 2].map((id) => fetchWithSession("/api/v1/inventory/sites", {
    method: "POST", body: JSON.stringify({ name: `Site ${id}` }),
  })));
  expect(fetch.mock.calls.filter(([request]) => new URL(request.url).pathname.endsWith("session-refresh"))).toHaveLength(1);
  expect(bodies.sort()).toEqual(['{"name":"Site 1"}', '{"name":"Site 1"}', '{"name":"Site 2"}', '{"name":"Site 2"}']);
});

it("keeps a real RBAC403 without refresh or logout", async () => {
  const fetch = vi.fn().mockResolvedValueOnce(Response.json({}, { status: 403 })).mockResolvedValueOnce(Response.json({ user_id: "user" }));
  const { fetchWithSession, subscribeSessionExpired } = await clientWith(fetch);
  const expired = vi.fn(); const unsubscribe = subscribeSessionExpired(expired);
  expect((await fetchWithSession("/api/v1/inventory/sites")).status).toBe(403);
  expect(fetch).toHaveBeenCalledTimes(2); expect(expired).not.toHaveBeenCalled(); unsubscribe();
});

it("notifies the global login boundary when anonymous403 is confirmed by me and refresh", async () => {
  const fetch = vi.fn().mockResolvedValueOnce(Response.json({}, { status: 403 }))
    .mockResolvedValueOnce(Response.json({}, { status: 401 })).mockResolvedValueOnce(Response.json({}, { status: 401 }));
  const { fetchWithSession, subscribeSessionExpired } = await clientWith(fetch);
  const expired = vi.fn(); const unsubscribe = subscribeSessionExpired(expired);
  await fetchWithSession("/api/v1/inventory/sites");
  expect(fetch).toHaveBeenCalledTimes(3); expect(expired).toHaveBeenCalledTimes(1); unsubscribe();
});

it("never retries indefinitely when refresh succeeds but the replay stays401", async () => {
  const fetch = vi.fn().mockResolvedValueOnce(Response.json({}, { status: 401 }))
    .mockResolvedValueOnce(Response.json({ status: "refreshed" })).mockResolvedValueOnce(Response.json({}, { status: 401 }));
  const { fetchWithSession, subscribeSessionExpired } = await clientWith(fetch);
  const expired = vi.fn(); const unsubscribe = subscribeSessionExpired(expired);
  expect((await fetchWithSession("/api/v1/inventory/sites")).status).toBe(401);
  expect(fetch).toHaveBeenCalledTimes(3); expect(expired).toHaveBeenCalledTimes(1); unsubscribe();
});

it.each([500, 503])("does not log out for refresh server failure %s", async (status) => {
  const fetch = vi.fn().mockResolvedValueOnce(Response.json({}, { status: 401 })).mockResolvedValueOnce(Response.json({}, { status }));
  const { fetchWithSession, subscribeSessionExpired } = await clientWith(fetch);
  const expired = vi.fn(); const unsubscribe = subscribeSessionExpired(expired);
  await expect(fetchWithSession("/api/v1/inventory/sites")).rejects.toMatchObject({ status });
  expect(expired).not.toHaveBeenCalled(); unsubscribe();
});

it("does not log out for a network failure", async () => {
  const fetch = vi.fn().mockRejectedValue(new TypeError("Network unavailable"));
  const { fetchWithSession, subscribeSessionExpired } = await clientWith(fetch);
  const expired = vi.fn(); const unsubscribe = subscribeSessionExpired(expired);
  await expect(fetchWithSession("/api/v1/inventory/sites")).rejects.toThrow("Network unavailable");
  expect(expired).not.toHaveBeenCalled(); unsubscribe();
});

it.each(["/api/v1/auth/login", "/api/v1/auth/logout", "https://external.example/api/v1/private"])("does not intercept %s", async (path) => {
  const fetch = vi.fn().mockResolvedValue(Response.json({}, { status: 401 }));
  const { fetchWithSession, subscribeSessionExpired } = await clientWith(fetch);
  const expired = vi.fn(); const unsubscribe = subscribeSessionExpired(expired);
  expect((await fetchWithSession(path)).status).toBe(401);
  expect(fetch).toHaveBeenCalledTimes(1); expect(expired).not.toHaveBeenCalled(); unsubscribe();
});

it("does not use browser refresh to recover a bearer-token request", async () => {
  const fetch = vi.fn().mockResolvedValue(Response.json({}, { status: 401 }));
  const { fetchWithSession } = await clientWith(fetch);
  await fetchWithSession("/api/v1/inventory/sites", { headers: { Authorization: "Bearer invalid-test-token" } });
  expect(fetch).toHaveBeenCalledTimes(1);
});

it("replays a stale anonymous403 after another request has already renewed the session", async () => {
  let siteCalls = 0;
  let deviceCalls = 0;
  let releaseDevice!: (response: Response) => void;
  const delayed403 = new Promise<Response>((resolve) => { releaseDevice = resolve; });
  const fetch = vi.fn((request: Request) => {
    const path = new URL(request.url).pathname;
    if (path.endsWith("/sites")) return Promise.resolve(Response.json({}, { status: ++siteCalls === 1 ? 401 : 200 }));
    if (path.endsWith("/devices")) return ++deviceCalls === 1 ? delayed403 : Promise.resolve(Response.json({}));
    return Promise.resolve(Response.json({ status: "refreshed" }));
  });
  const { fetchWithSession } = await clientWith(fetch);
  const device = fetchWithSession("/api/v1/inventory/devices");
  await fetchWithSession("/api/v1/inventory/sites");
  releaseDevice(Response.json({}, { status: 403 }));
  expect((await device).status).toBe(200);
  expect(deviceCalls).toBe(2);
});

it("preserves a typed openapi-fetch mutation across refresh", async () => {
  const bodies: unknown[] = [];
  const fetch = vi.fn(async (request: Request) => {
    if (new URL(request.url).pathname.endsWith("session-refresh")) return Response.json({ status: "refreshed" });
    bodies.push(await request.json());
    return Response.json({}, { status: bodies.length === 1 ? 401 : 200 });
  });
  const { apiClient, API_BASE_URL } = await clientWith(fetch);
  await apiClient.POST("/api/v1/inventory/sites", { baseUrl: API_BASE_URL || window.location.origin, body: { name: "Example", slug: "example" } });
  expect(bodies).toEqual([{ name: "Example", slug: "example" }, { name: "Example", slug: "example" }]);
});
