import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import WebhooksPage from "./WebhooksPage";

const api = vi.hoisted(() => ({
  listWebhooks: vi.fn(), getWebhookEventCatalog: vi.fn(), createWebhook: vi.fn(),
  updateWebhook: vi.fn(), deleteWebhook: vi.fn(), clearWebhookSecret: vi.fn(), testFireWebhook: vi.fn(),
}));
vi.mock("./api", () => api);
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));
const webhook = {
  id: "subscription", name: "Inventory listener", target_url: "https://example.test/events",
  secret_set: true, event_types: ["inventory.device.created"], enabled: true,
  conditions: null, last_status: null, last_fired_at: null, created_at: "2026-01-01T00:00:00Z",
};
function renderPage(actions = ["READ", "CREATE", "UPDATE", "DELETE"]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const tree = () => <QueryClientProvider client={client}><PermissionsContext.Provider value={{
    roles: [], isAdmin: false, can: action => actions.includes(action), canAny: () => false, hasRole: () => false,
  }}><WebhooksPage /></PermissionsContext.Provider></QueryClientProvider>;
  const view = render(tree());
  return { ...view, setActions: (next: string[]) => { actions = next; view.rerender(tree()); } };
}
async function openActions() {
  const trigger = await screen.findByRole("button", { name: `Actions: ${webhook.name}` });
  trigger.focus();
  fireEvent.keyDown(trigger, { key: "Enter" });
  return trigger;
}
describe("WebhooksPage mutation lifetime and access", () => {
  it("disables an open confirmation when delete permission is revoked", async () => {
    const view = renderPage(["READ", "DELETE"]);
    await openActions();
    fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
    const dialog = await screen.findByRole("alertdialog");
    view.setActions(["READ"]);
    await waitFor(() => expect(within(dialog).getByRole("button", { name: /^delete$/i })).toBeDisabled());
    fireEvent.click(within(dialog).getByRole("button", { name: /^delete$/i }));
    expect(api.deleteWebhook).not.toHaveBeenCalled();
  });
  beforeEach(() => {
    Object.values(api).forEach(mock => mock.mockReset());
    api.listWebhooks.mockResolvedValue({ items: [webhook], total: 1 });
    api.getWebhookEventCatalog.mockResolvedValue([]);
  });
  it("keeps deletion confirmation pending and retains it on failure", async () => {
    let reject!: (error: Error) => void;
    api.deleteWebhook.mockImplementation(() => new Promise((_, fail) => { reject = fail; }));
    renderPage();
    await openActions();
    fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
    const dialog = screen.getByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: /^delete$/i }));
    await waitFor(() => expect(api.deleteWebhook).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /cancel/i })).toBeDisabled();
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    await act(async () => { reject(new Error("Mock request failed")); await Promise.resolve(); });
    await waitFor(() => expect(within(dialog).getByRole("button", { name: /^delete$/i })).toBeEnabled());
    expect(api.deleteWebhook).toHaveBeenCalledTimes(1);
  });
  it("does not allow leaving or changing an editor during an update", async () => {
    let finish!: (value: typeof webhook) => void;
    api.updateWebhook.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    renderPage();
    await openActions();
    fireEvent.click(await screen.findByRole("menuitem", { name: /^edit$/i }));
    expect(screen.getByLabelText(/secret.*optional/i)).toHaveValue("");
    const name = screen.getByRole("textbox", { name: /name/i });
    fireEvent.change(name, { target: { value: "Updated listener" } });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(api.updateWebhook).toHaveBeenCalledTimes(1));
    expect(api.updateWebhook.mock.calls[0]![1]).not.toHaveProperty("secret");
    expect(name).toBeDisabled();
    expect(screen.getByRole("button", { name: /cancel/i })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /^back$/i }));
    expect(name).toBeInTheDocument();
    await act(async () => { finish(webhook); await Promise.resolve(); });
    await waitFor(() => expect(screen.queryByRole("button", { name: /^save$/i })).not.toBeInTheDocument());
  });
  it.each([["READ"], ["READ", "DELETE"]])("preserves action-specific permissions: %j", async (...actions) => {
    renderPage(actions);
    await screen.findByText(webhook.name);
    if (actions.includes("DELETE")) {
      await openActions();
      expect(await screen.findByRole("menuitem", { name: /^delete$/i })).toBeInTheDocument();
    } else {
      expect(screen.queryByRole("button", { name: `Actions: ${webhook.name}` })).not.toBeInTheDocument();
    }
    expect(screen.queryByRole("menuitem", { name: /^edit$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /test fire|clear secret/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /clear secret/i })).not.toBeInTheDocument();
    expect(api.updateWebhook).not.toHaveBeenCalled();
    expect(api.testFireWebhook).not.toHaveBeenCalled();
  });
  it("cancels deletion without mutation and restores focus to the menu trigger", async () => {
    renderPage();
    const trigger = await openActions();
    fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: /cancel/i }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(api.deleteWebhook).not.toHaveBeenCalled();
    await waitFor(() => expect(trigger).toHaveFocus());
  });
  it("cancels editing and restores focus to the row menu", async () => {
    renderPage();
    const trigger = await openActions();
    fireEvent.click(await screen.findByRole("menuitem", { name: /^edit$/i }));
    const editor = await screen.findByTestId("entity-editor-page");
    fireEvent.click(within(editor).getByRole("button", { name: /cancel/i }));
    await waitFor(() => expect(screen.queryByTestId("entity-editor-page")).toBeNull());
    expect(api.updateWebhook).not.toHaveBeenCalled();
    await waitFor(() => expect(trigger).toHaveFocus());
  });
});
