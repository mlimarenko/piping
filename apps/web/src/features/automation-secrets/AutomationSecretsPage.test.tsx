import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import AutomationSecretsPage from "./AutomationSecretsPage";
import type * as SecretsApi from "@/features/automation-secrets/api";
import type { AutomationSecret } from "./api";
import { PermissionsContext } from "@/shared/auth/permissionsContext";

const apiMock = vi.hoisted(() => ({
  listAutomationSecrets: vi.fn(),
  createAutomationSecret: vi.fn(),
  updateAutomationSecret: vi.fn(),
  deleteAutomationSecret: vi.fn(),
  setAutomationSecretValue: vi.fn(),
  clearAutomationSecretValue: vi.fn(),
}));

vi.mock("@/features/automation-secrets/api", async (importOriginal) => ({
  ...(await importOriginal<typeof SecretsApi>()),
  ...apiMock,
}));

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({ notify: notifyMock }));

const secret = (over: Partial<AutomationSecret>): AutomationSecret => ({
  id: "s1",
  key: "grad-token",
  description: "GRAD API token",
  has_value: true,
  is_masked: true,
  usage_count: 2,
  created_at: "2026-07-01T00:00:00Z",
  ...over,
});

function renderPage(allowed?: string[]) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <PermissionsContext.Provider value={{ roles: [], isAdmin: !allowed, can: (action) => !allowed || allowed.includes(action), canAny: () => true, hasRole: () => false }}>
        <AutomationSecretsPage />
      </PermissionsContext.Provider>
    </QueryClientProvider>,
  );
}

