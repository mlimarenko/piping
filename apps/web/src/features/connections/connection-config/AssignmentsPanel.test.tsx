import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import type * as InventoryApi from "@/features/inventory/api";
import type * as ConnectionApi from "./api";
import type { ConnConfAssignment } from "./api";
import AssignmentsPanel from "./AssignmentsPanel";

const api = vi.hoisted(() => ({ listAssignments: vi.fn(), deleteAssignment: vi.fn() }));
vi.mock("./api", async original => ({ ...await original<typeof ConnectionApi>(), ...api }));
vi.mock("@/features/inventory/api", async original => ({
  ...await original<typeof InventoryApi>(),
  listResources: vi.fn().mockResolvedValue({ items: [{ id: "device-1", name: "Router Berlin" }] }),
}));

const assignment: ConnConfAssignment = {
  id: "assignment-1", record_id: "record-1", scope_type: "device", scope_id: "device-1",
  weight: 3, enabled: true, created_at: "2026-10-05T00:00:00Z",
};

function renderPanel(permissions = ["READ", "UPDATE", "DELETE"]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PermissionsContext.Provider value={{ roles: [], isAdmin: false,
        can: action => permissions.includes(action), canAny: () => false, hasRole: () => false }}>
        <AssignmentsPanel recordId="record-1" />
      </PermissionsContext.Provider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  api.listAssignments.mockResolvedValue([assignment]);
  api.deleteAssignment.mockResolvedValue(undefined);
});
afterEach(() => {
  // Let Radix restore focus before Vitest replaces the document.
  vi.useFakeTimers(); cleanup(); vi.runOnlyPendingTimers(); vi.useRealTimers();
});

describe("assignment row actions", () => {
  it("supports menu arrows, Escape and returning focus without invoking an action", async () => {
    renderPanel();
    const trigger = await screen.findByRole("button", { name: "Actions: Router Berlin" });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    const edit = await screen.findByRole("menuitem", { name: "Edit" });
    await waitFor(() => expect(edit).toHaveFocus());
    fireEvent.keyDown(edit, { key: "ArrowDown" });
    const remove = screen.getByRole("menuitem", { name: "Remove assignment" });
    await waitFor(() => expect(remove).toHaveFocus());
    fireEvent.keyDown(remove, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(api.deleteAssignment).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("edits the selected assignment and returns focus to its menu invoker on cancel", async () => {
    renderPanel();
    const trigger = await screen.findByRole("button", { name: "Actions: Router Berlin" });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("spinbutton", { name: "Weight" })).toHaveValue(3);
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(api.deleteAssignment).not.toHaveBeenCalled();
  });

  it("keeps the remove callback on the same assignment", async () => {
    renderPanel(["READ", "DELETE"]);
    fireEvent.pointerDown(await screen.findByRole("button", { name: "Actions: Router Berlin" }), { button: 0, ctrlKey: false });
    expect(screen.queryByRole("menuitem", { name: "Edit" })).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole("menuitem", { name: "Remove assignment" }));
    await waitFor(() => expect(api.deleteAssignment).toHaveBeenCalledWith("assignment-1"));
  });

  it("does not render an empty trigger without update or delete permission", async () => {
    renderPanel(["READ"]);
    expect(await screen.findByText("Router Berlin")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Actions:/ })).not.toBeInTheDocument();
    expect(screen.getByRole("spinbutton", { name: "Weight" })).toBeDisabled();
  });
});
