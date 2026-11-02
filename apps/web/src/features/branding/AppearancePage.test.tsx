import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { BRANDING_QUERY_KEY } from "@/shared/api/branding-api";
import { derivePalette, hexToHsl } from "@/shared/theme/palette";
import AppearancePage from "./AppearancePage";
import type * as BrandingApi from "@/shared/api/branding-api";
import type * as ReactI18next from "react-i18next";

const api = vi.hoisted(() => ({ getBranding: vi.fn(), updateBranding: vi.fn() }));
vi.mock("@/shared/api/branding-api", async (original) => ({
  ...(await original<typeof BrandingApi>()), ...api,
}));
vi.mock("react-i18next", async (original) => ({
  ...(await original<typeof ReactI18next>()),
  useTranslation: () => ({ t: (key: string, options?: { defaultValue?: string } | string) =>
    typeof options === "object" ? options.defaultValue ?? key : key }),
}));
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));

const saved = {
  app_name: "Saved console", logo_data_url: null, favicon_data_url: null,
  theme: { mode: "custom", base: { h: 211, s: 90, l: 54 } },
};
function renderPage(branding = saved, update = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(BRANDING_QUERY_KEY, branding);
  return render(<QueryClientProvider client={client}><PermissionsContext.Provider value={{
    roles: [], isAdmin: false, can: action => action === "READ" || update, canAny: () => false, hasRole: () => false,
  }}><AppearancePage /></PermissionsContext.Provider></QueryClientProvider>);
}

