import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import { ApiError } from "@/shared/api/client";
import { AllPermissions } from "@/test/permissions";
import UserSettings from "./UserSettings";
import UserCredentialEditor from "./UserCredentialEditor";
import UserTokenEditor from "./UserTokenEditor";

const api = vi.hoisted(() => ({
  me: vi.fn(),
  listMyTokens: vi.fn(),
  listTokenScopes: vi.fn(),
  createMyToken: vi.fn(),
  revokeMyToken: vi.fn(),
  listMyCredentials: vi.fn(),
  createMyCredential: vi.fn(),
  updateMyCredential: vi.fn(),
  deleteMyCredential: vi.fn(),
  setDefaultMyCredential: vi.fn(),
  changeMyPassword: vi.fn(),
  copy: vi.fn(),
}));

vi.mock("@/shared/lib/clipboard", () => ({ writeBrowserClipboardText: api.copy }));

vi.mock("@/features/auth/api", async (original) => ({ ...(await original()), me: api.me }));
vi.mock("@/features/user-settings/api", async (original) => ({
  ...(await original()),
  listMyTokens: api.listMyTokens,
  listTokenScopes: api.listTokenScopes,
  createMyToken: api.createMyToken,
  revokeMyToken: api.revokeMyToken,
  listMyCredentials: api.listMyCredentials,
  createMyCredential: api.createMyCredential,
  updateMyCredential: api.updateMyCredential,
  deleteMyCredential: api.deleteMyCredential,
  setDefaultMyCredential: api.setDefaultMyCredential,
  changeMyPassword: api.changeMyPassword,
}));

function Location() {
  const location = useLocation();
  const navigate = useNavigate();
  return (
    <>
      <output data-testid="location">{location.pathname}{location.search}{location.hash}</output>
      <button onClick={() => void navigate("/user/settings?section=tokens&context=fixture#account")}>Navigate to tokens</button>
      <button onClick={() => void navigate(-1)}>History back</button>
      <button onClick={() => void navigate(1)}>History forward</button>
    </>
  );
}

function renderPages(path: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return { client, ...render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path="/user/settings" element={<UserSettings />} />
            <Route path="/user/settings/tokens/new" element={<UserTokenEditor />} />
            <Route path="/user/settings/credentials/new" element={<UserCredentialEditor />} />
            <Route path="/user/settings/credentials/:id/edit" element={<UserCredentialEditor />} />
          </Routes>
          <Location />
        </MemoryRouter>
      </AllPermissions>
    </QueryClientProvider>,
  ) };
}

