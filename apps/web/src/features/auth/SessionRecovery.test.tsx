import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, expect, it, vi } from "vitest";
import SessionRecovery from "./SessionRecovery";
import { fetchWithSession } from "@/shared/api/client";
import { safeLoginReturnTo } from "./api";
import type * as AuthApi from "./api";
import { BrandingProvider, useBranding } from "@/app/BrandingContext";
import { BRANDING_QUERY_KEY } from "@/shared/api/branding-api";
import type * as BrandingApi from "@/shared/api/branding-api";

const mocks = vi.hoisted(() => ({ me: vi.fn() }));
const branding = vi.hoisted(() => ({ get: vi.fn() }));
vi.mock("@/shared/api/branding-api", async original => ({ ...await original<typeof BrandingApi>(), getBranding: branding.get }));
vi.mock("./api", async (importOriginal) => ({ ...await importOriginal<typeof AuthApi>(), me: mocks.me }));

afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.useRealTimers(); mocks.me.mockReset(); });

function Location() { const location = useLocation(); return <div>{location.pathname}{location.search}</div>; }

function Brand() { return <div>{useBranding().appName}</div>; }

it("keeps the public branding observer alive when a protected session expires before branding resolves", async () => {
  let resolveBrand!: (value: unknown) => void;
  branding.get.mockImplementation(() => new Promise(resolve => { resolveBrand = resolve; }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["private"], "secret");
  let resolvePrivate!: (value: string) => void;
  const privateRequest = client.fetchQuery({ queryKey: ["pending-private"], queryFn: () => new Promise<string>(resolve => { resolvePrivate = resolve; }) }).catch(() => undefined);
  client.setQueryData(["auth", "me"], { id: "user" });
  mocks.me.mockResolvedValue({ id: "user" });
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json({}, { status: 401 })));
  render(<QueryClientProvider client={client}><BrandingProvider><MemoryRouter initialEntries={["/inventory"]}><SessionRecovery /><Brand /><Routes><Route path="*" element={<Location />} /></Routes></MemoryRouter></BrandingProvider></QueryClientProvider>);
  await act(async () => { await fetchWithSession("/api/v1/inventory/sites"); });
  await screen.findByText(/\/auth\/login\?redirect_to=/);
  expect(client.getQueryData(["private"])).toBeUndefined();
  act(() => { resolvePrivate("late secret"); });
  await privateRequest;
  expect(client.getQueryData(["pending-private"])).toBeUndefined();
  act(() => { resolveBrand({ app_name: "Printum", logo_data_url: null, favicon_data_url: null, theme: null }); });
  await waitFor(() => expect(screen.getByText("Printum")).toBeInTheDocument());
  expect(client.getQueryData(BRANDING_QUERY_KEY)).toMatchObject({ app_name: "Printum" });
  branding.get.mockResolvedValue({ app_name: "Updated instance", logo_data_url: null, favicon_data_url: null, theme: null });
  await act(async () => { await client.invalidateQueries({ queryKey: BRANDING_QUERY_KEY }); });
  expect(await screen.findByText("Updated instance")).toBeInTheDocument();
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json({})));
  await fetchWithSession("/api/v1/auth/login");
});

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

it("checks the server at an unchanged absolute deadline without a refresh loop", async () => {
  vi.useFakeTimers();
  const expiry = new Date(Date.now() + 1_000).toISOString();
  let refreshes = 0;
  vi.stubGlobal("fetch", vi.fn(() => {
    refreshes += 1;
    return Promise.resolve(refreshes === 1 ? Response.json({ status: "refreshed" }) : Response.json({}, { status: 401 }));
  }));
  mocks.me.mockResolvedValue({ id: "user", expiresAt: expiry });
  mount(expiry);
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  expect(refreshes).toBe(1);
  // The server answered with the same absolute deadline. A browser clock ahead
  // of the server must not turn that into a once-per-second refresh loop.
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(refreshes).toBe(1);
  await act(async () => { await vi.advanceTimersByTimeAsync(29_000); });
  expect(refreshes).toBe(2);
  expect(screen.getByText(/\/auth\/login\?redirect_to=/)).toBeInTheDocument();
});

it.each(["https://attacker.example/", "//attacker.example/", "/\\attacker.example/", "/auth/login", "javascript:alert(1)", "/..//attacker", "/%2e%2e//attacker", "/a/..//b"])("rejects external or looping login return path %s", (path) => {
  expect(safeLoginReturnTo(path)).toBe("/connections");
});