describe("AppearancePage local previews", () => {
  it("associates app-name help with the editable input", () => {
    renderPage();
    const input = screen.getByRole("textbox", { name: "appearance.appName" });
    expect(input).toHaveAccessibleDescription("appearance.appNameHint");
    const descriptionId = input.getAttribute("aria-describedby");
    input.focus();
    fireEvent.change(input, { target: { value: "Draft console" } });
    expect(input).toHaveFocus();
    expect(input).toHaveValue("Draft console");
    expect(input).toHaveAttribute("aria-describedby", descriptionId);
  });
  beforeEach(() => { vi.clearAllMocks(); api.getBranding.mockResolvedValue(saved); api.updateBranding.mockResolvedValue(saved); });
  afterEach(() => { document.getElementById("wl-palette-overrides")?.remove(); document.documentElement.classList.remove("dark"); });

  it("never changes the host palette while editing or unmounting without Save", () => {
    document.documentElement.classList.add("dark");
    const style = document.createElement("style");
    style.id = "wl-palette-overrides";
    style.textContent = ":root { --primary: 123 45% 67%; }";
    document.head.append(style);
    const { unmount } = renderPage();
    fireEvent.change(screen.getByPlaceholderText("appearance.appNamePlaceholder"), { target: { value: "Unsaved console" } });
    fireEvent.change(screen.getByLabelText("appearance.customColor"), { target: { value: "#c026d3" } });
    expect(style.textContent).toBe(":root { --primary: 123 45% 67%; }");
    unmount();
    expect(style.textContent).toBe(":root { --primary: 123 45% 67%; }");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(api.updateBranding).not.toHaveBeenCalled();
  });

  it("updates both previews immediately from drafts and resets only their accents", () => {
    renderPage();
    const light = screen.getByRole("region", { name: "Light preview" });
    const dark = screen.getByRole("region", { name: "Dark preview" });
    fireEvent.change(screen.getByPlaceholderText("appearance.appNamePlaceholder"), { target: { value: "Draft console" } });
    fireEvent.change(screen.getByLabelText("appearance.customColor"), { target: { value: "#c026d3" } });
    const base = hexToHsl("#c026d3")!;
    for (const [preview, mode] of [[light, "light"], [dark, "dark"]] as const) {
      expect(within(preview).getByText("Draft console")).toBeInTheDocument();
      expect(preview.style.getPropertyValue("--primary")).toBe(derivePalette(base)[mode]["--primary"]);
      expect(preview.style.getPropertyValue("--primary-fill")).toBe(derivePalette(base)[mode]["--primary-fill"]);
      expect(preview.style.getPropertyValue("--background")).toBe("");
      expect(within(preview).getByRole("button", { name: "Unavailable" })).toBeDisabled();
      fireEvent.click(within(preview).getByRole("button", { name: "Add resource" }));
    }
    expect(api.updateBranding).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "appearance.resetToDefault" }));
    expect(light.style.getPropertyValue("--primary")).toBe("");
    expect(dark.style.getPropertyValue("--primary")).toBe("");
    expect(screen.getByRole("button", { name: "appearance.presets.amber" })).toHaveAttribute("aria-pressed", "false");
    expect(within(light).getByText("Draft console")).toBeInTheDocument();
  });

  it("shows an uploaded draft logo in both previews without saving", async () => {
    const { container } = renderPage();
    const file = new File(["<svg xmlns='http://www.w3.org/2000/svg'/>"], "logo.svg", { type: "image/svg+xml" });
    fireEvent.change(container.querySelector('input[type="file"]')!, { target: { files: [file] } });
    await waitFor(() => {
      for (const name of ["Light preview", "Dark preview"]) {
        expect(within(screen.getByRole("region", { name })).getByRole("img", { name: "appearance.logo" }).getAttribute("src")).toMatch(/^data:image\/svg\+xml;base64,/);
      }
    });
    expect(api.updateBranding).not.toHaveBeenCalled();
  });

  it("retains the Save payload contract for custom and default palettes", async () => {
    renderPage();
    fireEvent.change(screen.getByPlaceholderText("appearance.appNamePlaceholder"), { target: { value: " Draft console " } });
    fireEvent.change(screen.getByLabelText("appearance.customColor"), { target: { value: "#c026d3" } });
    fireEvent.click(screen.getByRole("button", { name: "save" }));
    await waitFor(() => expect(api.updateBranding.mock.calls[0]?.[0]).toEqual({
      app_name: "Draft console", logo_data_url: null, favicon_data_url: null,
      theme: { mode: "custom", base: hexToHsl("#c026d3") },
    }));
    await waitFor(() => expect(screen.getByRole("button", { name: "save" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "appearance.resetToDefault" }));
    fireEvent.click(screen.getByRole("button", { name: "save" }));
    await waitFor(() => expect(api.updateBranding.mock.calls[1]?.[0]).toEqual({
      app_name: "Draft console", logo_data_url: null, favicon_data_url: null, theme: { mode: "default" },
    }));
  });

  it("does not turn an invalid custom color into a saved default palette", () => {
    renderPage();
    const hex = screen.getByPlaceholderText("#3b82f6");
    fireEvent.change(hex, { target: { value: "#invalid" } });
    expect(screen.getByRole("button", { name: "save" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "save" }));
    expect(api.updateBranding).not.toHaveBeenCalled();
    fireEvent.change(hex, { target: { value: "#0d9488" } });
    expect(screen.getByRole("button", { name: "save" })).not.toBeDisabled();
  });

  it("guards unsaved branding edits with the shared beforeunload prompt", () => {
    renderPage();
    const preventDefault = vi.fn();
    const event = new Event("beforeunload", { cancelable: true });
    event.preventDefault = preventDefault;

    window.dispatchEvent(event);
    expect(preventDefault).not.toHaveBeenCalled();

    fireEvent.change(screen.getByPlaceholderText("appearance.appNamePlaceholder"), {
      target: { value: "Unsaved console" },
    });

    window.dispatchEvent(event);
    expect(preventDefault).toHaveBeenCalledTimes(1);
  });

  it("clears the unsaved guard after a successful save, even with surrounding whitespace", async () => {
    renderPage();
    const preventDefault = vi.fn();
    const event = new Event("beforeunload", { cancelable: true });
    event.preventDefault = preventDefault;

    fireEvent.change(screen.getByPlaceholderText("appearance.appNamePlaceholder"), {
      target: { value: " Draft console " },
    });
    window.dispatchEvent(event);
    expect(preventDefault).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "save" }));
    await waitFor(() => expect(api.updateBranding).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "save" })).not.toBeDisabled(),
    );

    preventDefault.mockClear();
    window.dispatchEvent(event);
    expect(preventDefault).not.toHaveBeenCalled();
  });

  it("labels the name field and exposes exactly one page heading", () => {
    renderPage();
    expect(screen.getByRole("textbox", { name: "appearance.appName" })).toHaveValue("Saved console");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("button", { name: "save" })).toBeEnabled();
  });

  it("preserves stored theme options when saving or resetting the accent", async () => {
    const branding = { ...saved, theme: { ...saved.theme, uiScale: 1.1, overrides: { light: { "--primary": "211 90% 54%" } } } };
    renderPage(branding);
    fireEvent.click(screen.getByRole("button", { name: "appearance.resetToDefault" }));
    fireEvent.click(screen.getByRole("button", { name: "save" }));
    await waitFor(() => expect(api.updateBranding).toHaveBeenCalledWith(expect.objectContaining({
      theme: { mode: "default", uiScale: 1.1, overrides: branding.theme.overrides },
    }), expect.anything()));
  });

  it("locks draft controls while saving and retains them when the request fails", async () => {
    let reject!: (error: Error) => void;
    api.updateBranding.mockImplementation(() => new Promise((_, fail) => { reject = fail; }));
    renderPage();
    const name = screen.getByRole("textbox", { name: "appearance.appName" });
    fireEvent.change(name, { target: { value: "Draft console" } });
    fireEvent.click(screen.getByRole("button", { name: "save" }));
    await waitFor(() => expect(name).toBeDisabled());
    expect(screen.getByRole("button", { name: "appearance.resetToDefault" })).toBeDisabled();
    reject(new Error("Mock save failure"));
    await waitFor(() => expect(name).toBeEnabled());
    expect(name).toHaveValue("Draft console");
  });

  it("keeps branding READ-only without save, reset or active draft controls", () => {
    renderPage(saved, false);
    expect(screen.getByRole("textbox", { name: "appearance.appName" })).toBeDisabled();
    expect(screen.getByLabelText("appearance.customColor")).toBeDisabled();
    expect(screen.queryByRole("button", { name: "save" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "appearance.resetToDefault" })).not.toBeInTheDocument();
    expect(api.updateBranding).not.toHaveBeenCalled();
  });
});
