import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import InventoryRelatedValues from "./InventoryRelatedValues";

const api = vi.hoisted(() => ({ listAllResources: vi.fn() }));
vi.mock("@/features/inventory/api", () => api);
const id = "00000000-0000-4000-8000-000000009401";
beforeEach(() => { api.listAllResources.mockReset(); });

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter>
    <InventoryRelatedValues entity="Tags" ids={[id]} />
  </MemoryRouter></AllPermissions></QueryClientProvider>);
  return { client, cleanup: () => { view.unmount(); client.clear(); } };
}

it("resolves saved membership to catalog labels while retaining the UUID detail route", async () => {
  api.listAllResources.mockResolvedValue([{ id, name: "Renamed tag", type: "tag", color: "155dfc" }]);
  const { cleanup } = mount();
  const link = await screen.findByRole("link", { name: "Renamed tag" });
  expect(link).toHaveAttribute("href", `/inventory/resources/Tags/${id}`);
  expect(link.querySelector('[aria-hidden="true"]')).toHaveStyle({ backgroundColor: "#155dfc" });
  cleanup();
});

it.each(["denied", "missing"])("keeps an unavailable reference visible without making up a label (%s)", async outcome => {
  if (outcome === "denied") api.listAllResources.mockRejectedValue(new Error("private diagnostic"));
  else api.listAllResources.mockResolvedValue([]);
  const { client, cleanup } = mount();
  await waitFor(() => expect(client.getQueryState(["inventory", "resources", "Tags", "options"])?.status).toBe(outcome === "denied" ? "error" : "success"));
  expect(await screen.findByRole("link", { name: id })).toHaveAttribute("href", `/inventory/resources/Tags/${id}`);
  expect(screen.queryByText("private diagnostic")).not.toBeInTheDocument();
  cleanup();
});
