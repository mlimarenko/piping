import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import type { ReactNode } from "react";
import { beforeEach, expect, it, vi } from "vitest";
import AppShell from "@/app/layout/AppShell";
import InventoryResourceDetailPage from "./InventoryResourceDetailPage";
import { ApiError } from "@/shared/api/client";
import type * as InventoryApi from "./api";

const get = vi.hoisted(() => vi.fn());
vi.mock("./api", async original => ({ ...await original<typeof InventoryApi>(), getResourceWithRaw: get }));
vi.mock("@/app/layout/StatusBar", () => ({ default: () => null }));
vi.mock("@/app/layout/UserMenu", () => ({ default: () => null }));
vi.mock("@/app/BrandingContext", () => ({ useBranding: () => ({ appName: "Test instance" }) }));
vi.mock("@/features/assistant", () => ({ AssistantProvider: ({ children }: { children: ReactNode }) => children, AssistantWindow: () => null, AssistantLauncher: () => null }));
vi.mock("@/shared/hooks/useUiScalePreference", () => ({ useUiScalePreference: () => ({ value: 1 }), useApplyUiScale: vi.fn() }));
vi.mock("@/shared/hooks/theme-context", () => ({ useTheme: () => ({ resolvedTheme: "light", setTheme: vi.fn() }) }));
vi.mock("@/features/connections/ConnectionsWorkspace", () => ({ default: () => null }));

beforeEach(() => { get.mockReset(); });

it.each([404, 500, "pending", "invalid", "denied"] as const)("keeps actual AppShell heading/back for detail %s", async state => {
  if (typeof state === "number") get.mockRejectedValue(new ApiError({ status: state, message: "private diagnostic" }));
  else get.mockReturnValue(new Promise(() => {}));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 30_000 } } });
  client.setQueryData(["auth", "me"], { id: "reader", username: "reader", roles: [], capabilities: state === "denied" ? [] : [{ action: "READ", resourceType: "INVENTORY" }] });
  const entity = state === "invalid" ? "invalid-entity" : "Owners";
  const view = render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[`/inventory/resources/${entity}/owner-1?return=https%3A%2F%2Foutside.test`]}><Routes>
    <Route element={<AppShell />}><Route path="/inventory/resources/:entity/:id" element={<InventoryResourceDetailPage />} /></Route>
  </Routes></MemoryRouter></QueryClientProvider>);
  if (state !== "pending") await screen.findByRole("alert");
  const header = await screen.findByRole("banner");
  expect(await within(header).findByRole("heading", { level: 1 })).toHaveTextContent(state === "invalid" ? "Resources" : "Owner");
  expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  expect(within(header).getByRole("heading", { level: 1 })).not.toHaveTextContent(/loading/i);
  const actions = screen.getByRole("link", { name: "Back" });
  expect(actions).toHaveAttribute("href", `/inventory/resources?entity=${state === "invalid" ? "" : "Owners"}`);
  expect(screen.queryByRole("link", { name: "Edit" })).toBeNull();
  expect(screen.queryByRole("button", { name: /Save/ })).toBeNull();
  expect(screen.queryByText("private diagnostic")).toBeNull();
  if (state === "invalid" || state === "denied") expect(get).not.toHaveBeenCalled();
  view.unmount();
  client.clear();
});
