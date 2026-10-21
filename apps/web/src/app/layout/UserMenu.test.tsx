import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, * as languages from "@/shared/i18n";
import type * as Settings from "@/shared/settings";
import { ThemeProvider } from "@/shared/hooks/use-theme";
import UserMenu from "./UserMenu";

const settings = vi.hoisted(() => ({ mode: "light", setTheme: vi.fn(), setLanguage: vi.fn() }));
const api = vi.hoisted(() => ({ me: vi.fn(), logout: vi.fn(), notify: vi.fn() }));
vi.mock("@/shared/settings", async original => ({
  ...await original<typeof Settings>(),
  useSetting: (key: string) => key === "app.theme"
    ? { value: { mode: settings.mode }, setValue: settings.setTheme }
    : { value: { lng: "en" }, setValue: settings.setLanguage },
}));
vi.mock("@/features/auth/api", () => ({ me: api.me, logout: api.logout }));
vi.mock("@/shared/lib/notifications", () => ({ notify: api.notify }));

function Location() { return <output data-testid="location">{useLocation().pathname}</output>; }
function renderMenu() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  client.setQueryData(["auth", "me"], { id: "operator-id", username: "operator", email: "operator@example.test" });
  client.setQueryData(["private-workspace"], "cached workspace");
  const view = render(<QueryClientProvider client={client}><ThemeProvider><MemoryRouter initialEntries={["/inventory/resources"]}><UserMenu /><Location /></MemoryRouter></ThemeProvider></QueryClientProvider>);
  return { ...view, client };
}
async function openMenu() {
  const trigger = screen.getByRole("button", { name: i18n.t("topBar.userMenu") });
  trigger.focus();
  fireEvent.keyDown(trigger, { key: "ArrowDown" });
  await screen.findByRole("menu");
  return trigger;
}

describe("UserMenu global controls", () => {
  beforeEach(async () => {
    vi.clearAllMocks(); settings.mode = "light";
    api.me.mockResolvedValue({ id: "operator-id", username: "operator" });
    await i18n.changeLanguage("en");
  });
  it("opens with the keyboard and restores trigger focus on Escape", async () => {
    renderMenu();
    const trigger = await openMenu();
    expect(screen.getByText("operator@example.test")).toBeVisible();
    await waitFor(() => expect(screen.getByRole("menuitem", { name: "Account" })).toHaveFocus());
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
  });
  it("navigates to account settings through the existing menu", async () => {
    renderMenu(); await openMenu();
    fireEvent.keyDown(screen.getByRole("menuitem", { name: "Account" }), { key: "Enter" });
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent("/user/settings"));
  });
  it("switches themes through the existing settings owner", async () => {
    renderMenu(); await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Toggle theme: Dark" }));
    expect(settings.setTheme).toHaveBeenCalledWith({ mode: "dark" });
  });
  it("switches back to light", async () => {
    settings.mode = "dark";
    renderMenu(); await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Toggle theme: Light" }));
    expect(settings.setTheme).toHaveBeenCalledWith({ mode: "light" });
  });
  it("follows system media changes without changing the saved preference", async () => {
    settings.mode = "system";
    let listener: (() => void) | undefined;
    const media = { matches: false, addEventListener: (_: string, callback: () => void) => { listener = callback; }, removeEventListener: vi.fn() };
    const matchMedia = vi.spyOn(window, "matchMedia").mockReturnValue(media as unknown as MediaQueryList);
    const view = renderMenu();
    try {
      await openMenu();
      expect(screen.getByRole("menuitem", { name: "Toggle theme: Dark" })).toBeVisible();
      act(() => { media.matches = true; listener?.(); });
      expect(document.documentElement).toHaveClass("dark");
      expect(screen.getByRole("menuitem", { name: "Toggle theme: Light" })).toBeVisible();
      expect(settings.setTheme).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole("menuitem", { name: "Toggle theme: Light" }));
      expect(settings.setTheme).toHaveBeenCalledWith({ mode: "light" });
    } finally { view.unmount(); view.client.clear(); matchMedia.mockRestore(); }
  });
  it("shows the chosen language, persists the next language and loads its dictionary", async () => {
    const change = vi.spyOn(languages, "changeAppLanguage").mockResolvedValue();
    try {
      renderMenu(); await openMenu();
      const language = screen.getByRole("menuitem", { name: "Language: English → Russian" });
      expect(language).toHaveTextContent("English");
      fireEvent.click(language);
      expect(settings.setLanguage).toHaveBeenCalledWith({ lng: "ru" });
      expect(change).toHaveBeenCalledWith("ru");
    } finally { change.mockRestore(); }
  });
  it("localizes preference labels and switches from Russian back to English", async () => {
    await i18n.changeLanguage("ru");
    const change = vi.spyOn(languages, "changeAppLanguage").mockResolvedValue();
    try {
      renderMenu(); await openMenu();
      const label = `${i18n.t("userSettings.language")}: ${i18n.t("language.russian")} → ${i18n.t("language.english")}`;
      fireEvent.click(screen.getByRole("menuitem", { name: label }));
      expect(settings.setLanguage).toHaveBeenCalledWith({ lng: "en" });
      expect(change).toHaveBeenCalledWith("en");
    } finally { change.mockRestore(); }
  });
  it("disables destructive sign-out while pending and clears private cache on success", async () => {
    let complete!: () => void;
    api.logout.mockImplementation(() => new Promise<void>(resolve => { complete = resolve; }));
    const { client } = renderMenu(); await openMenu();
    const signOut = screen.getByRole("menuitem", { name: "Sign out" });
    expect(signOut).toHaveAttribute("data-variant", "destructive");
    fireEvent.click(signOut);
    await waitFor(() => expect(signOut).toHaveAttribute("aria-disabled", "true"));
    fireEvent.click(signOut);
    expect(api.logout).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("location")).toHaveTextContent("/inventory/resources");
    await act(async () => { complete(); await Promise.resolve(); });
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent("/auth/login"));
    expect(client.getQueryData(["private-workspace"])).toBeUndefined();
    expect(api.notify).toHaveBeenCalledWith("info", "Sign out");
  });
  it("still clears local state and returns to login when server sign-out fails", async () => {
    api.logout.mockRejectedValue(new Error("Local fixture failure"));
    const { client } = renderMenu(); await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Sign out" }));
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent("/auth/login"));
    expect(client.getQueryData(["private-workspace"])).toBeUndefined();
  });
});
