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

  it("keeps password reset unavailable for an OIDC user", async () => {
    listUsersMock.mockResolvedValue({ items: [{ id: "user-1", username: "alice", email: "alice@example.test", status: "active", authSource: "oidc", externalId: "" }], total: 1 });
    renderUsers();
    fireEvent.keyDown(await screen.findByRole("button", { name: "Actions: alice" }), { key: "Enter" });
    expect(await screen.findByRole("menuitem", { name: "Edit" })).toBeEnabled();
    expect(screen.queryByRole("menuitem", { name: /change password/i })).toBeNull();
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
