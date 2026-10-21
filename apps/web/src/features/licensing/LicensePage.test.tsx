import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import LicensePage from "./LicensePage";
import i18n from "@/shared/i18n";

const api = vi.hoisted(() => ({ getLicense: vi.fn(), getLicenseHealth: vi.fn(), activateLicense: vi.fn(), refreshLicense: vi.fn() }));
vi.mock("./api", () => api);
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));
const license = { status: "active", version: "1", seats: { used: 1, total: 10 } };
function renderPage(update = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><PermissionsContext.Provider value={{
    roles: [], isAdmin: false, can: action => action === "READ" || update, canAny: () => false, hasRole: () => false,
  }}><LicensePage /></PermissionsContext.Provider></QueryClientProvider>);
}
describe("LicensePage", () => {
  beforeEach(() => { vi.clearAllMocks(); api.getLicense.mockResolvedValue(license); api.getLicenseHealth.mockResolvedValue({ reasonCode: "OK" }); });
  it("shows a persisted issuer refusal after refresh fails, without claiming Community or activation success", async () => {
    api.refreshLicense.mockImplementation(() => {
      api.getLicense.mockResolvedValue({ ...license, status: "suspended" });
      api.getLicenseHealth.mockResolvedValue({ reasonCode: "TOKEN_REVOKED" });
      return Promise.reject(new Error("License refused"));
    });
    renderPage();
    await screen.findByText("1 / 10");
    fireEvent.click(screen.getByRole("button", { name: /^refresh$/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The activation token was revoked");
    expect(screen.getByText(/New seat allocations are blocked/)).toBeInTheDocument();
    expect(screen.queryByText(/Community/)).not.toBeInTheDocument();
    expect(screen.queryByText("Seat limit and validity period are applied.")).not.toBeInTheDocument();
  });
  it("serializes activation and refresh and preserves the draft after failure", async () => {
    let reject!: (error: Error) => void;
    api.activateLicense.mockImplementation(() => new Promise((_, fail) => { reject = fail; }));
    renderPage();
    fireEvent.click(screen.getByRole("button", { name: /^more seats$/i }));
    const field = screen.getByRole("textbox", { name: /license key/i });
    fireEvent.change(field, { target: { value: "mock-activation-input" } });
    fireEvent.click(screen.getByRole("button", { name: /^activate$/i }));
    await waitFor(() => expect(api.activateLicense).toHaveBeenCalledTimes(1));
    expect(field).toBeDisabled();
    expect(screen.getByRole("button", { name: /^refresh$/i })).toBeDisabled();
    await act(async () => { reject(new Error("Mock activation failed")); await Promise.resolve(); });
    await waitFor(() => expect(field).toBeEnabled());
    expect(field).toHaveValue("mock-activation-input");
    expect(api.refreshLicense).not.toHaveBeenCalled();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });
  it.each(["active", "inactive"])("shows %s state without activation or refresh for READ-only access", async status => {
    api.getLicense.mockResolvedValue({ ...license, status });
    renderPage(false);
    await screen.findByText("1 / 10");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /activate|refresh/i })).not.toBeInTheDocument();
    expect(api.activateLicense).not.toHaveBeenCalled();
  });
  it("does not claim Community while the state is loading", () => {
    api.getLicense.mockImplementation(() => new Promise(() => {}));
    renderPage();
    expect(screen.queryByText(/Community/)).not.toBeInTheDocument();
    expect(screen.queryByText(/\d+ \/ \d+/)).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });
  it.each(["en", "ru"])("shows inactive Community usage without activation in %s", async language => {
    await i18n.changeLanguage(language);
    api.getLicense.mockResolvedValue({ ...license, status: "inactive", seats: { used: 3, total: 10 } });
    renderPage();
    await screen.findByText("3 / 10");
    expect(screen.getByText(i18n.t("licensePage.freeLicenseLabel"))).toHaveTextContent("Community");
    expect(screen.getByText(i18n.t("licensePage.freeLicenseHint"))).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(api.activateLicense).not.toHaveBeenCalled();
    await i18n.changeLanguage("en");
  });
  it("does not present a load failure as Community", async () => {
    api.getLicense.mockRejectedValue(new Error("internal private diagnostic"));
    renderPage();
    expect(await screen.findByRole("alert")).toHaveTextContent("Failed to load license status");
    expect(screen.queryByText(/Community/)).not.toBeInTheDocument();
    expect(screen.queryByText("internal private diagnostic")).not.toBeInTheDocument();
  });
  it.each(["missing", "unknown"])("does not present %s as Community", async status => {
    api.getLicense.mockResolvedValue({ ...license, status });
    renderPage();
    expect(await screen.findByRole("alert")).toHaveTextContent("No license data");
    expect(screen.queryByText(/Community/)).not.toBeInTheDocument();
  });
  it("preserves a signed seat limit below ten", async () => {
    api.getLicense.mockResolvedValue({ ...license, seats: { used: 1, total: 4 } });
    renderPage(false);
    await screen.findByText("1 / 4");
    expect(screen.queryByText(/Community/)).not.toBeInTheDocument();
  });
  it("activates only after an explicit action and clears a successful draft", async () => {
    api.activateLicense.mockResolvedValue(license);
    renderPage();
    await screen.findByText("1 / 10");
    fireEvent.click(screen.getByRole("button", { name: /^more seats$/i }));
    const field = screen.getByRole("textbox", { name: /license key/i });
    fireEvent.change(field, { target: { value: "  mock-token  " } });
    expect(api.activateLicense).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /^activate$/i }));
    await waitFor(() => expect(api.activateLicense).toHaveBeenCalledWith({ activationToken: "mock-token" }, expect.anything()));
    await waitFor(() => expect(field).toHaveValue(""));
  });
  it("refreshes a signed entitlement only on an explicit permitted action", async () => {
    api.refreshLicense.mockResolvedValue(license);
    renderPage();
    await screen.findByText("1 / 10");
    expect(api.refreshLicense).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /^refresh$/i }));
    await waitFor(() => expect(api.refreshLicense).toHaveBeenCalledTimes(1));
    expect(api.activateLicense).not.toHaveBeenCalled();
  });
});
