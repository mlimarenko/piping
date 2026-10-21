import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Link, MemoryRouter, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import AccessGroups from "./AccessGroups";
import AccessRoles from "./AccessRoles";
import PageChrome from "@/shared/ui-shared/PageChrome";

const api = vi.hoisted(() => ({
  createGroup: vi.fn(), updateGroup: vi.fn(), deleteGroup: vi.fn(),
  createRole: vi.fn(), updateRole: vi.fn(), deleteRole: vi.fn(),
  listGroups: vi.fn(), listRoles: vi.fn(),
}));
vi.mock("./AccessDataContext", () => ({ useAccessData: () => ({ ...api, grants: [], policies: [] }) }));
vi.mock("@/features/access-control/api", async (original) => ({
  ...(await original()), listGroups: api.listGroups, listRoles: api.listRoles,
}));
vi.mock("@/shared/hooks/useTablePageSize", () => ({
  useTablePageSize: () => ({ value: 25, setValue: vi.fn() }),
}));

function NavigationProbe() { return <output data-testid="navigation">{useLocation().pathname}</output>; }
function renderEditor(Component: typeof AccessGroups, actions = ["READ", "CREATE", "UPDATE", "DELETE"]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => actions.includes(action), canAny: () => true, hasRole: () => false }}>
        <MemoryRouter><Link to="/audit-logs">Sidebar audit</Link><NavigationProbe /><PageChrome title="Access control" /><main id="main-content"><div><Component /></div></main></MemoryRouter>
      </PermissionsContext.Provider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  api.listGroups.mockResolvedValue({ items: [{ id: "group-1", name: "Operators", externalId: "remote-1" }], total: 1 });
  api.listRoles.mockResolvedValue({ items: [{ id: "role-1", name: "Operator", description: "Operations" }], total: 1 });
});

