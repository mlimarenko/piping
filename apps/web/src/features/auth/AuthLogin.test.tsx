import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import { ApiError } from "@/shared/api/client";
import AuthLogin from "./AuthLogin";
import type * as AuthApi from "./api";

const mocks = vi.hoisted(() => ({
  login: vi.fn(),
}));

vi.mock("@/features/auth/api", async (importOriginal) => ({
  ...await importOriginal<typeof AuthApi>(),
  buildSsoStartUrl: vi.fn(() => "/api/v1/auth/login?redirect_to=%2Fconnections"),
  getAuthProviders: vi.fn().mockResolvedValue({
    localEnabled: true,
    oidcEnabled: true,
    ldapEnabled: false,
    adEnabled: false,
  }),
  login: (...args: unknown[]) => mocks.login(...args),
}));

function renderLogin(initialEntry = "/auth/login") {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });

  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[initialEntry]}>
        <Routes>
          <Route path="/auth/login" element={<AuthLogin />} />
          <Route path="/connections" element={<div>Connections</div>} />
          <Route path="/inventory/resources" element={<div>Inventory</div>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("AuthLogin", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    mocks.login.mockReset();
  });

  it("returns to the local page after signing in again", async () => {
    mocks.login.mockResolvedValue({ user: { id: "user" } });
    renderLogin("/auth/login?redirect_to=%2Finventory%2Fresources");
    fireEvent.change(screen.getByPlaceholderText("username or email"), { target: { value: "user@example.test" } });
    fireEvent.change(screen.getByPlaceholderText("Password"), { target: { value: "test-input" } });
    fireEvent.click(screen.getByRole("button", { name: /sign in/i }));
    expect(await screen.findByText("Inventory")).toBeInTheDocument();
  });

  it("locks the submitted credentials until login settles and retains them on failure", async () => {
    let rejectLogin!: (error: Error) => void;
    mocks.login.mockImplementation(() => new Promise((_resolve, reject) => { rejectLogin = reject; }));
    renderLogin();
    const identifier = screen.getByLabelText(i18n.t("auth.identifier"));
    const passwordInput = screen.getByLabelText(i18n.t("auth.password"), { selector: "input" });
    fireEvent.change(identifier, { target: { value: "layout-test-user" } });
    fireEvent.change(passwordInput, { target: { value: "local-test-draft" } });
    fireEvent.click(screen.getByRole("button", { name: /sign in/i }));
    await waitFor(() => expect(mocks.login).toHaveBeenCalledTimes(1));
    expect(identifier).toBeDisabled();
    expect(passwordInput).toBeDisabled();
    expect(screen.getByRole("button", { name: "Show password" })).toBeDisabled();
    fireEvent.change(identifier, { target: { value: "replacement-user" } });
    expect(identifier).toHaveValue("layout-test-user");
    act(() => { rejectLogin(new Error("local fixture failure")); });
    await waitFor(() => expect(identifier).toBeEnabled());
    expect(identifier).toHaveValue("layout-test-user");
    expect(passwordInput).toHaveValue("local-test-draft");
    expect(screen.getByRole("alert")).toBeVisible();
  });

  it("shows query license errors on the login page", () => {
    renderLogin("/auth/login?error=license_seat_limit");

    expect(
      screen.getByText("All available seats are in use. Contact your administrator."),
    ).toBeInTheDocument();
  });

  it("associates visible labels and announces login errors without exposing the password", () => {
    renderLogin("/auth/login?error=license_seat_limit");
    const identifier = screen.getByLabelText(i18n.t("auth.identifier"));
    const password = screen.getByLabelText(i18n.t("auth.password"), { selector: "input" });
    expect(identifier).toHaveAttribute("aria-required", "true");
    expect(password).toHaveAttribute("type", "password");
    expect(password).toHaveAttribute("aria-describedby", screen.getByRole("alert").id);
    const description = screen.getByRole("alert").textContent ?? "";
    expect(identifier).toHaveAccessibleDescription(description);
    expect(password).toHaveAccessibleDescription(description);
    fireEvent.click(screen.getByRole("button", { name: "Show password" }));
    expect(password).toHaveAttribute("type", "text");
    expect(password).toHaveAccessibleDescription(description);
    fireEvent.click(screen.getByRole("button", { name: "Hide password" }));
    expect(password).toHaveAttribute("type", "password");
    fireEvent.change(identifier, { target: { value: "fixture" } });
    expect(identifier).not.toHaveAttribute("aria-describedby");
    expect(password).not.toHaveAttribute("aria-describedby");
  });

  it("maps local-login license API errors to safe localized text", async () => {
    mocks.login.mockRejectedValue(
      new ApiError({
        status: 403,
        code: "LICENSE_FUTURE_INTERNAL_ERROR",
        reasonCode: "ENTITLEMENT_FUTURE_INTERNAL_ERROR",
        message:
          "license verification public key InvalidSignature JWT build activation",
      }),
    );
    renderLogin();

    fireEvent.change(screen.getByPlaceholderText("username or email"), {
      target: { value: "user@example.test" },
    });
    fireEvent.change(screen.getByPlaceholderText("Password"), {
      target: { value: "secret-password" },
    });
    fireEvent.click(screen.getByRole("button", { name: /sign in/i }));

    expect(
      await screen.findByText(
        "Sign-in is temporarily unavailable because of a license issue. Contact your administrator.",
      ),
    ).toBeInTheDocument();

    expect(
      screen.queryByText(
        /verification|InvalidSignature|key|JWT|build|activation|ENTITLEMENT_/i,
      ),
    ).toBeNull();
  });

  it("does not show raw backend text for unmapped login errors", async () => {
    mocks.login.mockRejectedValue(
      new ApiError({
        status: 500,
        code: "INTERNAL_ERROR",
        message: "backend stack trace public key JWT activation",
      }),
    );
    renderLogin();

    fireEvent.change(screen.getByPlaceholderText("username or email"), {
      target: { value: "user@example.test" },
    });
    fireEvent.change(screen.getByPlaceholderText("Password"), {
      target: { value: "secret-password" },
    });
    fireEvent.click(screen.getByRole("button", { name: /sign in/i }));

    expect(await screen.findByText("Sign-in failed")).toBeInTheDocument();

    expect(screen.queryByText(/backend stack trace|public key|JWT|activation/i)).toBeNull();
  });
});
