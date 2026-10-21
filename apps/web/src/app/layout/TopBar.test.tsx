import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import type * as Settings from "@/shared/settings";
import { ThemeProvider } from "@/shared/hooks/use-theme";
import TopBar from "./TopBar";
import PageChrome from "@/shared/ui-shared/PageChrome";
import { PageChromeProvider } from "@/shared/ui-shared/pageChromeContext";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";

const settings = vi.hoisted(() => ({ mode: "light", setTheme: vi.fn(), setLanguage: vi.fn() }));
vi.mock("@/shared/settings", async original => ({
  ...await original<typeof Settings>(),
  useSetting: (key: string) => key === "app.theme"
    ? { value: { mode: settings.mode }, setValue: settings.setTheme }
    : { value: { lng: "en" }, setValue: settings.setLanguage },
}));
vi.mock("@/features/auth/api", () => ({ me: vi.fn().mockResolvedValue({ id: "test-user", username: "operator" }), logout: vi.fn() }));

const renderTopBar = (onToggle = vi.fn(), path = "/inventory/resources") => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["auth", "me"], { id: "test-user" });
  return render(<QueryClientProvider client={client}><ThemeProvider><MemoryRouter initialEntries={[path]}>
    <PageChromeProvider navigationToken="route"><TopBar showMobileToggle onToggleMobileSidebar={onToggle} /><PageChrome title="Feature-owned title" /></PageChromeProvider>
  </MemoryRouter></ThemeProvider></QueryClientProvider>);
};
describe("TopBar controls", () => {
  beforeEach(() => { vi.clearAllMocks(); settings.mode = "light"; });
  it("uses the routed feature title instead of guessing a catalogue title from the pathname", () => {
    renderTopBar(vi.fn(), "/connections/config/linux/edit");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Feature-owned title");
    expect(screen.queryByText("Connection Config")).toBeNull();
  });
  it("groups global preferences behind the single user menu trigger", () => {
    renderTopBar();
    expect(screen.getByRole("button", { name: "User menu" })).toBeInTheDocument();
    expect(screen.getAllByRole("button")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Language" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Toggle theme" })).toBeNull();
  });
  it("opens mobile navigation from an accessible button", () => {
    const toggle = vi.fn();
    renderTopBar(toggle);
    fireEvent.click(screen.getByRole("button", { name: "Toggle sidebar" }));
    expect(toggle).toHaveBeenCalledOnce();
  });
});

it("owns the sole routed title and actions, retaining its title and blocking page actions in an embedded editor", () => {
  function View() {
    const [open, setOpen] = useState(false);
    const [client] = useState(() => new QueryClient());
    return <QueryClientProvider client={client}><ThemeProvider><MemoryRouter><PageChromeProvider navigationToken="route"><TopBar showMobileToggle onToggleMobileSidebar={vi.fn()} />
      <main id="main-content"><PageChrome title="Resources" actions={<button onClick={() => setOpen(true)}>Create resource</button>} />
        <div>Resource list</div><EntityEditorHost open={open} onOpenChange={setOpen} title="New resource" footer={<button data-editor-cancel onClick={() => setOpen(false)}>Cancel</button>}><input aria-label="Name" /></EntityEditorHost>
      </main></PageChromeProvider></MemoryRouter></ThemeProvider></QueryClientProvider>;
  }
  render(<View />);
  const header = screen.getByRole("banner");
  expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Resources");
  expect(within(screen.getByRole("main")).queryByRole("heading", { level: 1 })).toBeNull();
  const action = within(header).getByRole("button", { name: "Create resource" });
  action.focus(); fireEvent.click(action);
  expect(action.closest("[inert]")).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 2, name: "New resource" })).toHaveFocus();
  expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Resources");
  expect(within(header).getByRole("button", { name: "User menu" }).closest("[inert]")).toBeNull();
  fireEvent.keyDown(within(header).getByRole("button", { name: "User menu" }), { key: "ArrowDown" });
  expect(screen.getByRole("menuitem", { name: /Toggle theme/ })).not.toHaveAttribute("aria-disabled", "true");
  fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(action.closest("[inert]")).toBeNull();
  expect(action).toHaveFocus();
});
