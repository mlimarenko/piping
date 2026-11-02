import { act, cleanup, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, expect, it, vi } from "vitest";
import SessionRecovery from "./SessionRecovery";
import { fetchWithSession } from "@/shared/api/client";
import { safeLoginReturnTo } from "./api";
import type * as AuthApi from "./api";

const mocks = vi.hoisted(() => ({ me: vi.fn() }));
vi.mock("./api", async (importOriginal) => ({ ...await importOriginal<typeof AuthApi>(), me: mocks.me }));

afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.useRealTimers(); mocks.me.mockReset(); });

function Location() { const location = useLocation(); return <div>{location.pathname}{location.search}</div>; }

function mount(expiresAt = new Date(Date.now() + 120_000).toISOString()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(["auth", "me"], { id: "user", expiresAt });
  client.setQueryData(["inventory", "private"], { confidential: true });
  render(<QueryClientProvider client={client}><MemoryRouter initialEntries={["/inventory/resources?entity=Sites"]}>
    <SessionRecovery /><Routes><Route path="*" element={<Location />} /></Routes>
  </MemoryRouter></QueryClientProvider>);
  return client;
}

it("redirects a signed-in shell with a cached user when a later page request loses its session", async () => {
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json({}, { status: 401 })));
  const client = mount();
  await act(async () => { await fetchWithSession("/api/v1/inventory/sites"); });
  expect(await screen.findByText(/\/auth\/login\?redirect_to=/)).toBeInTheDocument();
  expect(client.getQueryData(["inventory", "private"])).toBeUndefined();
});

it("renews before expiry, then reads the extended server expiry", async () => {
  vi.useFakeTimers();
  const fetch = vi.fn((request: Request) => {
    if (new URL(request.url).pathname.endsWith("session-refresh")) return Promise.resolve(Response.json({ status: "refreshed" }));
    return Promise.resolve(Response.json({ user_id: "user", username: "test", email: "test@example.test", expires_at: new Date(Date.now() + 120_000).toISOString() }));
  });
  vi.stubGlobal("fetch", fetch);
  mocks.me.mockImplementation(() => Promise.resolve({ id: "user", expiresAt: new Date(Date.now() + 120_000).toISOString() }));
  mount();
  await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
  expect(fetch.mock.calls.filter(([request]) => new URL(request.url).pathname.endsWith("session-refresh"))).toHaveLength(1);
  expect(mocks.me).toHaveBeenCalledTimes(1);
});

it.each(["https://attacker.example/", "//attacker.example/", "/\\attacker.example/", "/auth/login", "javascript:alert(1)", "/..//attacker", "/%2e%2e//attacker", "/a/..//b"])("rejects external or looping login return path %s", (path) => {
  expect(safeLoginReturnTo(path)).toBe("/connections");
});
