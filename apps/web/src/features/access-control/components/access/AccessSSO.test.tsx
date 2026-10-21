import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import AccessSSO from "./AccessSSO";

const api = vi.hoisted(() => ({
  getSsoConfig: vi.fn(),
  updateSsoConfig: vi.fn(),
  testSsoConnection: vi.fn(),
}));
vi.mock("@/features/access-control/api", () => api);
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));

describe("SSO configuration form", () => {
  it("labels its controls and preserves untouched non-UI configuration on save", async () => {
    api.getSsoConfig.mockResolvedValue({
      enabled: true,
      issuerUrl: "https://identity.example.test",
      clientId: "control-plane",
      redirectUrl: "https://app.example.test/callback",
      scopes: "openid profile",
      groupClaim: "groups",
      provisioningMode: "jit",
      clientSecretEnabled: true,
    });
    api.updateSsoConfig.mockResolvedValue({});
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><AllPermissions><AccessSSO /></AllPermissions></QueryClientProvider>);

    const issuer = await screen.findByRole("textbox", { name: "Issuer URL" });
    expect(screen.getByRole("switch", { name: "Enabled" })).toBeChecked();
    expect(screen.getByRole("textbox", { name: "Redirect URI" })).toHaveAttribute("readonly");
    expect(screen.getByLabelText("Client Secret")).toHaveAttribute("type", "password");
    fireEvent.change(issuer, { target: { value: "https://updated.example.test" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(api.updateSsoConfig).toHaveBeenCalledWith(expect.objectContaining({
      issuerUrl: "https://updated.example.test",
      provisioningMode: "jit",
      clientSecretEnabled: true,
    })));
    expect(api.updateSsoConfig.mock.calls[0]?.[0]).not.toHaveProperty("clientSecret");
  });
});
