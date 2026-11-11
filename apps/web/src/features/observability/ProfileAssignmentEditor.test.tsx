import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { MemoryRouter } from "react-router";
import { AllPermissions } from "@/test/permissions";
import { ProfileAssignmentForm } from "./ProfileAssignmentEditor";
import type { ScopeEntities } from "./useScopeEntities";
import type * as Api from "./api";

const api = vi.hoisted(() => ({ previewProfile: vi.fn(), createProfileAssignment: vi.fn(), updateProfileAssignment: vi.fn() }));
vi.mock("./api", async original => ({ ...(await original<typeof Api>()), ...api }));
const entities: ScopeEntities = { optionsByScope: { role: [], connection_target: [], site: [], tenant: [] }, resolveName: () => "Global", isPending: false };
const timestamp = "2026-01-01T00:00:00Z";
const profile: Api.MonitoringProfile = { id: "profile-1", name: "Linux", slug: "linux", enabled: true, items: [], item_count: 3, assignment_count: 1, created_at: timestamp, updated_at: timestamp };
const assignment: Api.ProfileAssignment = { id: "assignment-1", profile_id: profile.id, profile_name: profile.name, profile_slug: profile.slug, scope_type: "global", enabled: true, weight: 0, created_at: timestamp, updated_at: timestamp };
beforeEach(() => {
  vi.clearAllMocks();
  api.previewProfile.mockResolvedValue({ conflicts: [], groups: [], targets_count: 3, hosts: [{ host_id: "host-1", host_name: "Host A", host_address: "192.0.2.1", kind: "device", targets: ["192.0.2.1:9100", "192.0.2.1:9256", "192.0.2.1:9633"] }] });
  api.updateProfileAssignment.mockResolvedValue(assignment);
});
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter><ProfileAssignmentForm editing={assignment} profiles={[profile]} entities={entities} /></MemoryRouter></AllPermissions></QueryClientProvider>);
}
it("shows all expanded targets and requires review before saving the assignment", async () => {
  mount();
  await screen.findByText("192.0.2.1:9633");
  const save = screen.getByRole("button", { name: /save|сохранить/i });
  expect(save).toBeDisabled();
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(save);
  await waitFor(() => expect(api.updateProfileAssignment).toHaveBeenCalledWith("assignment-1", expect.objectContaining({ profile_id: "profile-1", scope_type: "global", scope_id: null, weight: 0 })));
});
it("keeps the draft after a failed save", async () => {
  api.updateProfileAssignment.mockRejectedValue(new Error("offline"));
  mount();
  await screen.findByText("192.0.2.1:9633");
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: /save|сохранить/i }));
  await waitFor(() => expect(api.updateProfileAssignment).toHaveBeenCalledTimes(1));
  expect(screen.getByText("192.0.2.1:9633")).toBeVisible();
});
