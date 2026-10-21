import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import AccessGroups from "./AccessGroups";
import AccessRoles from "./AccessRoles";
import AccessGrants from "./AccessGrants";
import type * as AccessData from "./AccessDataContext";
import type * as AccessApi from "@/features/access-control/api";

const mocks = vi.hoisted(() => ({
  listGroups: vi.fn(), listRoles: vi.fn(), listGrants: vi.fn(),
  deleteGroup: vi.fn(), deleteRole: vi.fn(), deleteGrant: vi.fn(),
  updateGroup: vi.fn(), updateRole: vi.fn(),
}));
vi.mock("@/features/access-control/api", async original => ({ ...(await original<typeof AccessApi>()), ...mocks }));
vi.mock("./AccessDataContext", async original => ({
  ...(await original<typeof AccessData>()),
  useAccessData: () => ({ ...mocks, grants: [], policies: [], roles: [], users: [], groups: [] }),
  useAccessLabels: () => ({ role: (id: string) => ({ id, name: "Operators" }), subject: (id: string) => ({ id, name: "alice" }) }),
}));
vi.mock("@/shared/hooks/useTablePageSize", () => ({ useTablePageSize: () => ({ value: 25, setValue: vi.fn() }) }));

const cases = [
  { name: "group", Component: AccessGroups, label: "Actions: SRE", id: "group-1", remove: mocks.deleteGroup, command: "Delete", edit: "SRE", update: mocks.updateGroup, createLabel: "Add Group" },
  { name: "role", Component: AccessRoles, label: "Actions: Operators", id: "role-1", remove: mocks.deleteRole, command: "Delete", edit: "Operators", update: mocks.updateRole, createLabel: "Add Role" },
  { name: "grant", Component: AccessGrants, label: "Actions: Operators → alice", id: "grant-1", remove: mocks.deleteGrant, command: "Revoke" },
];

function mount(Component: typeof AccessGroups, actions = ["READ", "UPDATE", "DELETE"]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<MemoryRouter><QueryClientProvider client={client}><PermissionsContext.Provider value={{
    roles: [], isAdmin: false, can: action => actions.includes(action), canAny: () => false, hasRole: () => false,
  }}><Component /></PermissionsContext.Provider></QueryClientProvider></MemoryRouter>);
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.listGroups.mockResolvedValue({ items: [{ id: "group-1", name: "SRE", externalId: "" }], total: 1 });
  mocks.listRoles.mockResolvedValue({ items: [{ id: "role-1", name: "Operators", description: "" }], total: 1 });
  mocks.listGrants.mockResolvedValue({ items: [{ id: "grant-1", role: "role-1", subject: "user-1", subjectType: "user" }], total: 1 });
});

describe.each(cases)("$name row menu", entry => {
  it("requires confirmation, cancels without mutation and returns focus to the row", async () => {
    mount(entry.Component);
    const trigger = await screen.findByRole("button", { name: entry.label });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: entry.command }));
    const dialog = await screen.findByRole("alertdialog");
    expect(entry.remove).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(entry.remove).not.toHaveBeenCalled();
    await waitFor(() => expect(trigger).toHaveFocus());
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: entry.command }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: entry.command }));
    await waitFor(() => expect(entry.remove).toHaveBeenCalledWith(entry.id));
  });

  it("does not render an empty trigger for a READ-only viewer", async () => {
    mount(entry.Component, ["READ"]);
    await waitFor(() => expect(screen.getAllByRole("row").length).toBeGreaterThan(1));
    expect(screen.queryByRole("button", { name: entry.label })).toBeNull();
    if (entry.createLabel) expect(screen.queryByRole("button", { name: entry.createLabel })).toBeNull();
  });

  if (entry.createLabel) it("creates from the shell header and returns focus after cancelling", async () => {
    mount(entry.Component, ["READ", "CREATE"]);
    const create = await screen.findByRole("button", { name: entry.createLabel });
    expect(create.closest("header")).not.toBeNull();
    create.focus();
    fireEvent.click(create);
    const editor = await screen.findByTestId("entity-editor-page");
    expect(create.closest("[inert]")).not.toBeNull();
    fireEvent.click(within(editor).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByTestId("entity-editor-page")).toBeNull());
    await waitFor(() => expect(create).toHaveFocus());
    expect(create.closest("[inert]")).toBeNull();
  });

  if (entry.edit) it("opens the existing editor, saves and restores focus", async () => {
    mount(entry.Component);
    const trigger = await screen.findByRole("button", { name: entry.label });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const editor = await screen.findByTestId("entity-editor-page");
    fireEvent.change(within(editor).getByDisplayValue(entry.edit), { target: { value: "Renamed" } });
    fireEvent.click(within(editor).getByRole("button", { name: "Save Changes" }));
    await waitFor(() => expect(entry.update).toHaveBeenCalledWith(entry.id, expect.objectContaining({ name: "Renamed" })));
    await waitFor(() => expect(trigger).toHaveFocus());
  });
});