describe("user settings routed editors", () => {
  it("restores the account section and its actions from URL history", async () => {
    renderPages("/user/settings?context=fixture#account");
    await waitFor(() => expect(api.me).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Navigate to tokens" }));
    expect(screen.queryByRole("link", { name: /create.*token/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "History back" }));
    expect(screen.queryByRole("link", { name: /create.*token/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "History forward" }));
    expect(screen.queryByRole("link", { name: /create.*token/i })).toBeInTheDocument();
  });

  it("keeps unrelated query/hash context when following section links", () => {
    renderPages("/user/settings?context=fixture#account");
    const tokens = screen.getByRole("link", { name: /api tokens/i });
    expect(tokens).toHaveAttribute("href", "/user/settings?context=fixture&section=tokens#account");
    fireEvent.click(tokens);
    expect(screen.getByTestId("location")).toHaveTextContent("/user/settings?context=fixture&section=tokens#account");
    expect(tokens).toHaveAttribute("aria-current", "page");
    fireEvent.click(screen.getByRole("link", { name: /profile/i }));
    expect(screen.getByTestId("location")).toHaveTextContent("/user/settings?context=fixture#account");
    expect(screen.getByRole("link", { name: /profile/i })).toHaveAttribute("aria-current", "page");
  });

  it("PWA-116 validates in field order, rechecks confirmation and clears success", async () => {
    api.me.mockResolvedValue({ username: "fixture", authSource: "local", roles: [] });
    api.changeMyPassword.mockResolvedValue(undefined);
    renderPages("/user/settings");
    const current = await screen.findByLabelText(/Current password/);
    const next = screen.getByLabelText(/^New password/);
    const confirm = screen.getByLabelText(/Confirm password/);
    const submit = screen.getByRole("button", { name: "Change password" });
    fireEvent.change(current, { target: { value: "synthetic-current" } });
    fireEvent.change(next, { target: { value: "short" } });
    fireEvent.click(submit);
    expect(next).toHaveFocus();
    expect(next).toHaveAccessibleDescription("Min 8 characters");
    fireEvent.change(next, { target: { value: "synthetic-next" } });
    fireEvent.change(confirm, { target: { value: "different-fixture" } });
    fireEvent.click(submit);
    expect(confirm).toHaveFocus();
    expect(confirm).toHaveAttribute("aria-invalid", "true");
    fireEvent.change(confirm, { target: { value: "synthetic-next" } });
    expect(confirm).not.toHaveAttribute("aria-invalid", "true");
    fireEvent.change(next, { target: { value: "synthetic-changed" } });
    expect(confirm).toHaveAttribute("aria-invalid", "true");
    fireEvent.change(confirm, { target: { value: "synthetic-changed" } });
    fireEvent.submit(submit.closest("form")!);
    await waitFor(() => expect(api.changeMyPassword).toHaveBeenCalledTimes(1));
    expect(api.changeMyPassword).toHaveBeenCalledWith({ currentPassword: "synthetic-current", newPassword: "synthetic-changed" });
    await waitFor(() => expect(current).toHaveValue(""));
    expect(next).toHaveValue("");
    expect(confirm).toHaveValue("");
    expect(current).not.toHaveAttribute("aria-invalid", "true");
  });

  it.each([400, 500])("PWA-116 blocks duplicate submissions and hides raw %s errors", async (status) => {
    api.me.mockResolvedValue({ username: "fixture", authSource: "local", roles: [] });
    let reject!: (reason: Error) => void;
    api.changeMyPassword.mockReturnValue(new Promise((_, fail) => { reject = fail; }));
    renderPages("/user/settings");
    const current = await screen.findByLabelText(/Current password/);
    const next = screen.getByLabelText(/^New password/);
    const confirm = screen.getByLabelText(/Confirm password/);
    fireEvent.change(current, { target: { value: "synthetic-current" } });
    fireEvent.change(next, { target: { value: "synthetic-next" } });
    fireEvent.change(confirm, { target: { value: "synthetic-next" } });
    const form = current.closest("form")!;
    fireEvent.submit(form);
    fireEvent.submit(form);
    await waitFor(() => expect(api.changeMyPassword).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("button", { name: "Changing password…" })).toBeDisabled();
    expect(current).toBeDisabled();
    act(() => reject(new ApiError({ status, message: "internal fixture diagnostic" })));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not change password");
    expect(screen.queryByText("internal fixture diagnostic")).not.toBeInTheDocument();
    expect(current).toBeEnabled();
    expect(current).not.toHaveAttribute("aria-invalid", "true");
  });

  it("PWA-116 hides the local password form for external authentication", async () => {
    api.me.mockResolvedValue({ username: "fixture", authSource: "oidc", roles: [] });
    renderPages("/user/settings");
    await screen.findByText(i18n.t("userSettings.externalPasswordHint"));
    expect(screen.queryByLabelText(/Current password/)).not.toBeInTheDocument();
  });
  it("PWA-116 associates required errors and focuses the first invalid password", async () => {
    api.me.mockResolvedValue({ username: "fixture", authSource: "local", roles: [] });
    renderPages("/user/settings");
    const current = await screen.findByLabelText(/Current password/);
    fireEvent.click(screen.getByRole("button", { name: "Change password" }));
    expect(current).toHaveFocus();
    expect(current).toHaveAttribute("aria-invalid", "true");
    expect(current).toHaveAccessibleDescription("Enter your current password");
    expect(api.changeMyPassword).not.toHaveBeenCalled();
  });
  beforeEach(() => {
    vi.clearAllMocks();
    api.me.mockResolvedValue({ username: "admin", email: "admin@example.test", roles: [], isAdmin: true });
    api.listMyTokens.mockResolvedValue({ items: [], total: 0, limit: 25, offset: 0 });
    api.listTokenScopes.mockResolvedValue([]);
    api.listMyCredentials.mockResolvedValue({ items: [], total: 0, limit: 25, offset: 0 });
  });

  it.each([
    ["edit", "loading"], ["edit", "error"], ["edit", "missing"],
    ["create", "loading"], ["create", "error"],
  ])("keeps the credential %s heading and Cancel while %s", async (mode, state) => {
    let resolve!: (value: unknown) => void;
    const deferred = new Promise(value => { resolve = value; });
    if (state === "loading") api.listMyCredentials.mockReturnValue(deferred);
    else if (state === "error") api.listMyCredentials.mockRejectedValue(new Error("fixture unavailable"));
    else api.listMyCredentials.mockResolvedValue([]);
    renderPages(mode === "create" ? "/user/settings/credentials/new" : "/user/settings/credentials/missing/edit");
    if (state === "loading") await waitFor(() => expect(api.listMyCredentials).toHaveBeenCalled());
    else await screen.findByText(i18n.t(state === "error" ? "userSettings.loadFailed" : "userSettings.credentialNotFound"));
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 1, name: i18n.t(mode === "edit" ? "userSettings.editCredential" : "userSettings.addCredential") })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^(Save|Create)$/ })).not.toBeInTheDocument();
    const back = screen.getByRole("button", { name: i18n.t("back") });
    expect(back).toBeEnabled();
    if (state === "error") expect(screen.getByRole("button", { name: i18n.t("retry") })).toBeEnabled();
    fireEvent.click(state === "error" ? back : screen.getByRole("button", { name: i18n.t("cancel") }));
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent("/user/settings?section=credentials"));
    await act(async () => { resolve([]); await deferred; });
  });

  it("routes the ?create=token deep link to the token editor page", async () => {
    renderPages("/user/settings?create=token");

    expect(await screen.findByRole("heading", { name: "Create API token" })).toBeInTheDocument();
    expect(screen.getByTestId("location")).toHaveTextContent("/user/settings/tokens/new");
  });
  it("keeps account return context through credential creation and cancel", async () => {
    api.listMyCredentials.mockResolvedValue([]);
    renderPages("/user/settings?section=credentials&context=fixture#account");
    fireEvent.click(await screen.findByRole("link", { name: /add credential/i }));
    await screen.findByLabelText(/^Name/);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByTestId("location").textContent).toBe("/user/settings?context=fixture&section=credentials#account");
  });

  it("focuses the invalid credential name and associates its inline error", async () => {
    api.listMyCredentials.mockResolvedValue([]);
    renderPages("/user/settings/credentials/new");
    const name = await screen.findByLabelText(/^Name/);
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(name).toHaveFocus();
    expect(name).toHaveAttribute("aria-invalid", "true");
    expect(name).toHaveAccessibleDescription(i18n.t("userSettings.nameRequired"));
    expect(api.createMyCredential).not.toHaveBeenCalled();
  });

  it("omits blank write-only secrets when editing a credential", async () => {
    api.listMyCredentials.mockResolvedValue([{ id: "cred-1", name: "Operations", protocol: "SSH", username: "operator", isDefault: false }]);
    api.updateMyCredential.mockResolvedValue(undefined);
    renderPages("/user/settings/credentials/cred-1/edit");
    const name = await screen.findByLabelText(/^Name/);
    const secret = screen.getByLabelText(/^Password/);
    expect(secret).toHaveAttribute("type", "password");
    expect(secret).toHaveValue("");
    expect(screen.getByLabelText(/Private key/)).toHaveValue("");
    fireEvent.change(name, { target: { value: "Operations updated" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(api.updateMyCredential).toHaveBeenCalledTimes(1));
    const body = api.updateMyCredential.mock.calls[0]?.[1];
    expect(body).toBeDefined();
    expect(body.password).toBeUndefined();
    expect(body.privateKey).toBeUndefined();
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent("/user/settings?section=credentials"));
  });

  it("keeps failed credential edits dirty without rendering raw backend errors", async () => {
    api.listMyCredentials.mockResolvedValue([]);
    api.createMyCredential.mockRejectedValue(new ApiError({ status: 500, message: "raw backend fixture" }));
    renderPages("/user/settings/credentials/new");
    fireEvent.change(await screen.findByLabelText(/^Name/), { target: { value: "fixture" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("userSettings.saveFailed"));
    expect(document.body.textContent?.includes("raw backend fixture")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
  });
  it("switches only the active section's creation action in the shared header", async () => {
    api.listMyCredentials.mockResolvedValue([]);
    renderPages("/user/settings?section=credentials");
    const credentialAction = await screen.findByRole("link", { name: /add credential/i });
    expect(credentialAction.closest('[data-slot="page-actions"]')).not.toBeNull();
    expect(screen.queryByRole("link", { name: /create.*token/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: /api tokens/i }));
    const tokenAction = await screen.findByRole("link", { name: /create.*token/i });
    expect(tokenAction.closest('[data-slot="page-actions"]')).not.toBeNull();
    expect(screen.queryByRole("link", { name: /add credential/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: /profile/i }));
    expect(screen.queryByRole("link", { name: /create.*token/i })).not.toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it("exposes credential commands and cancels removal with row focus restored", async () => {
    api.listMyCredentials.mockResolvedValue([{ id: "cred-1", name: "Operations", protocol: "SSH", username: "operator", isDefault: false }]);
    renderPages("/user/settings?section=credentials");
    const trigger = await screen.findByRole("button", { name: /actions: Operations/i });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    expect(await screen.findByRole("menuitem", { name: /^edit$/i })).toHaveAttribute("href", "/user/settings/credentials/cred-1/edit");
    expect(screen.getByRole("menuitem", { name: /set.*default/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: /^delete$/i }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^cancel$/i }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(api.deleteMyCredential).not.toHaveBeenCalled();
  });

  it("sets the selected credential default and confirms removal of that credential", async () => {
    api.listMyCredentials.mockResolvedValue([{ id: "cred-1", name: "Operations", protocol: "SSH", username: "operator", isDefault: false }]);
    api.setDefaultMyCredential.mockResolvedValue(undefined);
    api.deleteMyCredential.mockResolvedValue(undefined);
    renderPages("/user/settings?section=credentials");
    const trigger = await screen.findByRole("button", { name: /actions: Operations/i });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /set.*default/i }));
    await waitFor(() => expect(api.setDefaultMyCredential).toHaveBeenCalledWith("cred-1"));
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^remove$/i }));
    await waitFor(() => expect(api.deleteMyCredential).toHaveBeenCalledWith("cred-1"));
  });

  it("guards the token Cancel with the unsaved-changes prompt once the form is dirty", async () => {
    renderPages("/user/settings/tokens/new");

    const name = await screen.findByRole("textbox", { name: /^Token name/ });
    fireEvent.change(name, { target: { value: "ci-token" } });

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
  });

  it("uses inline token validation and focuses name then permissions", async () => {
    renderPages("/user/settings/tokens/new");
    const name = await screen.findByRole("textbox", { name: /Token name/ });
    expect(name).toHaveAccessibleDescription(i18n.t("userSettings.nameSectionHelper"));
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(name).toHaveFocus();
    expect(name).toHaveAccessibleDescription(i18n.t("userSettings.nameRequired"));
    fireEvent.change(name, { target: { value: " " } });
    expect(name).toHaveAccessibleDescription(i18n.t("userSettings.nameRequired"));
    fireEvent.change(name, { target: { value: "fixture" } });
    expect(name).toHaveAccessibleDescription(i18n.t("userSettings.nameSectionHelper"));
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    const permissions = screen.getByRole("group", { name: i18n.t("userSettings.permissionsSectionTitle") });
    expect(permissions).toHaveFocus();
    expect(permissions).toHaveAccessibleDescription(i18n.t("userSettings.scopeRequired"));
    expect(api.createMyToken).not.toHaveBeenCalled();
  });

  it.each(["close", "unmount"])("reveals a mock token once, excludes plaintext from cache and clears on %s", async (exit) => {
    api.me.mockResolvedValue({ username: "fixture", capabilities: [{ action: "READ", resourceType: "inventory" }] });
    api.listTokenScopes.mockResolvedValue([{ value: "read:inventory", action: "read", resourceType: "inventory", label: "read inventory" }]);
    const plaintext = "disposable-mock-only";
    api.createMyToken.mockResolvedValue({ id: "fixture-token", name: "fixture", scopes: "read:inventory@*", plaintext });
    const { client, unmount } = renderPages("/user/settings/tokens/new?context=fixture#account");
    fireEvent.change(await screen.findByRole("textbox", { name: /Token name/ }), { target: { value: "fixture" } });
    const permissions = screen.getByRole("group", { name: i18n.t("userSettings.permissionsSectionTitle") });
    const enabled = await waitFor(() => {
      const values = within(permissions).getAllByRole("checkbox").filter(node => !node.hasAttribute("disabled"));
      expect(values.length).toBeGreaterThan(0);
      return values;
    });
    const selectable = enabled[0];
    if (!selectable) throw new Error("Expected an available fixture scope");
    fireEvent.click(selectable);
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    const dialog = await screen.findByRole("dialog");
    // Boolean assertions avoid rendering even disposable plaintext in failure diffs.
    expect(dialog.textContent?.includes(plaintext)).toBe(true);
    expect(client.getMutationCache().getAll().some(mutation => JSON.stringify(mutation.state.data)?.includes(plaintext))).toBe(false);
    api.copy.mockRejectedValueOnce(new Error("fixture clipboard unavailable"));
    fireEvent.click(within(dialog).getByRole("button", { name: i18n.t("userSettings.tokenRevealCopy") }));
    await waitFor(() => expect(api.copy).toHaveBeenCalledTimes(1));
    expect(within(dialog).queryByRole("button", { name: i18n.t("userSettings.tokenRevealCopied") })).not.toBeInTheDocument();
    api.copy.mockResolvedValueOnce(undefined);
    fireEvent.click(within(dialog).getByRole("button", { name: i18n.t("userSettings.tokenRevealCopy") }));
    await within(dialog).findByRole("button", { name: i18n.t("userSettings.tokenRevealCopied") });
    expect(api.createMyToken).toHaveBeenCalledTimes(1);
    if (exit === "close") {
      fireEvent.click(within(dialog).getByRole("button", { name: i18n.t("userSettings.tokenRevealDone") }));
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      expect(screen.getByTestId("location").textContent).toBe("/user/settings?context=fixture&section=tokens#account");
      expect(document.body.textContent?.includes(plaintext)).toBe(false);
    }
    unmount();
    expect(document.body.textContent?.includes(plaintext)).toBe(false);
    expect(client.getMutationCache().getAll().some(mutation => JSON.stringify(mutation.state.data)?.includes(plaintext))).toBe(false);
  });

  it("does not grant unavailable token scopes even to an admin without capabilities", async () => {
    api.listTokenScopes.mockResolvedValue([{ value: "delete:inventory", action: "delete", resourceType: "inventory", label: "delete inventory" }]);
    renderPages("/user/settings/tokens/new");
    const permissions = screen.getByRole("group", { name: i18n.t("userSettings.permissionsSectionTitle") });
    const checkboxes = await within(permissions).findAllByRole("checkbox");
    expect(checkboxes.every(node => node.hasAttribute("disabled"))).toBe(true);
    expect(api.createMyToken).not.toHaveBeenCalled();
  });

  it("focuses and describes invalid custom token expiry without sending a request", async () => {
    api.me.mockResolvedValue({ username: "fixture", capabilities: [{ action: "READ", resourceType: "inventory" }] });
    api.listTokenScopes.mockResolvedValue([{ value: "read:inventory", action: "read", resourceType: "inventory", label: "read inventory" }]);
    renderPages("/user/settings/tokens/new");
    fireEvent.change(await screen.findByRole("textbox", { name: /Token name/ }), { target: { value: "fixture" } });
    const permissions = screen.getByRole("group", { name: i18n.t("userSettings.permissionsSectionTitle") });
    const selectable = await waitFor(() => {
      const checkbox = within(permissions).getAllByRole("checkbox").find(node => !node.hasAttribute("disabled"));
      if (!checkbox) throw new Error("Expected an available fixture scope");
      return checkbox;
    });
    fireEvent.click(selectable);
    fireEvent.click(screen.getByRole("button", { name: i18n.t("userSettings.expiryPresetCustom") }));
    const expiry = screen.getByLabelText(i18n.t("userSettings.expiryCustomSecondsLabel"));
    fireEvent.change(expiry, { target: { value: "-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(expiry).toHaveFocus();
    expect(expiry).toHaveAccessibleDescription(i18n.t("userSettings.expiresInSecondsInvalid"));
    expect(api.createMyToken).not.toHaveBeenCalled();
  });

  it("restores token row focus after cancelling revoke", async () => {
    api.listMyTokens.mockResolvedValue({ items: [{ id: "token-1", name: "Fixture token", scopes: "read:inventory", status: "active" }], total: 1, limit: 25, offset: 0 });
    renderPages("/user/settings?section=tokens");
    const trigger = await screen.findByRole("button", { name: /actions: Fixture token/i });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /^revoke$/i }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(api.revokeMyToken).not.toHaveBeenCalled();
  });

  it("returns to the tokens section after discarding the dirty token form", async () => {
    renderPages("/user/settings/tokens/new");

    const name = await screen.findByRole("textbox", { name: /^Token name/ });
    fireEvent.change(name, { target: { value: "ci-token" } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard" }));

    expect(screen.getByTestId("location")).toHaveTextContent("/user/settings?section=tokens");
  });

  it("names the account password controls and keeps confirmation validation", async () => {
    api.me.mockResolvedValue({ username: "admin", email: "admin@example.test", roles: [], authSource: "local" });
    api.listMyCredentials.mockResolvedValue([]);
    renderPages("/user/settings?section=profile");
    expect(await screen.findByLabelText(/^Current password/)).toHaveAttribute("type", "password");
    expect(screen.getByLabelText(/^New password/)).toHaveAttribute("type", "password");
    expect(screen.getByLabelText(/^Confirm password/)).toHaveAttribute("type", "password");
    fireEvent.click(screen.getByRole("button", { name: "Change password" }));
    expect(api.changeMyPassword).not.toHaveBeenCalled();
  });

  it("labels credential fields without reading stored secret values", async () => {
    api.listMyCredentials.mockResolvedValue([{ id: "cred-1", name: "Operations", protocol: "SSH", username: "operator", isDefault: true }]);
    renderPages("/user/settings/credentials/cred-1/edit");
    const name = await screen.findByLabelText(/^Name/);
    expect(screen.getByLabelText("Username")).toHaveValue("operator");
    expect(screen.getByLabelText("Password")).toHaveValue("");
    expect(screen.getByLabelText(/Private key/)).toHaveValue("");
    fireEvent.change(name, { target: { value: "Operations updated" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(api.updateMyCredential).toHaveBeenCalledWith("cred-1", {
      name: "Operations updated", protocol: "SSH", username: "operator", domain: undefined,
      password: undefined, privateKey: undefined,
    }));
  });
});
