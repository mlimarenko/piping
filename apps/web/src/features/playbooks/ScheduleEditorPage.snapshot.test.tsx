import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AllPermissions } from "@/test/permissions";
import "@/shared/i18n";
import ScheduleEditorPage from "./ScheduleEditorPage";

const api = vi.hoisted(() => ({ jobs: vi.fn(), list: vi.fn(), get: vi.fn(), update: vi.fn(), preview: vi.fn() }));
vi.mock("./api", () => ({ listJobTemplates: api.jobs, listPlaybookTargetSets: api.list, getSchedule: api.get, updatePlaybookSchedule: api.update, createPlaybookSchedule: vi.fn(), previewPlaybookTargetSet: api.preview }));
vi.mock("@/features/connections/api", () => ({ CONNECTION_TARGETS_QUERY_KEY: ["test", "connection-targets"], listConnectionTargets: vi.fn().mockResolvedValue({ items: [] }) }));

const targetSet = { id: "77777777-7777-4777-8777-777777777777", name: "Production hosts", entity_kind: "target_set", mode: "static" as const, selector: {}, created_at: "", updated_at: "" };
const hostA = { hostname: "node-a", target_id: "11111111-1111-4111-8111-111111111111", source_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa" };
const hostB = { hostname: "node-b", target_id: "22222222-2222-4222-8222-222222222222", source_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb" };
const hostC = { hostname: "node-c", target_id: "33333333-3333-4333-8333-333333333333", source_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc" };
const saved = { id: "existing", name: "Nightly", template_id: "job", owner_user_id: "delegator", targets: { kind: "fixed", target_set_id: targetSet.id, targets: [hostA] }, cron_expression: "0 3 * * *", timezone: "UTC", concurrency_policy: "skip_if_running", misfire_policy: "fire_once", enabled: true, metadata: {}, created_at: "", updated_at: "" };

function preview(...hosts: typeof hostA[]) {
  return { target_set: targetSet, items: hosts.map((host) => ({ name: host.hostname, target_id: host.target_id, source_id: host.source_id })), total: hosts.length, limit: 1000, truncated: false };
}

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={["/playbooks/schedules/existing/edit"]}><Routes>
    <Route path="/playbooks/schedules/:id/edit" element={<ScheduleEditorPage />} />
    <Route path="*" element={<p>Returned</p>} />
  </Routes></MemoryRouter></AllPermissions></QueryClientProvider>);
}

async function openSnapshotDialog() {
  await screen.findByRole("textbox", { name: /^name$/i });
  fireEvent.click(screen.getByRole("button", { name: /update snapshot from set/i }));
  return screen.findByRole("dialog");
}

describe("schedule snapshot refresh", () => {
  beforeEach(() => {
    api.jobs.mockReset().mockResolvedValue({ items: [] });
    api.list.mockReset().mockResolvedValue({ items: [targetSet] });
    api.get.mockReset().mockResolvedValue(saved);
    api.update.mockReset().mockResolvedValue(saved);
    api.preview.mockReset();
  });

  it("shows added and removed hosts and applies the refreshed snapshot", async () => {
    api.preview.mockResolvedValue(preview(hostB, hostC));
    mount();
    const dialog = await openSnapshotDialog();
    expect(dialog).toHaveTextContent(/new composition: 2 hosts/i);
    expect(dialog).toHaveTextContent("node-b");
    expect(dialog).toHaveTextContent("node-c");
    expect(dialog).toHaveTextContent("node-a");
    fireEvent.click(screen.getByRole("button", { name: /^update snapshot$/i }));
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(await screen.findByText("Returned")).toBeInTheDocument();
    expect(api.update).toHaveBeenCalledWith("existing", expect.objectContaining({ targets: { kind: "fixed", target_set_id: targetSet.id, targets: [hostB, hostC] } }));
  });

  it("keeps the saved snapshot when the dialog is cancelled", async () => {
    api.preview.mockResolvedValue(preview(hostB));
    mount();
    await openSnapshotDialog();
    fireEvent.click(screen.getByRole("button", { name: /^cancel$/i }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(await screen.findByText("Returned")).toBeInTheDocument();
    expect(api.update).toHaveBeenCalledWith("existing", expect.objectContaining({ targets: { kind: "fixed", target_set_id: targetSet.id, targets: [hostA] } }));
  });

  it("diffs every refresh against the snapshot it just applied", async () => {
    api.preview.mockResolvedValueOnce(preview(hostB)).mockResolvedValueOnce(preview(hostC));
    mount();
    await openSnapshotDialog();
    fireEvent.click(screen.getByRole("button", { name: /^update snapshot$/i }));
    fireEvent.click(screen.getByRole("button", { name: /update snapshot from set/i }));
    const second = await screen.findByRole("dialog");
    expect(second).toHaveTextContent("node-c");
    expect(second).toHaveTextContent("node-b");
    expect(second).not.toHaveTextContent("node-a");
    fireEvent.click(screen.getByRole("button", { name: /^update snapshot$/i }));
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(await screen.findByText("Returned")).toBeInTheDocument();
    expect(api.update).toHaveBeenCalledWith("existing", expect.objectContaining({ targets: { kind: "fixed", target_set_id: targetSet.id, targets: [hostC] } }));
  });

  it("does not offer snapshot refresh for a dynamic selector", async () => {
    api.get.mockResolvedValue({ ...saved, targets: { kind: "dynamic", selector: {}, target_set_id: targetSet.id } });
    mount();
    await screen.findByRole("textbox", { name: /^name$/i });
    expect(screen.queryByRole("button", { name: /update snapshot from set/i })).not.toBeInTheDocument();
  });

  it("reports invalid target JSON at the advanced field and blocks saving", async () => {
    mount();
    await screen.findByRole("textbox", { name: /^name$/i });
    const json = screen.getByLabelText(/resolved fixed targets/i);
    fireEvent.change(json, { target: { value: "{" } });
    expect(await screen.findByText(/the targets are not valid json/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(api.update).not.toHaveBeenCalled();
  });
});
