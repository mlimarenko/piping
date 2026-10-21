import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import type { ConnConfRecord } from "../api";
import type * as ConnectionApi from "../api";
import type * as AccessApi from "@/features/access-control/api";
import ConnectionConfig from "../ConnectionConfig";
import ConnectionConfigDetail from "../ConnectionConfigDetail";
import ConnectionConfigEditor from "../ConnectionConfigEditor";

const api = vi.hoisted(() => ({ listRecords: vi.fn(), createRecord: vi.fn(), updateRecord: vi.fn(), deleteRecord: vi.fn(), revealRecordCredentials: vi.fn() }));
const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("../api", async original => ({ ...await original<typeof ConnectionApi>(), ...api }));
vi.mock("@/shared/lib/notifications", () => ({ notify: notifyMock }));
vi.mock("@/features/access-control/api", async original => ({ ...await original<typeof AccessApi>(), listUsers: vi.fn().mockResolvedValue({ items: [] }) }));
vi.mock("../AssignmentsPanel", () => ({ default: ({recordId}: {recordId: string}) => <p>Assignments for {recordId}</p> }));
vi.mock("../EffectivePreview", () => ({ default: () => <p>Resolved target settings</p> }));

const fixture: ConnConfRecord = { id: "linux", name: "Linux SSH", description: "Shared Linux access", protocol: "ssh", enabled: true,
  created_at: "2026-10-05T00:00:00Z", updated_at: "2026-10-05T00:00:00Z", custom_field_data: {}, params: { host: "{{primary_ip4}}", credentials: { username: "root" } } };
