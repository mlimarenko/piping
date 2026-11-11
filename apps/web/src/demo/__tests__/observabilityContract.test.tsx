import { setupServer } from "msw/node";
import createClient from "openapi-fetch";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { render } from "@/test/pageChrome";
import { AllPermissions } from "@/test/permissions";
import { listMetricQueries, listQuerySources } from "@/features/observability/api";
import QuerySourceDetail from "@/features/observability/QuerySourceDetail";
import MetricQueryDetail from "@/features/observability/MetricQueryDetail";
import { handlers } from "../handlers";
import type { paths } from "@/shared/api/schema.gen";
import type * as ApiClientModule from "@/shared/api/client";

vi.mock("@/shared/api/client", async (original) => {
  const module = await original<typeof ApiClientModule>();
  // Native Node Request needs an origin; browser requests are same-origin.
  return { ...module, apiClient: createClient<paths>({
    baseUrl: window.location.origin,
    fetch: (request) => globalThis.fetch(request),
  }) };
});

const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterEach(() => server.resetHandlers());
afterAll(() => server.close());

function renderDetails(path: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path="/observability/sources/:id" element={<QuerySourceDetail />} />
            <Route path="/observability/metrics/:id" element={<MetricQueryDetail />} />
          </Routes>
        </MemoryRouter>
      </AllPermissions>
    </QueryClientProvider>,
  );
}

describe("observability demo API consumed by detail pages", () => {
  it("renders the seeded query endpoint and finite timeout from the real demo response", async () => {
    const { items } = await listQuerySources();
    expect(items.length).toBeGreaterThan(0);
    const item = items[0]!;
    expect(new URL(item.query_url).protocol).toBe("http:");
    expect(Number.isFinite(item.timeout_ms)).toBe(true);
    renderDetails(`/observability/sources/${item.id}`);
    expect(await screen.findByText(item.query_url)).toBeInTheDocument();
    expect(screen.getByText(`${item.timeout_ms} ms`)).toBeInTheDocument();
    expect(screen.queryByText("undefined ms")).not.toBeInTheDocument();
  });

  it("renders the actual queries, units and rendering metadata returned by demo GET", async () => {
    const { items } = await listMetricQueries();
    expect(items).toHaveLength(2);
    for (const item of items) {
      expect(item.query).toContain("node_");
      expect(item.slug).toMatch(/^[a-z][a-z-]+$/);
      expect(item.query_type).toBe("promql");
      expect(item.unit).toBe("%");
    }
    const item = items[0]!;
    renderDetails(`/observability/metrics/${item.id}`);
    expect(await screen.findByText(item.query)).toBeInTheDocument();
    expect(screen.getByText(item.slug)).toBeInTheDocument();
    expect(screen.getByText(item.widget_type)).toBeInTheDocument();
  });
});
