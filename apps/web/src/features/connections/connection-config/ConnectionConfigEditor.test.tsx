import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import ConnectionConfigEditor from "./ConnectionConfigEditor";
import type * as ConfigApi from "./api";

const api = vi.hoisted(() => ({ listRecords: vi.fn() }));
vi.mock("./api", async original => ({ ...await original<typeof ConfigApi>(), listRecords: api.listRecords }));
beforeEach(async () => { await i18n.changeLanguage("en"); api.listRecords.mockReset(); });

it.each([
  ["edit", "loading"], ["edit", "error"], ["edit", "missing"],
  ["create", "loading"], ["create", "error"],
])("keeps the configuration %s heading and Cancel while %s", async (mode, state) => {
  let resolve!: (value: unknown) => void;
  const deferred = new Promise(value => { resolve = value; });
  if (state === "loading") api.listRecords.mockReturnValue(deferred);
  else if (state === "error") api.listRecords.mockRejectedValue(new Error("fixture unavailable"));
  else api.listRecords.mockResolvedValue([]);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions>
    <MemoryRouter initialEntries={[mode === "edit" ? "/connections/config/missing/edit" : "/connections/config/new"]}><Routes>
      <Route path="/connections/config/new" element={<ConnectionConfigEditor />} />
      <Route path="/connections/config/:id/edit" element={<ConnectionConfigEditor />} />
      <Route path="/connections/config" element={<h1>Configuration list</h1>} />
    </Routes></MemoryRouter>
  </AllPermissions></QueryClientProvider>);
  if (state === "loading") await waitFor(() => expect(api.listRecords).toHaveBeenCalled());
  else await screen.findByText(i18n.t(state === "error" ? "connectionConfig.loadFailed" : "connectionConfig.notFound"));
  expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  expect(screen.getByRole("heading", { level: 1, name: i18n.t(mode === "edit" ? "connectionConfig.editTitle" : "connectionConfig.createTitle") })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /^(Save|Save changes|Create record)$/i })).not.toBeInTheDocument();
  const back = screen.getByRole("button", { name: i18n.t("connectionConfig.backToList") });
  expect(back).toBeEnabled();
  if (state === "error") expect(screen.getByRole("button", { name: i18n.t("retry") })).toBeEnabled();
  fireEvent.click(state === "error" ? back : screen.getByRole("button", { name: i18n.t("cancel") }));
  expect(await screen.findByRole("heading", { name: "Configuration list" })).toBeInTheDocument();
  await act(async () => { resolve([]); await deferred; });
});
