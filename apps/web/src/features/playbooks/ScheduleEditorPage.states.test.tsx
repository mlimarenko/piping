import { act, fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import "@/shared/i18n";
import ScheduleEditorPage from "./ScheduleEditorPage";

const api = vi.hoisted(() => ({ jobs: vi.fn(), list: vi.fn(), get: vi.fn(), update: vi.fn(), create: vi.fn(), preview: vi.fn() }));
vi.mock("./api", () => ({ listJobTemplates: api.jobs, listPlaybookTargetSets: api.list, getSchedule: api.get, updatePlaybookSchedule: api.update, createPlaybookSchedule: api.create, previewPlaybookTargetSet: api.preview }));
vi.mock("@/features/connections/api", () => ({ CONNECTION_TARGETS_QUERY_KEY: ["test", "connection-targets"], listConnectionTargets: vi.fn().mockResolvedValue({ items: [] }) }));

const saved = { id: "existing", name: "Nightly", template_id: "job", owner_user_id: "delegator", targets: { kind: "dynamic", selector: { target_kind: ["custom"], source_kind: ["device"], custom: true } }, cron_expression: "0 3 * * *", timezone: "UTC", concurrency_policy: "skip_if_running", misfire_policy: "fire_once", enabled: true, metadata: {}, created_at: "", updated_at: "" };

function mount(path: string, denyExecute = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tree = <MemoryRouter initialEntries={[path]}><Routes><Route path="/playbooks/schedules/new" element={<ScheduleEditorPage />} /><Route path="/playbooks/schedules/:id/edit" element={<ScheduleEditorPage />} /><Route path="*" element={<p>Returned</p>} /></Routes></MemoryRouter>;
  const permissions = { roles: [], isAdmin: false, can: (action: string) => action !== "EXECUTE", canAny: () => false, hasRole: () => false };
  render(<QueryClientProvider client={client}>{denyExecute ? <PermissionsContext.Provider value={permissions}>{tree}</PermissionsContext.Provider> : <AllPermissions>{tree}</AllPermissions>}</QueryClientProvider>);
  return client;
}
describe("schedule draft route states", () => {
  beforeEach(() => { api.jobs.mockReset().mockResolvedValue({ items: [] }); api.list.mockReset().mockResolvedValue({ items: [] }); api.get.mockReset().mockResolvedValue(saved); api.update.mockReset(); });
  it("preserves the edited draft on failure and sends no immutable identities", async () => {
    api.update.mockRejectedValue(new Error("Save denied"));
    mount("/playbooks/schedules/existing/edit");
    const name = await screen.findByRole("textbox", { name: /^name$/i });
    fireEvent.change(name, { target: { value: "Changed schedule" } });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/could not save the schedule/i);
    expect(screen.queryByText("Save denied")).not.toBeInTheDocument();
    expect(name).toHaveValue("Changed schedule");
    expect(api.update).toHaveBeenCalledWith("existing", expect.not.objectContaining({ template_id: expect.anything(), service_principal_id: expect.anything() }));
    expect(api.update).toHaveBeenCalledWith("existing", expect.objectContaining({ targets: saved.targets, misfire_policy: "fire_once" }));
    expect(screen.getByRole("textbox", { name: /^job$/i })).toHaveAttribute("readonly");
  });
  it("returns after the update response", async () => {
    api.update.mockResolvedValue(saved);
    mount("/playbooks/schedules/existing/edit");
    await screen.findByRole("textbox", { name: /^name$/i });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(await screen.findByText("Returned")).toBeInTheDocument();
    expect(api.update).toHaveBeenCalledOnce();
  });
  it("updates the selector through the shared builder without losing loaded semantics", async () => {
    api.update.mockResolvedValue(saved);
    mount("/playbooks/schedules/existing/edit");
    await screen.findByRole("textbox", { name: /^name$/i });
    const [selectorSwitch] = screen.getAllByRole("switch");
    if (!selectorSwitch) throw new Error("Selector switch is missing");
    fireEvent.click(selectorSwitch);
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(await screen.findByText("Returned")).toBeInTheDocument();
    expect(api.update).toHaveBeenCalledWith("existing", expect.objectContaining({ targets: { kind: "dynamic", selector: { ...saved.targets.selector, address_bound: false } } }));
  });
  it("does not offer save without execute permission on the job template", async () => {
    mount("/playbooks/schedules/existing/edit", true);
    expect(await screen.findByRole("alert")).toHaveTextContent(/permission/i);
    expect(screen.queryByRole("button", { name: /^save$/i })).not.toBeInTheDocument();
    expect(api.update).not.toHaveBeenCalled();
  });
  it("retains a dirty draft and discard guard after reference refresh fails", async () => {
    const client = mount("/playbooks/schedules/new");
    const name = await screen.findByRole("textbox", { name: /^name$/i });
    fireEvent.change(name, { target: { value: "Unsaved schedule" } });
    api.jobs.mockRejectedValue(new Error("Reference data unavailable"));
    await act(async () => { await client.refetchQueries({ queryKey: ["playbooks", "job-templates"] }); });
    expect(name).toHaveValue("Unsaved schedule");
    expect(await screen.findByRole("alert")).toHaveTextContent("Reference data unavailable");
    fireEvent.click(screen.getByRole("button", { name: /^back$/i }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
  });
});
