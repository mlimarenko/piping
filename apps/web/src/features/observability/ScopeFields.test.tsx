import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import ScopeFields from "./ScopeFields";

const search = vi.hoisted(() => vi.fn());
vi.mock("@/features/connections/api", () => ({ searchConnectionTargets: search }));
beforeEach(async () => { vi.resetAllMocks(); await i18n.changeLanguage("en"); search.mockResolvedValue([{ id: "search-target", name: "Synthetic search match" }]); });
function mount(allowed = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: () => allowed, canAny: () => allowed, hasRole: () => false }}><ScopeFields scopeType="connection_target" scopeId="selected" onScopeTypeChange={vi.fn()} onScopeIdChange={vi.fn()} entities={{ optionsByScope: { role: [], site: [], tenant: [], connection_target: [{ id: "inventory-target", name: "Not a picker search result" }] }, resolveName: () => allowed ? "Synthetic selected identity" : "Name unavailable · selected", isPending: false }} /></PermissionsContext.Provider></QueryClientProvider>);
}
it("retains bounded lazy target search and uses the common selected identity instead of full inventory options", async () => {
  mount();
  const picker = screen.getByRole("combobox", { name: /^target/i });
  expect(picker).toHaveTextContent("Synthetic selected identity");
  expect(search).not.toHaveBeenCalled();
  fireEvent.click(picker);
  await screen.findByText("Synthetic search match");
  expect(screen.queryByText("Not a picker search result")).not.toBeInTheDocument();
  expect(search).toHaveBeenCalledWith("", 50);
  fireEvent.change(screen.getByPlaceholderText("Search connection targets…"), { target: { value: "edge" } });
  await waitFor(() => expect(search).toHaveBeenCalledWith("edge", 50));
});
it("shows a truthful error instead of empty search results on lookup failure", async () => {
  search.mockRejectedValue(new Error("private synthetic failure"));
  mount();
  fireEvent.click(screen.getByRole("combobox", { name: /^target/i }));
  await screen.findByText("Failed to load");
  expect(screen.queryByText("private synthetic failure")).not.toBeInTheDocument();
});
it("does not request denied target names or leave the picker loading forever", async () => {
  mount(false);
  fireEvent.click(screen.getByRole("combobox", { name: /^target/i }));
  await screen.findByText("Name unavailable");
  expect(search).not.toHaveBeenCalled();
});
