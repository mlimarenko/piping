import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import QuerySourceEditor from "./QuerySourceEditor";
import type * as ApiModule from "./api";

vi.mock("@/features/observability/api", async (original) => ({
  ...(await original<typeof ApiModule>()),
  listQuerySources: vi.fn().mockResolvedValue({ items: [], total: 0 }),
}));

it("describes the actual query-source inputs and retains the URL description while editing", async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions>
    <MemoryRouter initialEntries={["/observability/sources/new"]}>
      <Routes><Route path="/observability/sources/new" element={<QuerySourceEditor />} /></Routes>
    </MemoryRouter>
  </AllPermissions></QueryClientProvider>);
  const url = await screen.findByRole("textbox", { name: /^Query URL/i });
  expect(url).toHaveAccessibleDescription(/Use a Prometheus-compatible \/api\/v1\/query endpoint/);
  expect(screen.getByRole("textbox", { name: /Bearer-token named secret ID/i }))
    .toHaveAccessibleDescription("UUID of an existing named secret. The secret value is never stored in the source.");
  const descriptionId = url.getAttribute("aria-describedby");
  url.focus();
  fireEvent.change(url, { target: { value: "https://metrics.example/api/v1/query" } });
  expect(url).toHaveFocus();
  expect(url).toHaveValue("https://metrics.example/api/v1/query");
  expect(url).toHaveAttribute("aria-describedby", descriptionId);
});
