import { screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import ConnectionConfigDetail from "./ConnectionConfigDetail";
import type * as ConfigApi from "./api";

const api = vi.hoisted(() => ({ listRecords: vi.fn() }));
vi.mock("./api", async original => ({ ...await original<typeof ConfigApi>(), listRecords: api.listRecords }));

beforeEach(async () => { await i18n.changeLanguage("en"); api.listRecords.mockReset(); });

it.each(["missing", "error"])("keeps a heading and navigation when configuration is %s", async state => {
  if (state === "error") api.listRecords.mockRejectedValue(new Error("local fixture unavailable"));
  else api.listRecords.mockResolvedValue([]);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions>
    <MemoryRouter initialEntries={["/connections/config/missing"]}><Routes>
      <Route path="/connections/config/:id" element={<ConnectionConfigDetail />} />
    </Routes></MemoryRouter>
  </AllPermissions></QueryClientProvider>);
  await screen.findByText(i18n.t(state === "error" ? "connectionConfig.loadFailed" : "connectionConfig.notFound"));
  expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  expect(screen.getByRole("link", { name: i18n.t("connectionConfig.backToList") })).toHaveAttribute("href", "/connections/config");
});
