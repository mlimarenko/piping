import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { ApiError } from "@/shared/api/client";
import { AllPermissions } from "@/test/permissions";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import TargetSetEditorPage from "./TargetSetEditorPage";
import type * as PlaybookApi from "./api";

const api = vi.hoisted(() => ({ playbook: vi.fn(), target: vi.fn(), schedule: vi.fn(), project: vi.fn(), files: vi.fn(), list: vi.fn() }));
vi.mock("./api", async (original) => ({ ...await original<typeof PlaybookApi>(), getPlaybook: api.playbook, getTargetSet: api.target, getSchedule: api.schedule, getProject: api.project, getProjectFiles: api.files, listPlaybookTargetSets: api.list, listPlaybooks: api.list }));
vi.mock("@/features/connections/api", () => ({ CONNECTION_TARGETS_QUERY_KEY: ["connections", "targets"], listConnectionTargets: () => Promise.resolve({ items: [] }) }));
vi.mock("@/features/automation-secrets/api", () => ({ AUTOMATION_SECRETS_QUERY_KEY: ["secrets"], listAutomationSecrets: () => Promise.resolve([]) }));
vi.mock("./CodeEditor", () => ({ default: () => <textarea aria-label="Code" /> }));

const cases = [
  { name: "target", path: "/playbooks/settings/target-sets/:id/edit", entry: "/playbooks/settings/target-sets/t/edit", page: <TargetSetEditorPage />, fetch: api.target, loaded: { id: "t", name: "Loaded target", mode: "dynamic", selector: {} }, destination: "/playbooks/settings/target-sets" },
];
function Destination() {
  const location = useLocation();
  return <p>Returned to parent: {location.pathname + location.search}</p>;
}
function mount(testCase: typeof cases[number]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><AllPermissions><MemoryRouter initialEntries={[testCase.entry]}><Routes>
    <Route path={testCase.path} element={testCase.page} />
    <Route path="*" element={<Destination />} />
  </Routes></MemoryRouter></AllPermissions></QueryClientProvider>);
  return client;
}
function shell() {
  expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  const owner = screen.getByTestId("entity-editor-page");
  expect(document.querySelectorAll('[data-slot="entity-editor-page"]')).toHaveLength(1);
  expect(owner.querySelector('[data-slot="entity-editor-page"], [data-slot="page-layout"]')).toBeNull();
  expect(owner.parentElement?.closest('[data-slot="entity-editor-page"], [data-slot="page-layout"]')).toBeNull();
  expect(screen.getByRole("button", { name: "Back" })).toBeEnabled();
  return owner;
}
describe.each(cases)("$name route state shell", (testCase) => {
  beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    api.list.mockResolvedValue({ items: [] });
    api.files.mockResolvedValue({ files: [{ path: "site.yml", content: "# initial", executable: false }] });
    api.project.mockResolvedValue({ id: "p", name: "Loaded project", scm_type: "manual" });
  });
  it("keeps the same single feature shell and width from pending to loaded", async () => {
    let resolve!: (value: unknown) => void;
    testCase.fetch.mockImplementation(() => new Promise((done) => { resolve = done; }));
    mount(testCase);
    const pendingOwner = shell();
    const pendingClass = pendingOwner.className;
    expect(screen.getByLabelText("Loading...")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^(Save|Create)$/ })).not.toBeInTheDocument();
    act(() => { resolve(testCase.loaded); });
    await waitFor(() => expect(screen.queryByLabelText("Loading...")).not.toBeInTheDocument());
    expect(shell().className).toBe(pendingClass);
  });
  it("shows error with retry in the feature shell and recovers to loaded", async () => {
    testCase.fetch.mockRejectedValueOnce(new Error("private backend diagnostic")).mockResolvedValue(testCase.loaded);
    mount(testCase);
    await screen.findByRole("alert");
    shell();
    expect(screen.queryByText("private backend diagnostic")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    expect(testCase.fetch).toHaveBeenCalledTimes(2);
    shell();
  });
  it("shows missing data without a blank editable form, and preserves Back", async () => {
    testCase.fetch.mockResolvedValue(null);
    mount(testCase);
    await screen.findByRole("alert");
    shell();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^(Save|Create)$/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(await screen.findByText(`Returned to parent: ${testCase.destination}`)).toBeInTheDocument();
  });
  it("keeps missing API resources in the shell and never exposes an editable blank record", async () => {
    testCase.fetch.mockRejectedValue(new ApiError({ status: 404, message: "private missing diagnostic" }));
    mount(testCase);
    expect(await screen.findByRole("alert")).toHaveTextContent("Page Not Found");
    shell();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });
  it("keeps Back available while pending without waiting for the request", async () => {
    testCase.fetch.mockImplementation(() => new Promise(() => {}));
    mount(testCase);
    shell();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(await screen.findByText(`Returned to parent: ${testCase.destination}`)).toBeInTheDocument();
  });
  it("retains the loaded dirty draft and discard guard after a failed background refresh", async () => {
    testCase.fetch.mockResolvedValue(testCase.loaded);
    const client = mount(testCase);
    const input = (await screen.findAllByRole("textbox"))[0];
    if (!input) throw new Error("Loaded editor must expose an editable field");
    fireEvent.change(input, { target: { value: "Unsaved local draft" } });
    expect(screen.getByText("You have unsaved changes")).toBeInTheDocument();
    testCase.fetch.mockRejectedValue(new Error("private refresh diagnostic"));
    await act(async () => { await client.refetchQueries({ queryKey: ["playbooks"] }); });
    expect(input).toHaveValue("Unsaved local draft");
    shell();
    expect(screen.queryByText("private refresh diagnostic")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(screen.queryByText(`Returned to parent: ${testCase.destination}`)).not.toBeInTheDocument();
  });
});