describe("AutomationSecretsPage", () => {
  beforeEach(() => {
    Object.values(apiMock).forEach((m) => m.mockReset());
    notifyMock.mockReset();
    apiMock.listAutomationSecrets.mockResolvedValue([
      secret({}),
      secret({ id: "s2", key: "ext-key", description: null, has_value: false, usage_count: 0 }),
    ]);
  });

  it("lists named secrets with value state and usage", async () => {
    renderPage();
    expect(await screen.findByText("grad-token")).toBeInTheDocument();
    expect(screen.getByText("ext-key")).toBeInTheDocument();
    // has_value chips: one Set, one Not set.
    expect(screen.getByText(/^set$/i)).toBeInTheDocument();
    expect(screen.getByText(/^not set$/i)).toBeInTheDocument();
  });

  it("rejects a reserved key before calling the API", async () => {
    renderPage();
    await screen.findByText("grad-token");

    fireEvent.click(screen.getByRole("button", { name: /new secret/i }));
    const dialog = await screen.findByTestId("entity-editor-page");
    fireEvent.change(within(dialog).getByPlaceholderText("grad-api-token"), {
      target: { value: "secret-field/x" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /create/i }));

    expect(apiMock.createAutomationSecret).not.toHaveBeenCalled();
    expect(within(dialog).getByText(/reserved/i)).toBeInTheDocument();
  });

  it("creates a secret with a valid key", async () => {
    apiMock.createAutomationSecret.mockResolvedValue(secret({ id: "s3", key: "new-key" }));
    renderPage();
    await screen.findByText("grad-token");

    fireEvent.click(screen.getByRole("button", { name: /new secret/i }));
    const dialog = await screen.findByTestId("entity-editor-page");
    fireEvent.change(within(dialog).getByPlaceholderText("grad-api-token"), {
      target: { value: "new-key" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /create/i }));

    await waitFor(() =>
      expect(apiMock.createAutomationSecret).toHaveBeenCalledWith({
        key: "new-key",
        description: null,
      }),
    );
  });

  it("locks metadata submission, retains a failed draft, and guards discard", async () => {
    let fail!: (error: Error) => void;
    apiMock.createAutomationSecret.mockImplementation(() => new Promise((_resolve, reject) => { fail = reject; }));
    renderPage();
    await screen.findByText("grad-token");
    fireEvent.click(screen.getByRole("button", { name: /new secret/i }));
    const editor = await screen.findByTestId("entity-editor-page");
    const key = within(editor).getByRole("textbox", { name: /^key/i });
    const description = within(editor).getByRole("textbox", { name: /description/i });
    fireEvent.change(key, { target: { value: "draft-key" } });
    fireEvent.change(description, { target: { value: "Draft description" } });
    fireEvent.click(within(editor).getByRole("button", { name: /^create$/i }));
    await waitFor(() => expect(key).toBeDisabled());
    expect(description).toBeDisabled();
    expect(within(editor).getByRole("button", { name: /^cancel$/i })).toBeDisabled();
    fail(new Error("Save failed"));
    await waitFor(() => expect(key).toBeEnabled());
    expect(key).toHaveValue("draft-key");
    expect(description).toHaveValue("Draft description");
    fireEvent.click(within(editor).getByRole("button", { name: /^cancel$/i }));
    const guard = await screen.findByRole("alertdialog");
    fireEvent.click(within(guard).getByRole("button", { name: /continue editing/i }));
    expect(key).toHaveValue("draft-key");
    fireEvent.click(within(editor).getByRole("button", { name: /^cancel$/i }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^discard/i }));
    await waitFor(() => expect(screen.queryByTestId("entity-editor-page")).not.toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: /new secret/i }));
    expect(await screen.findByRole("textbox", { name: /^key/i })).toHaveValue("");
    expect(apiMock.createAutomationSecret).toHaveBeenCalledTimes(1);
  });

  it("explains a 409 delete conflict as in-use", async () => {
    apiMock.deleteAutomationSecret.mockRejectedValue(
      Object.assign(new Error("conflict"), { status: 409 }),
    );
    renderPage();
    await screen.findByText("grad-token");

    fireEvent.pointerDown(screen.getByRole("button", { name: /actions: grad-token/i }), { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
    const alert = await screen.findByRole("alertdialog");
    fireEvent.click(within(alert).getByRole("button", { name: /^delete$/i }));

    await waitFor(() =>
      expect(notifyMock).toHaveBeenCalledWith(
        "error",
        expect.stringMatching(/still used by a playbook/i),
      ),
    );
  });

  it("edits only metadata without reading, clearing, or writing the stored value", async () => {
    apiMock.updateAutomationSecret.mockResolvedValue(secret({ description: "Changed description" }));
    renderPage();
    await screen.findByText("grad-token");
    fireEvent.pointerDown(screen.getByRole("button", { name: /actions: grad-token/i }), { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /^edit$/i }));
    const editor = await screen.findByTestId("entity-editor-page");
    expect(within(editor).queryByLabelText(/new value/i)).not.toBeInTheDocument();
    fireEvent.change(within(editor).getByRole("textbox", { name: /description/i }), { target: { value: "Changed description" } });
    fireEvent.click(within(editor).getByRole("button", { name: /save changes/i }));
    await waitFor(() => expect(apiMock.updateAutomationSecret).toHaveBeenCalledWith("s1", { description: "Changed description" }));
    expect(apiMock.setAutomationSecretValue).not.toHaveBeenCalled();
    expect(apiMock.clearAutomationSecretValue).not.toHaveBeenCalled();
  });
  it("cancels a row deletion without writing and returns focus to its menu", async () => {
    renderPage();
    const trigger = await screen.findByRole("button", { name: /actions: grad-token/i });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /^cancel$/i }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(apiMock.deleteAutomationSecret).not.toHaveBeenCalled();
  });

  it("opens the value command without conflating metadata and secret permissions", async () => {
    renderPage(["READ", "SECRET"]);
    fireEvent.pointerDown(await screen.findByRole("button", { name: /actions: grad-token/i }), { button: 0, ctrlKey: false });
    expect(screen.queryByRole("menuitem", { name: /^edit$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /^delete$/i })).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole("menuitem", { name: /set.*value/i }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    expect(apiMock.setAutomationSecretValue).not.toHaveBeenCalled();
  });

  it("shows no empty row trigger or create action for read-only access", async () => {
    renderPage(["READ"]);
    await screen.findByText("grad-token");
    expect(screen.queryByRole("button", { name: /actions:/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /new secret/i })).not.toBeInTheDocument();
  });

  it("returns from metadata editing to the invoking row menu without a write", async () => {
    renderPage();
    const trigger = await screen.findByRole("button", { name: /actions: grad-token/i });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /^edit$/i }));
    fireEvent.click(within(await screen.findByTestId("entity-editor-page")).getByRole("button", { name: /^cancel$/i }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(apiMock.updateAutomationSecret).not.toHaveBeenCalled();
  });
});
