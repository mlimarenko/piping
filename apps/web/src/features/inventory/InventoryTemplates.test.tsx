import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { AllPermissions } from "@/test/permissions";
import { useState, type ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import InventoryTemplates from "./InventoryTemplates";
import type { InventoryTemplate } from "@/features/inventory/components/inventory/templateTypes";

const apiMocks = vi.hoisted(() => ({
  listTemplates: vi.fn(),
  createTemplate: vi.fn(),
  deleteTemplate: vi.fn(),
  runTemplate: vi.fn(),
  listTemplateRuns: vi.fn(),
  listRunSteps: vi.fn(),
}));

vi.mock("@/features/inventory/api", () => ({
  listTemplates: apiMocks.listTemplates,
  createTemplate: apiMocks.createTemplate,
  deleteTemplate: apiMocks.deleteTemplate,
  runTemplate: apiMocks.runTemplate,
  listTemplateRuns: apiMocks.listTemplateRuns,
  listRunSteps: apiMocks.listRunSteps,
}));

vi.mock("@/shared/hooks/useTablePageSize", () => ({
  useTablePageSize: () => ({ value: 25, setValue: vi.fn() }),
}));

vi.mock("@/features/inventory/components/inventory/TemplateRunDialog", () => ({
  default: function RunFixture({ open, onRun, onOpenChange }: { open: boolean; onRun: (values: Record<string, unknown>) => Promise<unknown>; onOpenChange: (open: boolean) => void }) {
    const [state, setState] = useState("");
    return open ? <div role="dialog">Run template<button onClick={async () => {
      setState("waiting");
      try { await onRun({ manual: "exact-slug" }); onOpenChange(false); }
      catch { setState("rejected"); }
    }}>Submit fixture</button><span>{state}</span></div> : null;
  },
}));

function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>{ui}</AllPermissions>
    </QueryClientProvider>,
  );
}

const template: InventoryTemplate = {
  id: "template-1",
  name: "Store template",
  slug: "store-template",
  status: "active",
  description: "Creates a store site",
  variables: [],
  steps: [],
  updatedAt: "2026-06-18T12:00:00Z",
};

describe("InventoryTemplates deletion", () => {
  let deleted = false;

  beforeEach(() => {
    deleted = false;
    apiMocks.listTemplates.mockImplementation(
      (query?: { status?: string; limit?: number; offset?: number }) => {
        const visible = !deleted;
        return Promise.resolve({
          items: visible ? [template] : [],
          total: visible ? 1 : 0,
          limit: query?.limit ?? 25,
          offset: query?.offset ?? 0,
        });
      },
    );
    apiMocks.deleteTemplate.mockImplementation(() => {
      deleted = true;
      return Promise.resolve();
    });
    apiMocks.listTemplateRuns.mockResolvedValue({ items: [], total: 0, limit: 8, offset: 0 });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("deletes a template and removes it from the list", async () => {
    renderWithClient(
      <MemoryRouter initialEntries={["/inventory/templates"]}>
        <InventoryTemplates />
      </MemoryRouter>,
    );

    expect(await screen.findByText("Store template")).toBeInTheDocument();

    fireEvent.keyDown(screen.getByRole("button", { name: "Actions: Store template" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Delete Template")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(apiMocks.deleteTemplate).toHaveBeenCalledWith("template-1");
    });
    await waitFor(() => {
      expect(screen.queryByText("Store template")).not.toBeInTheDocument();
    });
    await waitFor(() => expect(within(screen.getByRole("banner")).getByRole("button", { name: /Create Template/i })).toHaveFocus());
  });

  it("keeps running a template visible as the primary row action", async () => {
    renderWithClient(<MemoryRouter initialEntries={["/inventory/templates"]}><InventoryTemplates /></MemoryRouter>);
    const row = (await screen.findByText("Store template")).closest("tr");
    expect(row).not.toBeNull();
    fireEvent.click(within(row!).getByRole("button", { name: /Run Template/i }));
    expect(await screen.findByRole("dialog", { name: "" })).toHaveTextContent("Run template");
  });

  it("returns mutation acceptance to the wizard and retains its target on rejection", async () => {
    let reject!: (error: Error) => void;
    apiMocks.runTemplate.mockImplementation(() => new Promise((_, rejectPromise) => { reject = rejectPromise; }));
    renderWithClient(<MemoryRouter initialEntries={["/inventory/templates"]}><InventoryTemplates /></MemoryRouter>);
    const row = (await screen.findByText("Store template")).closest("tr")!;
    fireEvent.click(within(row).getByRole("button", { name: /Run Template/i }));
    fireEvent.click(screen.getByRole("button", { name: "Submit fixture" }));
    expect(await screen.findByText("waiting")).toBeInTheDocument();
    expect(apiMocks.runTemplate).toHaveBeenCalledWith("template-1", { manual: "exact-slug" });
    reject(new Error("rejected"));
    expect(await screen.findByText("rejected")).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(apiMocks.runTemplate).toHaveBeenCalledTimes(1);
  });
});
