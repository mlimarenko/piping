import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { MemoryRouter, Route, Routes } from "react-router";
import { AllPermissions } from "@/test/permissions";
import "@/shared/i18n";
import ProfileEditor, { ProfileForm } from "./ProfileEditor";
import type * as Api from "./api";

const api = vi.hoisted(() => ({ updateProfile: vi.fn(), getProfile: vi.fn(), listExporters: vi.fn(), listMetricSets: vi.fn(), listQuerySources: vi.fn(), listAlertSources: vi.fn() }));
vi.mock("./api", async original => ({ ...(await original<typeof Api>()), ...api }));
const now = "2026-01-01T00:00:00Z";
const exporters: Api.ExporterCatalogItem[] = ["node", "process", "blackbox"].map((slug, index) => ({ id: `exporter-${index}`, name: slug, slug, enabled: true, exporter_type: "prometheus", default_job: slug, default_scheme: "http", default_port: 9100 + index, default_target_source: "host", default_metrics_path: "/metrics", labels: {}, created_at: now, updated_at: now }));
const profile: Api.MonitoringProfile = { id: "profile-1", name: "Synthetic baseline", slug: "synthetic-baseline", enabled: true, item_count: 3, assignment_count: 0, created_at: now, updated_at: now, items: exporters.map((exporter, index) => ({ id: `item-${index}`, profile_id: "profile-1", exporter_id: exporter.id, exporter_name: exporter.name, exporter_slug: exporter.slug, effective_job: exporter.slug, enabled: true, sort_order: index, labels: {}, created_at: now, updated_at: now })) };
beforeEach(() => {
  vi.clearAllMocks();
  api.updateProfile.mockResolvedValue(profile);
  api.getProfile.mockResolvedValue(profile);
  api.listExporters.mockResolvedValue({items:exporters,total:exporters.length});
  for (const list of [api.listMetricSets,api.listQuerySources,api.listAlertSources]) list.mockResolvedValue({items:[],total:0});
});
function mount() {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter><ProfileForm editing={profile} exporters={exporters} links={{ sets: [], sources: [], alerts: [] }} /></MemoryRouter></AllPermissions></QueryClientProvider>);
}
it("saves the three exporter items as one profile", async () => {
  mount();
  fireEvent.click(screen.getByRole("button", { name: /^save$|^сохранить$/i }));
  await waitFor(() => expect(api.updateProfile).toHaveBeenCalledWith("profile-1", expect.objectContaining({ items: exporters.map((exporter, index) => expect.objectContaining({ exporter_id: exporter.id, sort_order: index, labels: {}, enabled: true })) })));
});
it("keeps an invalid labels draft and does not issue a write", () => {
  mount();
  const labels = within(screen.getByRole("group", { name: /Exporter 1|Экспортёр 1/ })).getByRole("textbox", { name: /labels|метки/i });
  fireEvent.change(labels, { target: { value: "[" } });
  fireEvent.click(screen.getByRole("button", { name: /^save$|^сохранить$/i }));
  expect(api.updateProfile).not.toHaveBeenCalled();
  expect(labels).toHaveValue("[");
  expect(screen.getByText(/Check exporter ports|Проверьте порты/)).toBeVisible();
});
it("retains typed fields when the profile update fails", async () => {
  api.updateProfile.mockRejectedValue(new Error("offline"));
  mount();
  fireEvent.change(screen.getByRole("textbox", { name: /^name$|^название$/i }), { target: { value: "Unsent name" } });
  fireEvent.click(screen.getByRole("button", { name: /^save$|^сохранить$/i }));
  await waitFor(() => expect(api.updateProfile).toHaveBeenCalledTimes(1));
  expect(screen.getByRole("textbox", { name: /^name$|^название$/i })).toHaveValue("Unsent name");
});

it.each(["record", "catalog"] as const)("retains the dirty profile and offers retry after a background %s failure", async source => {
  const client=new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={["/profiles/profile-1/edit"]}><Routes><Route path="/profiles/:id/edit" element={<ProfileEditor />} /></Routes></MemoryRouter></AllPermissions></QueryClientProvider>);
  const name=await screen.findByRole("textbox",{name:/^name$|^название$/i});
  fireEvent.change(name,{target:{value:"Retained local name"}});
  const request=source==="record"?api.getProfile:api.listExporters;
  request.mockRejectedValue(new Error("Synthetic background failure"));
  await client.invalidateQueries({queryKey:["observability","profiles",source==="record"?"profile-1":"editor-references"]});
  const retry=await screen.findByRole("button",{name:/^retry$|повторить/i});
  expect(name).toHaveValue("Retained local name");
  if(source==="record")api.getProfile.mockResolvedValue(profile);
  else api.listExporters.mockResolvedValue({items:exporters,total:exporters.length});
  fireEvent.click(retry);
  await waitFor(()=>expect(screen.queryByRole("button",{name:/^retry$|повторить/i})).not.toBeInTheDocument());
  expect(name).toHaveValue("Retained local name");
});
