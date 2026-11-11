import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import TargetSetSelectorBuilder from "./TargetSetSelectorBuilder";
import { emptySelectorModel, modelToSelector } from "./selectorModel";

const api = vi.hoisted(() => ({ facets: vi.fn() }));
vi.mock("./api", () => ({ listTargetSetFacets: api.facets }));
const first = "11111111-1111-4111-8111-111111111111";
const second = "22222222-2222-4222-8222-222222222222";

function editor(model = emptySelectorModel()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onChange = vi.fn();
  render(<QueryClientProvider client={client}><TargetSetSelectorBuilder model={model} onChange={onChange} /></QueryClientProvider>);
  return { client, onChange };
}

describe("server target facets", () => {
  // Vitest treats a returned function as a cleanup hook.
  beforeEach(() => { api.facets.mockReset(); });

  it("keeps selected identifiers removable when the current search has no matching facet", async () => {
    api.facets.mockResolvedValue({ items: [{ id: first, name: "Live role", count: 3 }], total: 1, limit: 50, offset: 0 });
    const model = emptySelectorModel();
    model.facets.role = [first, second];
    const { onChange } = editor(model);
    expect(api.facets).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /Role 2/i }));
    await screen.findByText("Live role");
    fireEvent.click(screen.getByRole("checkbox", { name: new RegExp(second) }));
    expect(onChange).toHaveBeenCalledWith({ ...model, facets: { ...model.facets, role: [first] } });
  });

  it("loads the next server page and saves the chosen ID rather than its display name", async () => {
    api.facets.mockImplementation((_kind, _selector, _search, offset) => Promise.resolve({
      items: offset ? [{ id: second, name: "Second role", count: 7 }] : Array.from({ length: 50 }, (_, index) => ({
        id: index ? `33333333-3333-4333-8333-${String(index).padStart(12, "0")}` : first,
        name: index ? `Other ${index}` : "First role", count: 2,
      })),
      total: 51, limit: 50, offset,
    }));
    const { onChange } = editor();
    fireEvent.click(screen.getByRole("button", { name: /^Role$/i }));
    await screen.findByText("First role");
    fireEvent.click(screen.getByRole("button", { name: /^Next/i }));
    await screen.findByText("Second role");
    fireEvent.click(screen.getByRole("checkbox", { name: /Second role/ }));
    const next = onChange.mock.calls[0]?.[0];
    expect(modelToSelector(next)).toMatchObject({ role_ids: [second] });
    expect(modelToSelector(next)).not.toHaveProperty("role");
    expect(api.facets).toHaveBeenLastCalledWith("role", expect.any(Object), "", 50, 50);
  });

  it("uses server search and keeps load failures distinct from no matches", async () => {
    api.facets.mockRejectedValue(new Error("unavailable"));
    const { client } = editor();
    fireEvent.click(screen.getByRole("button", { name: /^Role$/i }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.queryByText("No matches")).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: /search: role/i }), { target: { value: "Renamed" } });
    await waitFor(() => { expect(api.facets).toHaveBeenLastCalledWith("role", expect.any(Object), "Renamed", 0, 50); });
    await waitFor(() => { expect(client.isFetching()).toBe(0); });
    expect(await screen.findByRole("alert")).toBeInTheDocument();
  });
});
