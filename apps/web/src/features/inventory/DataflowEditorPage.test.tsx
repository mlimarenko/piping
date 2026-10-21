import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import "@/shared/i18n";
import DataflowEditorPage from "./DataflowEditorPage";

const api = vi.hoisted(() => ({ getDataflow: vi.fn(), createDataflow: vi.fn(), updateDataflow: vi.fn() }));
vi.mock("./dataflows-api", async original => ({ ...(await original()), ...api }));
vi.mock("./api", () => ({ listAllResources: () => Promise.resolve([]) }));

const flow = {
  id: "flow-1", name: "Recovered flow", description: "", status: "draft", transport: "any", destination_ports: null,
  allow_cross_vrf: false,
  source: { object_type: "all", object_id: null, address_mode: "default", ipaddresses: [] },
  destination: { object_type: "internet", object_id: null, address_mode: "default", ipaddresses: [] },
};

function mount(path = "/inventory/dataflows/flow-1/edit", writable = true) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ" || writable, canAny: () => writable, hasRole: () => false }}>
      <MemoryRouter initialEntries={[path]}><Routes>
        <Route path="/inventory/dataflows/:id/edit" element={<DataflowEditorPage />} />
        <Route path="/inventory/dataflows/new" element={<DataflowEditorPage />} />
        <Route path="/inventory/dataflows" element={<div>dataflow-list</div>} />
      </Routes></MemoryRouter>
    </PermissionsContext.Provider>
  </QueryClientProvider>);
}

function expectShell() {
  expect(screen.getAllByTestId("entity-editor-page")).toHaveLength(1);
  expect(screen.getByTestId("entity-editor-page")).toHaveClass("max-w-[960px]");
  expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
}

describe("DataflowEditorPage state ownership", () => {
  beforeEach(() => vi.resetAllMocks());

  it("keeps one form-width shell from pending to loaded", async () => {
    let finish!: (value: unknown) => void;
    api.getDataflow.mockReturnValue(new Promise(resolve => { finish = resolve; }));
    mount();
    expect(screen.getByRole("status")).toHaveTextContent("Loading");
    expectShell();
    finish(flow);
    await screen.findByRole("textbox", { name: "Name (optional)" });
    expectShell();
    expect(screen.getByRole("button", { name: "Save Changes" })).toBeInTheDocument();
  });

  it.each(["error", "missing"])("owns %s and retry without granting write access", async state => {
    if (state === "error") api.getDataflow.mockRejectedValueOnce(new Error("private backend detail"));
    else api.getDataflow.mockResolvedValueOnce(null);
    api.getDataflow.mockResolvedValue(flow);
    mount(undefined, false);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load dataflows");
    expect(screen.queryByText("private backend detail")).not.toBeInTheDocument();
    expectShell();
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    await screen.findByRole("textbox", { name: "Name (optional)" });
    expectShell();
    expect(screen.getByRole("textbox", { name: "Name (optional)" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Save Changes" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("dataflow-list")).toBeInTheDocument();
  });

  it("keeps the create shell and guards dirty cancellation", async () => {
    mount("/inventory/dataflows/new");
    expectShell();
    expect(api.getDataflow).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "Name (optional)" }), { target: { value: "Unsaved" } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await screen.findByRole("alertdialog");
    fireEvent.click(screen.getByRole("button", { name: /Continue editing/i }));
    expect(screen.getByRole("textbox", { name: "Name (optional)" })).toHaveValue("Unsaved");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: /^Discard/i }));
    expect(await screen.findByText("dataflow-list")).toBeInTheDocument();
  });
});
