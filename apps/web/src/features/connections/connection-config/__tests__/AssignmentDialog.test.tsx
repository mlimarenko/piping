import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import type * as ConnectionApi from "../api";
import type * as InventoryApi from "@/features/inventory/api";
import AssignmentDialog from "../AssignmentDialog";

const api = vi.hoisted(() => ({createAssignment: vi.fn(), updateAssignment: vi.fn()}));
vi.mock("../api", async original => ({...await original<typeof ConnectionApi>(), ...api}));
vi.mock("@/features/inventory/api", async original => ({...await original<typeof InventoryApi>(), listResources: vi.fn().mockResolvedValue({items: []})}));

beforeEach(() => vi.clearAllMocks());
afterEach(() => {
  // Finish Radix focus restoration in the same DOM realm before Vitest replaces it.
  vi.useFakeTimers(); cleanup(); vi.runOnlyPendingTimers(); vi.useRealTimers();
});

it.each([false, true])("locks the entire assignment draft and cancel during a deferred save (edit=%s)", async editing => {
  let rejectSave!: (error: Error) => void;
  const save = editing ? api.updateAssignment : api.createAssignment;
  save.mockImplementation(() => new Promise((_resolve, reject) => { rejectSave = reject; }));
  const client = new QueryClient({defaultOptions: {queries: {retry: false}, mutations: {retry: false}}});
  const onOpenChange = vi.fn();
  const assignment = editing ? {id: "a1", record_id: "record-id", scope_type: "global", scope_id: null, weight: 3, enabled: true, created_at: "2026-10-05T00:00:00Z"} : undefined;
  const content = (open: boolean) => <QueryClientProvider client={client}><AssignmentDialog open={open} recordId="record-id" assignment={assignment} onOpenChange={onOpenChange} /></QueryClientProvider>;
  const view = render(content(false));
  view.rerender(content(true));
  if (!editing) {
    fireEvent.click(screen.getByRole("combobox", {name: "Scope"}));
    fireEvent.click(screen.getByRole("option", {name: "Global"}));
  }
  const weight = screen.getByRole("spinbutton", {name: "Weight"});
  fireEvent.change(weight, {target: {value: "9"}});
  fireEvent.click(screen.getByRole("button", {name: editing ? "Save Changes" : "Add"}));
  await waitFor(() => expect(save).toHaveBeenCalled());
  expect(weight).toBeDisabled();
  expect(screen.getByRole("combobox", {name: "Scope"})).toBeDisabled();
  expect(screen.getByRole("button", {name: "Cancel"})).toBeDisabled();
  if (editing) expect(screen.getByRole("switch")).toBeDisabled();
  rejectSave(new Error("save failed"));
  await waitFor(() => expect(weight).not.toBeDisabled());
  expect(weight).toHaveValue(9);
  expect(onOpenChange).not.toHaveBeenCalled();
});
