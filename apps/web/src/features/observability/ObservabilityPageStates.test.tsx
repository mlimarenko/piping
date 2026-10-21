import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import type * as ObservabilityApi from "./api";
import QuerySourceEditor from "./QuerySourceEditor";
import QuerySourceDetail from "./QuerySourceDetail";
import AlertSourceEditor from "./AlertSourceEditor";
import AlertSourceDetail from "./AlertSourceDetail";
import MetricQueryEditor from "./MetricQueryEditor";
import MetricQueryDetail from "./MetricQueryDetail";
import MetricSetEditor from "./MetricSetEditor";
import MetricSetDetail from "./MetricSetDetail";

const api = vi.hoisted(() => ({ listQuerySources: vi.fn(), listAlertSources: vi.fn(), listMetricQueries: vi.fn(), listMetricSets: vi.fn() }));
vi.mock("./api", async original => ({ ...await original<typeof ObservabilityApi>(), ...api }));
beforeEach(async () => { await i18n.changeLanguage("en"); vi.resetAllMocks(); });
function Location() { return <output data-testid="location">{useLocation().pathname}</output>; }

const families = [
  { base: "sources", editor: QuerySourceEditor, detail: QuerySourceDetail, query: api.listQuerySources },
  { base: "alert-sources", editor: AlertSourceEditor, detail: AlertSourceDetail, query: api.listAlertSources },
  { base: "metrics", editor: MetricQueryEditor, detail: MetricQueryDetail, query: api.listMetricQueries },
  { base: "sets", editor: MetricSetEditor, detail: MetricSetDetail, query: api.listMetricSets },
];
for (const family of families) {
  for (const view of ["detail", "editor"] as const) {
    for (const state of ["loading", "error", "missing"] as const) {
      it(`${family.base} ${view} retains heading and exit while ${state}`, async () => {
        if (state === "loading") family.query.mockReturnValue(new Promise(() => {}));
        else if (state === "error") family.query.mockRejectedValue(new Error("local fixture unavailable"));
        else family.query.mockResolvedValue({ items: [] });
        const Component = family[view];
        const path = `/observability/${family.base}/missing${view === "editor" ? "/edit" : ""}`;
        const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
        render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path={`/observability/${family.base}/:id${view === "editor" ? "/edit" : ""}`} element={<Component />} />
            <Route path="/observability" element={<Location />} />
          </Routes>
        </MemoryRouter></AllPermissions></QueryClientProvider>);
        if (state === "loading") await screen.findByLabelText(i18n.t("loading"));
        else await screen.findByText(i18n.t(state === "error" ? "observability.entityLoadFailed" : "observability.entityNotFound"));
        expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
        expect(screen.queryByRole("button", { name: /^save|^create/i })).not.toBeInTheDocument();
        const back = screen.queryByRole("link", { name: /^back$/i }) ?? screen.getByRole("button", { name: /^cancel$/i });
        fireEvent.click(back);
        expect(await screen.findByTestId("location")).toHaveTextContent("/observability");
      });
    }
  }
}
