import { screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import "@/shared/i18n";
import { describe, expect, it, vi } from "vitest";

import { AllPermissions } from "@/test/permissions";
import AlertSourceEditor from "./AlertSourceEditor";
import type * as ApiModule from "./api";

const apiMocks = vi.hoisted(() => ({
  createAlertSource: vi.fn(),
  updateAlertSource: vi.fn(),
  listAlertSources: vi.fn(),
}));

vi.mock("@/features/observability/api", async () => {
  const actual = await vi.importActual<typeof ApiModule>("@/features/observability/api");
  return { ...actual, ...apiMocks };
});

vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));

function renderPage() {
  apiMocks.listAlertSources.mockResolvedValue({ items: [], total: 0 });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <MemoryRouter initialEntries={["/observability/alert-sources/new"]}>
          <Routes>
            <Route path="/observability/alert-sources/new" element={<AlertSourceEditor />} />
            <Route path="/observability/alert-sources/:id/edit" element={<AlertSourceEditor />} />
          </Routes>
        </MemoryRouter>
      </AllPermissions>
    </QueryClientProvider>,
  );
}

describe("Alert source editor credential field", () => {
  it("presents credential_ref as a named-secret UUID, never a credential value", async () => {
    renderPage();

    expect(
      await screen.findByText("Bearer-token named secret ID (optional)"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "UUID of an existing named secret. The secret value is never stored in the source.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByPlaceholderText("00000000-0000-0000-0000-000000000000"),
    ).toHaveAccessibleDescription("UUID of an existing named secret. The secret value is never stored in the source.");
    expect(screen.getByRole("textbox", { name: /^Alerts URL/i })).toHaveAccessibleDescription(/Use the Alertmanager active-alerts endpoint/);
  });
});
