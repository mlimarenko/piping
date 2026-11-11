import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import AccessUsers from "./AccessUsers";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import type * as AccessDataModule from "./AccessDataContext";
import type * as AccessControlApi from "@/features/access-control/api";

/* ── Mocks ─────────────────────────────────────────────────────────────────
   The user table is driven by `listUsers`; the dialogs' mutation callbacks and
   the role chips' label resolver come from the AccessDataContext hooks. We
   override those two hooks and the paginated list call, mirroring the sibling
   AccessPolicies.test.tsx harness. */

const accessDataMock = vi.hoisted(() => ({
  users: [] as Array<{ id: string; username: string; email: string; status: string; authSource: string; externalId: string }>,
  createUser: vi.fn(),
  updateUser: vi.fn(),
  changeUserPassword: vi.fn(),
  deleteUser: vi.fn(),
}));

vi.mock("./AccessDataContext", async (importOriginal) => {
  const actual = await importOriginal<typeof AccessDataModule>();
  return {
    ...actual,
    useAccessData: () =>
      ({
        createUser: accessDataMock.createUser,
        updateUser: accessDataMock.updateUser,
        changeUserPassword: accessDataMock.changeUserPassword,
        deleteUser: accessDataMock.deleteUser,
        // The refreshed directory the editor re-reads before submitting.
        users: accessDataMock.users,
        grants: [],
      }) as unknown as ReturnType<typeof actual.useAccessData>,
    useAccessLabels: () =>
      ({
        role: (value: string) => ({ id: value }),
      }) as unknown as ReturnType<typeof actual.useAccessLabels>,
  };
});

const listUsersMock = vi.hoisted(() => vi.fn());
vi.mock("@/features/access-control/api", async (importOriginal) => ({
  ...(await importOriginal<typeof AccessControlApi>()),
  listUsers: listUsersMock,
}));

vi.mock("@/shared/hooks/useTablePageSize", () => ({
  useTablePageSize: () => ({ value: 25, setValue: vi.fn(), isLoading: false }),
}));

/* ── Render helper ─────────────────────────────────────────────────────── */

const localUser = { id: "user-1", username: "alice", email: "alice@example.test", status: "active" as const, authSource: "local", externalId: "" };
const oidcUser = { ...localUser, id: "user-2", username: "bob", email: "bob@example.test", authSource: "oidc", externalId: "ext-bob" };

function renderUsers() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <AccessUsers />
      </AllPermissions>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  accessDataMock.users = [];
  listUsersMock.mockResolvedValue({ items: [], total: 0 });
});

