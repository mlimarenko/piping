import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { AllPermissions } from "@/test/permissions";
import type { ComponentProps } from "react";
import type { ScheduleEditor } from "./ScheduleEditor";
import { describe, expect, it, vi } from "vitest";
import ScheduleEditorPage from "./ScheduleEditorPage";
import "@/shared/i18n";

const api = vi.hoisted(() => ({ parent: vi.fn(), catalog: vi.fn(), list: vi.fn().mockResolvedValue({ items: [] }) }));
vi.mock("./api", () => ({ getPlaybook: api.parent, listPlaybooks: api.catalog, getSchedule: vi.fn(), listPlaybookServicePrincipals: api.list, listPlaybookTargetSets: api.list, createPlaybookSchedule: vi.fn(), createPlaybookServicePrincipal: vi.fn(), updatePlaybookSchedule: vi.fn() }));
vi.mock("./ScheduleEditor", () => ({ ScheduleEditor: (props: ComponentProps<typeof ScheduleEditor>) => <>
  <output>{JSON.stringify({ playbookId: props.form.playbookId, playbooks: props.playbooks })}</output>
  <button onClick={props.onBack}>Back to parent</button>
</> }));
function Destination() { const location = useLocation(); return <p>{location.pathname + location.search}</p>; }
describe("schedule creation from selected playbook", () => {
  it("seeds the exact parent beyond any catalog page and returns to its filters", async () => {
    const id = "11111111-1111-4111-8111-111111111111";
    const parent = { playbook: { id, name: "Parent outside catalog" } };
    api.parent.mockResolvedValue(parent);
    const returnTo = `/playbooks/${id}?section=schedules&schedules.enabled=false`;
    render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AllPermissions>
      <MemoryRouter initialEntries={[`/automation/schedules/new?${new URLSearchParams({ playbook: id, returnTo })}`]}><Routes>
        <Route path="/automation/schedules/new" element={<ScheduleEditorPage />} />
        <Route path="/playbooks/:id" element={<Destination />} />
      </Routes></MemoryRouter>
    </AllPermissions></QueryClientProvider>);
    expect(await screen.findByText(JSON.stringify({ playbookId: id, playbooks: [parent] }))).toBeInTheDocument();
    expect(api.parent).toHaveBeenCalledWith(id);
    expect(api.catalog).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Back to parent" }));
    expect(await screen.findByText(returnTo)).toBeInTheDocument();
  });
});