function Location() { const location = useLocation(); return <output data-testid="location">{location.pathname}{location.search}</output>; }
function renderPages(path: string, readOnly = false) {
  const client = new QueryClient({defaultOptions: {queries: {retry: false}, mutations: {retry: false}}});
  const pages = <MemoryRouter initialEntries={[path]}>
    <Routes>
      <Route path="/connections/config" element={<ConnectionConfig />} />
      <Route path="/connections/config/new" element={<ConnectionConfigEditor />} />
      <Route path="/connections/config/:id/edit" element={<ConnectionConfigEditor />} />
      <Route path="/connections/config/:id" element={<ConnectionConfigDetail />} />
    </Routes><Location />
  </MemoryRouter>;
  render(<QueryClientProvider client={client}>{readOnly
    ? <PermissionsContext.Provider value={{roles: [], isAdmin: false, can: action => action === "READ", canAny: () => false, hasRole: () => false}}>{pages}</PermissionsContext.Provider>
    : <AllPermissions>{pages}</AllPermissions>}
  </QueryClientProvider>);
}
beforeEach(() => {
  vi.clearAllMocks();
  let records = [fixture];
  api.listRecords.mockImplementation(() => Promise.resolve(records));
  api.revealRecordCredentials.mockResolvedValue({ can_view_secrets: false, fields: [] });
  api.createRecord.mockImplementation(body => { const created = {...fixture, ...body, id: "created"}; records = [...records, created]; return Promise.resolve(created); });
  api.updateRecord.mockImplementation((id, body) => { const saved = {...fixture, ...body, id}; records = records.map(r => r.id === id ? saved : r); return Promise.resolve(saved); });
  api.deleteRecord.mockResolvedValue(undefined);
});
describe("connection configuration pages", () => {
  it("locks the whole draft until a deferred save settles and unlocks after failure", async () => {
    let rejectSave!: (error: Error) => void;
    api.updateRecord.mockImplementation(() => new Promise((_resolve, reject) => { rejectSave = reject; }));
    renderPages("/connections/config/linux/edit");
    const name = await screen.findByRole("textbox", {name: /^Name/});
    fireEvent.change(name, {target: {value: "Submitted SSH"}});
    fireEvent.click(screen.getByRole("button", {name: "Save Changes"}));
    await waitFor(() => expect(api.updateRecord).toHaveBeenCalled());
    expect(name).toBeDisabled();
    expect(screen.getByRole("combobox", {name: "Protocol"})).toBeDisabled();
    expect(screen.getByRole("switch", {name: "Connection enabled"})).toBeDisabled();
    fireEvent.change(name, {target: {value: "Late draft"}});
    expect(name).toHaveValue("Submitted SSH");
    rejectSave(new Error("save failed"));
    await waitFor(() => expect(name).not.toBeDisabled());
    expect(name).toHaveValue("Submitted SSH");
    fireEvent.change(name, {target: {value: "Retry SSH"}});
    expect(name).toHaveValue("Retry SSH");
  });
  it("opens a full-width record page from the list with independent overview, assignments and target check", async () => {
    renderPages("/connections/config");
    fireEvent.click(await screen.findByRole("link", {name: "Linux SSH"}));
    expect(await screen.findByRole("heading", {name: "Linux SSH"})).toBeInTheDocument();
    expect(screen.getByTestId("location")).toHaveTextContent("/connections/config/linux");
    expect(screen.getByRole("tab", {name: "Overview"})).toHaveAttribute("aria-selected", "true");
    fireEvent.mouseDown(screen.getByRole("tab", {name: "Assignments"}), { button: 0, ctrlKey: false });
    expect(await screen.findByText("Assignments for linux")).toBeInTheDocument();
    expect(screen.getByTestId("location")).toHaveTextContent("tab=assignments");
  });

  it("creates a configuration and opens its assignments as the next step", async () => {
    renderPages("/connections/config/new");
    fireEvent.change(await screen.findByRole("textbox", {name: /^Name/}), {target: {value: "New SSH"}});
    fireEvent.click(screen.getByRole("button", {name: "Create configuration"}));
    expect(await screen.findByText("Assignments for created")).toBeInTheDocument();
    expect(api.createRecord).toHaveBeenCalledWith(expect.objectContaining({name: "New SSH", enabled: true, protocol: "ssh", params: {host: "{{primary_ip4}}"}}));
    expect(screen.getByTestId("location")).toHaveTextContent("/connections/config/created?tab=assignments");
    expect(notifyMock).toHaveBeenCalledWith("success", 'Record "New SSH" created');
  });

  it("guards protocol changes and returns to the original record after discarding", async () => {
    renderPages("/connections/config/linux/edit");
    fireEvent.click(await screen.findByRole("combobox", {name: "Protocol"}));
    fireEvent.click(screen.getByRole("option", {name: "RDP"}));
    fireEvent.click(screen.getByRole("button", {name: "Cancel"}));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", {name: "Continue editing"}));
    expect(screen.getByRole("combobox", {name: "Protocol"})).toHaveTextContent("RDP");
    fireEvent.click(screen.getByRole("button", {name: "Cancel"}));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", {name: "Discard"}));
    expect(await screen.findByRole("heading", {name: "Linux SSH"})).toBeInTheDocument();
    expect(api.updateRecord).not.toHaveBeenCalled();
  });

  it("saves the enabled state through the standard editor", async () => {
    renderPages("/connections/config/linux/edit");
    fireEvent.click(await screen.findByRole("switch", {name: "Connection enabled"}));
    fireEvent.click(screen.getByRole("button", {name: "Save Changes"}));
    await waitFor(() => expect(api.updateRecord).toHaveBeenCalledWith("linux", expect.objectContaining({enabled: false})));
    expect(await screen.findByRole("heading", {name: "Linux SSH"})).toBeInTheDocument();
    expect(notifyMock).toHaveBeenCalledWith("success", "Record updated");
  });

  it("reports a successful delete", async () => {
    renderPages("/connections/config/linux");
    fireEvent.click(await screen.findByRole("button", {name: "Delete"}));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", {name: "Delete"}));
    await waitFor(() => expect(api.deleteRecord).toHaveBeenCalledWith("linux"));
    expect(notifyMock).toHaveBeenCalledWith("success", "Record deleted");
    expect(screen.getByTestId("location")).toHaveTextContent("/connections/config");
  });

  it("canonical configuration actions preserve confirmation and return focus", async () => {
    renderPages("/connections/config/linux");
    expect(screen.getByRole("link", { name: "Back to configurations" })).toHaveAttribute("href", "/connections/config");
    const edit = await screen.findByRole("link", { name: "Edit" });
    expect(edit).toHaveAttribute("href", "/connections/config/linux/edit");
    edit.focus();
    expect(edit).toHaveFocus();
    const remove = screen.getByRole("button", { name: "Delete" });
    remove.focus();
    expect(remove).toHaveFocus();
    fireEvent.click(remove);
    expect(api.deleteRecord).not.toHaveBeenCalled();
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(remove).toHaveFocus());
    expect(api.deleteRecord).not.toHaveBeenCalled();
    expect(screen.getByTestId("location")).toHaveTextContent(/^\/connections\/config\/linux$/);
  });

  it("canonical configuration creation is in the header and read-only detail keeps back navigation", async () => {
    renderPages("/connections/config/linux", true);
    await screen.findByRole("heading", { name: "Linux SSH" });
    expect(screen.queryByRole("button", { name: /^Actions:/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Edit" })).not.toBeInTheDocument();
    const back = screen.getByRole("link", { name: "Back to configurations" });
    back.focus(); expect(back).toHaveFocus(); fireEvent.click(back);
    await screen.findByRole("link", { name: "Linux SSH" });
    expect(screen.queryByRole("link", { name: "Create configuration" })).not.toBeInTheDocument();
  });

  it("canonical configuration root keeps its create entry in PageChrome", async () => {
    renderPages("/connections/config");
    await screen.findByRole("link", { name: "Linux SSH" });
    expect(within(screen.getByRole("banner")).getByRole("link", { name: "Create configuration" })).toHaveAttribute("href", "/connections/config/new");
  });

  it("canonical configuration edit activates its routed editor", async () => {
    renderPages("/connections/config/linux");
    fireEvent.click(await screen.findByRole("link", { name: "Edit" }));
    expect(await screen.findByRole("textbox", { name: /^Name/ })).toHaveValue("Linux SSH");
    expect(screen.getByTestId("location")).toHaveTextContent("/connections/config/linux/edit");
  });

  it("keeps the draft after a failed save and retries without losing changes", async () => {
    renderPages("/connections/config/linux/edit");
    fireEvent.change(await screen.findByRole("textbox", {name: /^Name/}), {target: {value: "Updated SSH"}});
    api.updateRecord.mockRejectedValueOnce(new Error("Unavailable"));
    fireEvent.click(screen.getByRole("button", {name: "Save Changes"}));
    expect(await screen.findByRole("alert")).toHaveTextContent("Failed to update record");
    expect(screen.getByRole("textbox", {name: /^Name/})).toHaveValue("Updated SSH");
    fireEvent.click(screen.getByRole("button", {name: "Save Changes"}));
    expect(await screen.findByRole("heading", {name: "Updated SSH"})).toBeInTheDocument();
  });

  it("does not submit an unnamed configuration", async () => {
    renderPages("/connections/config/new");
    fireEvent.click(await screen.findByRole("button", {name: "Create configuration"}));
    expect(screen.getByRole("alert")).toHaveTextContent("Name is required");
    expect(api.createRecord).not.toHaveBeenCalled();
  });

  it("offers retry after a load error", async () => {
    api.listRecords.mockRejectedValueOnce(new Error("Unavailable"));
    renderPages("/connections/config/linux");
    fireEvent.click(await screen.findByRole("button", {name: "Retry"}));
    expect(await screen.findByRole("heading", {name: "Linux SSH"})).toBeInTheDocument();
  });

  it.each(["new", "linux/edit"])("denies direct editor access without mutation permissions: %s", async path => {
    renderPages(`/connections/config/${path}`, true);
    expect(await screen.findByRole("heading", { level: 1, name: "You don't have access to this section" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.queryByRole("textbox", {name: /^Name/})).not.toBeInTheDocument();
    expect(api.createRecord).not.toHaveBeenCalled();
    expect(api.updateRecord).not.toHaveBeenCalled();
  });
});