describe("AccessUsers password placeholder", () => {
  it("describes invalid create controls and clears descriptions after correction", async () => {
    renderUsers();
    fireEvent.click(screen.getByRole("button", { name: /add user/i }));
    const editor = await screen.findByTestId("entity-editor-page");
    fireEvent.change(within(editor).getByLabelText(/^password/i), { target: { value: "short" } });
    fireEvent.click(within(editor).getByRole("button", { name: /create user/i }));
    for (const label of [/^username/i, /^email/i, /^password/i, /^confirm password/i]) {
      const control = within(editor).getByLabelText(label);
      const error = control.closest('[data-slot="form-field"]')!.querySelector('[role="alert"]')!;
      expect(error).toBeInTheDocument();
      expect(control).toHaveAccessibleDescription(error.textContent);
      expect(control).toHaveAttribute("aria-invalid", "true");
    }
    const username = within(editor).getByLabelText(/^username/i);
    username.focus();
    fireEvent.change(username, { target: { value: "alice" } });
    expect(username).toHaveFocus();
    expect(username).toHaveValue("alice");
    expect(username).toHaveAttribute("aria-invalid", "false");
    expect(username).not.toHaveAttribute("aria-describedby");
    expect(accessDataMock.createUser).not.toHaveBeenCalled();
  });
  it("omits the menu for READ-only users", async () => {
    listUsersMock.mockResolvedValue({ items: [{ id: "user-1", username: "alice", email: "alice@example.test", status: "active", authSource: "local", externalId: "" }], total: 1 });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: action => action === "READ", canAny: () => false, hasRole: () => false }}><AccessUsers /></PermissionsContext.Provider></QueryClientProvider>);
    await screen.findByText("alice");
    expect(screen.queryByRole("button", { name: "Actions: alice" })).toBeNull();
  });

  it("keeps editing and password reset available for a locally provisioned user", async () => {
    listUsersMock.mockResolvedValue({ items: [localUser], total: 1 });
    accessDataMock.users = [localUser];
    renderUsers();
    fireEvent.keyDown(await screen.findByRole("button", { name: "Actions: alice" }), { key: "Enter" });
    expect(await screen.findByRole("menuitem", { name: "Edit" })).toBeEnabled();
    expect(screen.getByRole("menuitem", { name: /change password/i })).toBeInTheDocument();
  });

  it("offers no edit or password action for an externally managed account", async () => {
    listUsersMock.mockResolvedValue({ items: [oidcUser], total: 1 });
    accessDataMock.users = [oidcUser];
    renderUsers();
    fireEvent.keyDown(await screen.findByRole("button", { name: "Actions: bob" }), { key: "Enter" });
    expect(screen.queryByRole("menuitem", { name: "Edit" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: /change password/i })).toBeNull();
    // Deletion is a separate administrative boundary and stays available.
    expect(screen.getByRole("menuitem", { name: /delete/i })).toBeInTheDocument();
  });

  it("submits a local edit and keeps the account's auth source fixed", async () => {
    accessDataMock.updateUser.mockResolvedValue(undefined);
    listUsersMock.mockResolvedValue({ items: [localUser], total: 1 });
    accessDataMock.users = [localUser];
    renderUsers();
    fireEvent.keyDown(await screen.findByRole("button", { name: "Actions: alice" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const editor = await screen.findByTestId("entity-editor-page");
    expect(within(editor).getByRole("combobox", { name: /auth.*source/i })).toBeDisabled();
    fireEvent.change(within(editor).getByLabelText(/^username/i), { target: { value: "alice-renamed" } });
    fireEvent.click(within(editor).getByRole("button", { name: /save changes/i }));
    await waitFor(() =>
      expect(accessDataMock.updateUser).toHaveBeenCalledWith(
        "user-1",
        expect.objectContaining({ username: "alice-renamed", authSource: "local" }),
      ),
    );
  });

  it("refuses to submit a stale editor whose refreshed record became externally managed", async () => {
    listUsersMock.mockResolvedValue({ items: [localUser], total: 1 });
    accessDataMock.users = [localUser];
    renderUsers();
    fireEvent.keyDown(await screen.findByRole("button", { name: "Actions: alice" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const editor = await screen.findByTestId("entity-editor-page");

    // The directory refresh behind the open dialog now reports the account as
    // OIDC-owned; the typed change re-renders the editor against that record.
    accessDataMock.users = [{ ...localUser, authSource: "oidc", externalId: "ext-alice" }];
    fireEvent.change(within(editor).getByLabelText(/^email/i), { target: { value: "alice2@example.test" } });

    expect(await within(editor).findByRole("alert")).toHaveTextContent(/external identity provider/i);
    const save = within(editor).getByRole("button", { name: /save changes/i });
    expect(save).toBeDisabled();
    fireEvent.click(save);
    expect(accessDataMock.updateUser).not.toHaveBeenCalled();
  });
  it("describes both reset-password validation errors on their actual inputs", async () => {
    listUsersMock.mockResolvedValue({ items: [{ id: "user-1", username: "alice", email: "alice@example.test", status: "active", authSource: "local", externalId: "" }], total: 1 });
    renderUsers();
    fireEvent.keyDown(await screen.findByRole("button", { name: "Actions: alice" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: /change password/i }));
    const dialog = await screen.findByRole("dialog");
    const password = within(dialog).getByLabelText(/^New password/i);
    fireEvent.change(password, { target: { value: "short" } });
    fireEvent.click(within(dialog).getByRole("button", { name: /save|change password/i }));
    for (const label of [/^New password/i, /^Confirm password/i]) {
      const control = within(dialog).getByLabelText(label);
      const error = control.closest('[data-slot="form-field"]')!.querySelector('[role="alert"]')!;
      expect(control).toHaveAccessibleDescription(error.textContent);
      expect(control).toHaveAttribute("aria-invalid", "true");
    }
    expect(accessDataMock.changeUserPassword).not.toHaveBeenCalled();
  });
  it("names every create-user control through its visible label", async () => {
    renderUsers();
    fireEvent.click(screen.getByRole("button", { name: /add user/i }));
    const editor = await screen.findByTestId("entity-editor-page");
    for (const label of [/^username/i, /^email/i, /^password/i, /^confirm password/i]) {
      expect(within(editor).getByLabelText(label)).toBeEnabled();
    }
    expect(within(editor).getByRole("combobox", { name: /status/i })).toBeEnabled();
    expect(within(editor).getByRole("combobox", { name: /auth.*source/i })).toBeEnabled();
  });
  it.each([/edit/i, /change password/i, /delete/i])("restores focus to row action %s on dismissal", async (action) => {
    listUsersMock.mockResolvedValue({ items: [{ id: "user-1", username: "alice", email: "alice@example.test", status: "active", authSource: "local", externalId: "" }], total: 1 });
    renderUsers();
    const trigger = await screen.findByRole("button", { name: "Actions: alice" });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: action }));
    const dialog = action.source === "edit" ? await screen.findByTestId("entity-editor-page") : await screen.findByRole(action.source === "delete" ? "alertdialog" : "dialog");
    if (action.source === "delete" || action.source === "edit") {
      fireEvent.click(within(dialog).getByRole("button", { name: /cancel/i }));
    } else {
      fireEvent.keyDown(dialog, { key: "Escape", code: "Escape" });
    }
    await waitFor(() => expect(trigger).toHaveFocus(), { timeout: 1000 });
    expect(accessDataMock.deleteUser).not.toHaveBeenCalled();
    expect(accessDataMock.updateUser).not.toHaveBeenCalled();
    expect(accessDataMock.changeUserPassword).not.toHaveBeenCalled();
  });

  it("restores focus to the create trigger after returning from the page", async () => {
    renderUsers();
    const trigger = screen.getByRole("button", { name: /add user/i });
    trigger.focus();
    fireEvent.click(trigger);
    const dialog = await screen.findByTestId("entity-editor-page");
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    fireEvent.click(within(dialog).getByRole("button", { name: /cancel/i }));
    await waitFor(() => expect(screen.queryByTestId("entity-editor-page")).not.toBeInTheDocument());
    await waitFor(() => expect(trigger).toHaveFocus(), { timeout: 1000 });
  });

  it("renders an honest placeholder on the create-user password fields", async () => {
    renderUsers();

    // Open the create-user dialog from the toolbar trigger.
    fireEvent.click(screen.getByRole("button", { name: /add user/i }));
    const dialog = await screen.findByTestId("entity-editor-page");

    // `accessControl.users.passwordPlaceholder` is handed straight to the
    // shared Input's native `placeholder` attribute, so this query matches the
    // rendered DOM attribute (not the i18n object). The create dialog carries
    // two local-auth password fields (password + confirmation).
    const passwordInputs = within(dialog).getAllByPlaceholderText("Password");
    expect(passwordInputs).toHaveLength(2);
    for (const input of passwordInputs) {
      expect(input).toHaveAttribute("placeholder", "Password");
      expect(input).toHaveAttribute("type", "password");
    }

    // The literal-dots placeholder must never reach the DOM.
    expect(within(dialog).queryByPlaceholderText("••••••••")).toBeNull();
  });
});
