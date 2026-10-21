import { setupServer } from "msw/node";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { AllPermissions } from "@/test/permissions";
import DeviceDashboardPage from "@/features/device-dashboard/DeviceDashboardPage";
import { getConnectionTarget } from "@/features/connections/api";
import type { components } from "@/shared/api/schema.gen";
import "@/shared/i18n";
import { handlers } from "../handlers";

vi.hoisted(() => {
  // Browser Request resolves the application's relative API paths; Node's does not.
  const NativeRequest = globalThis.Request;
  vi.stubGlobal("Request", class extends NativeRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(typeof input === "string" ? new URL(input, window.location.origin) : input, init);
    }
  });
});

const server = setupServer(...handlers);
const base = `${window.location.origin}/api/v1/connections`;

beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => { server.close(); vi.unstubAllGlobals(); });

describe("demo connection launches", () => {
  it("serves target details as the canonical row DTO and preserves inventory metadata", async () => {
    const response = await fetch(`${base}/targets`);
    const { items } = await response.json() as { items: components["schemas"]["ConnectionTargetListRowResponse"][] };
    expect(items.length).toBeGreaterThan(0);
    for (const target of items) {
      const detail = await fetch(`${base}/targets/${target.id}`);
      expect(detail.status).toBe(200);
      expect(await detail.json()).toEqual({ ...target, address_bound: true });
    }
  });

  it("returns 404 for an unknown target instead of a successful list envelope", async () => {
    const response = await fetch(`${base}/targets/unknown-target`);
    expect(response.status).toBe(404);
    await expect(getConnectionTarget("unknown-target")).rejects.toMatchObject({ status: 404 });
  });

  it("renders the device heading from the actual demo target-detail HTTP response", async () => {
    const response = await fetch(`${base}/targets`);
    const { items } = await response.json() as { items: components["schemas"]["ConnectionTargetListRowResponse"][] };
    const target = items.find(item => item.is_cctv)!;
    expect(target).toBeDefined();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(createElement(QueryClientProvider, { client },
      createElement(AllPermissions, null,
        createElement(MemoryRouter, { initialEntries: [`/connections/devices/${target.id}`] },
          createElement(Routes, null, createElement(Route, {
            path: "/connections/devices/:targetId", element: createElement(DeviceDashboardPage),
          })),
        ),
      ),
    ));
    expect(await screen.findByRole("heading", { level: 1, name: target.name })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByText(target.family!)).toBeInTheDocument();
    expect(await getConnectionTarget(target.id)).toMatchObject({
      id: target.id, name: target.name, primaryIp: target.address, targetKind: target.target_kind,
      family: target.family, deviceId: target.device_id, platformId: target.platform_id ?? undefined,
    });
  });

  it("links every connection target to its editable inventory object", async () => {
    const response = await fetch(`${base}/targets`);
    const { items } = await response.json() as { items: { name: string; device_id: string | null; vm_id: string | null }[] };
    for (const target of items) {
      const resource = target.device_id ? "devices" : "virtual_machines";
      const id = target.device_id ?? target.vm_id;
      const result = await fetch(`${window.location.origin}/api/v1/inventory/${resource}/${id}`);
      expect(result.ok).toBe(true);
      const entity = await result.json() as { id: string; name: string };
      expect(entity.name, `Inventory object for ${target.name}`).toBe(target.name);
      expect(entity.id).toBe(id);
    }
  });

  it("keeps two launched sessions distinct so both can occupy panes", async () => {
    const response = await fetch(`${base}/targets`);
    const { items } = await response.json() as { items: { id: string }[] };
    const launches = await Promise.all(items.slice(0, 2).map(async (target) => {
      const result = await fetch(`${base}/sessions`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ target_id: target.id, protocol: "ssh" }),
      });
      expect(result.ok).toBe(true);
      return await result.json() as {
        session: { id: string; target_id: string };
        proxy: { connection_id: string };
      };
    }));
    expect(launches).toHaveLength(2);
    expect(launches[0]!.session.target_id).not.toBe(launches[1]!.session.target_id);
    expect(launches[0]!.session.id).not.toBe(launches[1]!.session.id);
    expect(launches[0]!.proxy.connection_id).not.toBe(launches[1]!.proxy.connection_id);
  });
});
