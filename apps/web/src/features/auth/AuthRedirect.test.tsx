import { act, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import AuthRedirect from "./AuthRedirect";

const LocationProbe = () => {
  const location = useLocation();
  return <div data-testid="location">{`${location.pathname}${location.search}`}</div>;
};

function renderRedirect(initialEntry: string) {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <Routes>
        <Route
          path="/auth/redirect"
          element={
            <>
              <AuthRedirect />
              <LocationProbe />
            </>
          }
        />
        <Route path="/auth/login" element={<LocationProbe />} />
        <Route path="/connections" element={<LocationProbe />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe("AuthRedirect", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows license errors and does not continue to the protected app", () => {
    vi.useFakeTimers();
    renderRedirect("/auth/redirect?error=license_unavailable&redirect=/connections");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("alert")).toHaveTextContent("Contact your administrator.");

    expect(
      screen.getByText("Sign-in is temporarily unavailable because of a license issue. Contact your administrator."),
    ).toBeInTheDocument();

    act(() => {
      vi.advanceTimersByTime(2_000);
    });

    expect(screen.getByTestId("location")).toHaveTextContent(
      "/auth/redirect?error=license_unavailable&redirect=/connections",
    );
  });

  it("preserves the error code when returning to login", () => {
    renderRedirect("/auth/redirect?error=license_seat_limit");

    fireEvent.click(screen.getByRole("button", { name: /back to login/i }));

    expect(screen.getByTestId("location")).toHaveTextContent(
      "/auth/login?error=license_seat_limit",
    );
  });
});