describe.each([
  { Component: AccessGroups, kind: "group", name: "Group name", second: "External ID", create: api.createGroup },
  { Component: AccessRoles, kind: "role", name: "Name", second: "Description", create: api.createRole },
])("$kind editor", ({ Component, kind, name, second, create }) => {
  it("describes name validation on the actual input and clears it after editing", async () => {
    renderEditor(Component);
    fireEvent.click(screen.getByRole("button", { name: new RegExp(`add ${kind}`, "i") }));
    const editor = await screen.findByTestId("entity-editor-page");
    const input = within(editor).getByLabelText(new RegExp(`^${name}`, "i"));
    fireEvent.click(within(editor).getByRole("button", { name: new RegExp(`create ${kind}`, "i") }));
    const error = within(editor).getByRole("alert");
    expect(input).toHaveAccessibleDescription(error.textContent);
    expect(input).toHaveAttribute("aria-invalid", "true");
    input.focus();
    fireEvent.change(input, { target: { value: "Corrected name" } });
    expect(input).toHaveFocus();
    expect(input).toHaveValue("Corrected name");
    expect(input).not.toHaveAttribute("aria-describedby");
    expect(input).toHaveAttribute("aria-invalid", "false");
    expect(create).not.toHaveBeenCalled();
  });
  it("retains the failed draft and its navigation protection after a pending save", async () => {
    let reject!: (error: Error) => void;
    create.mockImplementation(() => new Promise<void>((_resolve, fail) => { reject = fail; }));
    renderEditor(Component);
    fireEvent.click(screen.getByRole("button", { name: new RegExp(`add ${kind}`, "i") }));
    const editor = await screen.findByTestId("entity-editor-page");
    const input = within(editor).getByLabelText(new RegExp(`^${name}`, "i"));
    fireEvent.change(input, { target: { value: "Retained draft" } });
    fireEvent.click(within(editor).getByRole("button", { name: new RegExp(`create ${kind}`, "i") }));
    await waitFor(() => expect(input).toBeDisabled());
    fireEvent.click(screen.getByRole("link", { name: "Sidebar audit" }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    await act(async () => { reject(new Error("Local fixture failure")); await Promise.resolve(); });
    await waitFor(() => expect(input).toBeEnabled());
    expect(input).toHaveValue("Retained draft");
    expect(screen.getByTestId("navigation")).toHaveTextContent(/^\/$/);
    fireEvent.click(screen.getByRole("link", { name: "Sidebar audit" }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(screen.getByTestId("navigation")).toHaveTextContent(/^\/$/);
  });
  it("locks authoring and navigation while creation is pending", async () => {
    let complete!: () => void;
    create.mockImplementation(() => new Promise<void>(resolve => { complete = resolve; }));
    renderEditor(Component);
    fireEvent.click(screen.getByRole("button", { name: new RegExp(`add ${kind}`, "i") }));
    const editor = await screen.findByTestId("entity-editor-page");
    const primary = within(editor).getByLabelText(new RegExp(`^${name}`, "i"));
    const secondary = within(editor).getByLabelText(new RegExp(`^${second}`, "i"));
    fireEvent.change(primary, { target: { value: "Pending draft" } });
    fireEvent.click(within(editor).getByRole("button", { name: new RegExp(`create ${kind}`, "i") }));
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(primary).toBeDisabled();
    expect(secondary).toBeDisabled();
    fireEvent.change(primary, { target: { value: "Replacement draft" } });
    expect(primary).toHaveValue("Pending draft");
    fireEvent.click(within(editor).getByRole("button", { name: /back/i }));
    expect(editor).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: "Sidebar audit" }));
    expect(screen.getByTestId("navigation")).toHaveTextContent(/^\/$/);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    const unload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(unload);
    expect(unload.defaultPrevented).toBe(true);
    act(() => { complete(); });
    await waitFor(() => expect(screen.queryByTestId("entity-editor-page")).not.toBeInTheDocument());
  });
  it("owns the visible page heading and associates its fields", async () => {
    renderEditor(Component);
    fireEvent.click(screen.getByRole("button", { name: new RegExp(`add ${kind}`, "i") }));
    const editor = await screen.findByTestId("entity-editor-page");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(within(editor).getByRole("heading", { level: 2 })).toBeInTheDocument();
    expect(within(editor).getByLabelText(new RegExp(`^${name}`, "i"))).toBeEnabled();
    expect(within(editor).getByLabelText(new RegExp(`^${second}`, "i"))).toBeEnabled();
    fireEvent.change(within(editor).getByLabelText(new RegExp(`^${name}`, "i")), { target: { value: "Draft" } });
    fireEvent.click(within(editor).getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(screen.queryByTestId("entity-editor-page")).not.toBeInTheDocument());
    expect(screen.getByRole("heading", { level: 1, name: "Access control" })).toBeInTheDocument();
  });

  it("keeps DELETE permission independent from UPDATE and CREATE", async () => {
    renderEditor(Component, ["READ", "DELETE"]);
    const trigger = await screen.findByRole("button", { name: `Actions: ${kind === "group" ? "Operators" : "Operator"}` });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    expect(await screen.findByRole("menuitem", { name: "Delete" })).toBeEnabled();
    expect(screen.queryByRole("menuitem", { name: "Edit" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: new RegExp(`add ${kind}`, "i") })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    const confirmation = await screen.findByRole("alertdialog");
    const remove = kind === "group" ? api.deleteGroup : api.deleteRole;
    expect(remove).not.toHaveBeenCalled();
    fireEvent.click(within(confirmation).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(remove).toHaveBeenCalledWith(`${kind}-1`));
  });

  it("retains required-name validation before creation", async () => {
    renderEditor(Component);
    fireEvent.click(screen.getByRole("button", { name: new RegExp(`add ${kind}`, "i") }));
    const editor = await screen.findByTestId("entity-editor-page");
    fireEvent.click(within(editor).getByRole("button", { name: new RegExp(`create ${kind}`, "i") }));
    expect(create).not.toHaveBeenCalled();
    expect(within(editor).getByRole("alert")).toBeInTheDocument();
  });
});
