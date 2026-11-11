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

const api = vi.hoisted(() => ({ list: vi.fn().mockResolvedValue({ items: [] }) }));
vi.mock("./api", () => ({ listJobTemplates: api.list, listPlaybookTargetSets: api.list }));
vi.mock("./ScheduleEditor", () => ({ ScheduleEditor: (props: ComponentProps<typeof ScheduleEditor>) => <>
  <output>{props.form.jobTemplateId}</output>
  <button onClick={props.onBack}>Back to parent</button>
</> }));
function Destination() { const location = useLocation(); return <p>{location.pathname + location.search}</p>; }
describe("schedule creation from selected job", () => {
  it("seeds the job identity and returns to the requested location", async () => {
    const id = "11111111-1111-4111-8111-111111111111";
    const returnTo = `/playbooks/jobs/${id}?section=schedules&schedules.enabled=false`;
    render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AllPermissions>
      <MemoryRouter initialEntries={[`/automation/schedules/new?${new URLSearchParams({ job_template_id: id, returnTo })}`]}><Routes>
        <Route path="/automation/schedules/new" element={<ScheduleEditorPage />} />
        <Route path="/playbooks/jobs/:id" element={<Destination />} />
      </Routes></MemoryRouter>
    </AllPermissions></QueryClientProvider>);
    expect(await screen.findByText(id)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Back to parent" }));
    expect(await screen.findByText(returnTo)).toBeInTheDocument();
  });
});
