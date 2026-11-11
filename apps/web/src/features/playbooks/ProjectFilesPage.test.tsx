import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { ApiError } from "@/shared/api/client";
import ProjectFilesPage from "./ProjectFilesPage";

const api = vi.hoisted(() => ({ getProject: vi.fn(), getProjectRevision: vi.fn(), changeProjectRevision: vi.fn(), listExecutionEnvironments: vi.fn(), syncProject: vi.fn(), forkProject: vi.fn(), updateProject: vi.fn() }));
const demo = vi.hoisted(() => ({ active: false }));
vi.mock("@/demo/env", () => ({ get IS_DEMO() { return demo.active; } }));
vi.mock("./api", () => api);
vi.mock("@/features/automation-secrets/api", () => ({ AUTOMATION_SECRETS_QUERY_KEY: ["automation-secrets"], listAutomationSecrets: vi.fn().mockResolvedValue([]) }));
vi.mock("@/features/connections/connection-config/api", () => ({ listRecords: vi.fn().mockResolvedValue([]) }));
vi.mock("./CodeEditor", () => ({ default: ({ value, onChange, readOnly, documentId }: { value: string; onChange: (value: string) => void; readOnly: boolean; documentId: string }) => <textarea aria-label="Content" data-document-id={documentId} value={value} readOnly={readOnly} onChange={(e) => onChange(e.target.value)} /> }));
vi.mock("@/shared/ui-shared/FormSelect", () => ({ default: ({ label, value, onValueChange, options }: { label: string; value: string; onValueChange: (value: string) => void; options: { value: string; label: string }[] }) => <label>{label}<select value={value} onChange={(e) => onValueChange(e.target.value)}><option value="">Choose</option>{options.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}</select></label> }));
const project = { id: "p", name: "Ops", source: { kind: "editable" as const }, current_revision_id: "r1", created_at: "", updated_at: "" };
const revision = { project_id: "p", revision_id: "r1", created_at: "", tree_sha256: "hash", entries: [{ path: "site.yml", content: { kind: "file" as const, bytes: [65], executable: true } }] };

function renderPage(path = "files", deny: string[] = []) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const content = <MemoryRouter initialEntries={[`/playbooks/projects/p/${path}`]}><Routes><Route path="/playbooks/projects/:projectId/*" element={<ProjectFilesPage />} /></Routes></MemoryRouter>;
  render(<QueryClientProvider client={client}>{deny.length ? <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: (action, resource) => !deny.includes(`${action}:${resource}`), canAny: () => false, hasRole: () => false }}>{content}</PermissionsContext.Provider> : <AllPermissions>{content}</AllPermissions>}</QueryClientProvider>);
}

/** Opens one tree row's action menu the way the keyboard does. */
async function openFileActions(path: string) {
  const trigger = screen.getByRole("button", { name: `Actions for ${path}` });
  trigger.focus();
  fireEvent.keyDown(trigger, { key: "ArrowDown" });
  await screen.findByRole("menu");
}

describe("persisted project revisions", () => {
  beforeEach(() => { vi.restoreAllMocks(); vi.resetAllMocks(); demo.active = false; api.getProject.mockResolvedValue(project); api.getProjectRevision.mockResolvedValue(revision); api.listExecutionEnvironments.mockResolvedValue({ items: [] }); });
  it("retains a later document change while a revision save is in flight", async () => {
    let finish!: (value: unknown) => void;
    api.changeProjectRevision.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    renderPage();
    const editor = await screen.findByRole("textbox", { name: "Content" });
    const identity = editor.getAttribute("data-document-id");
    fireEvent.change(editor, { target: { value: "B" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(api.changeProjectRevision).toHaveBeenCalledTimes(1));
    fireEvent.change(editor, { target: { value: "C" } });
    act(() => { finish({ ...revision, revision_id: "r2", entries: [{ path: "site.yml", content: { kind: "file", bytes: [66], executable: true } }] }); });
    await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeEnabled());
    expect(screen.getByRole("textbox", { name: "Content" })).toHaveValue("C");
    expect(screen.getByRole("textbox", { name: "Content" })).toHaveAttribute("data-document-id", identity);
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });
  it("saves against the captured revision and adopts the returned baseline", async () => {
    api.changeProjectRevision.mockResolvedValue({ ...revision, revision_id: "r2", entries: [{ path: "site.yml", content: { kind: "file", bytes: [66], executable: true } }] });
    renderPage();
    fireEvent.change(await screen.findByRole("textbox", { name: "Content" }), { target: { value: "B" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(api.changeProjectRevision).toHaveBeenCalledWith("p", { base_revision_id: "r1", changes: [{ operation: "delete", path: "site.yml" }, { operation: "put", entry: { path: "site.yml", content: { kind: "file", bytes: [66], executable: true } } }] }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeDisabled());
    fireEvent.change(screen.getByRole("textbox", { name: "Content" }), { target: { value: "C" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(api.changeProjectRevision.mock.calls[1]?.[1].base_revision_id).toBe("r2"));
  });
  it("retains a conflicting draft until explicit discard and reload", async () => {
    api.changeProjectRevision.mockRejectedValue(new ApiError({ status: 409, message: "conflict" }));
    renderPage();
    fireEvent.change(await screen.findByRole("textbox", { name: "Content" }), { target: { value: "draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Content" })).toHaveValue("draft");
    api.getProject.mockResolvedValue({ ...project, current_revision_id: "r2" });
    api.getProjectRevision.mockResolvedValue({ ...revision, revision_id: "r2" });
    fireEvent.click(screen.getByRole("button", { name: "Discard and reload" }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard and reload" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Content" })).toHaveValue("A"));
  });
  it("retains a draft after an ordinary save failure", async () => {
    api.changeProjectRevision.mockRejectedValue(new Error("offline"));
    renderPage();
    fireEvent.change(await screen.findByRole("textbox", { name: "Content" }), { target: { value: "draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/Could not save/)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Content" })).toHaveValue("draft");
  });
  it("browses a Git revision read-only and forks that exact revision", async () => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "https://git.example/x", reference: "main", auth: { kind: "anonymous" } } });
    api.forkProject.mockResolvedValue({ ...project, id: "fork" });
    renderPage("source");
    fireEvent.click(await screen.findByRole("button", { name: "Create editable copy" }));
    const dialog = await screen.findByRole("dialog");
    const name = within(dialog).getByRole("textbox", { name: /Copy name/ });
    expect(name).toHaveFocus();
    expect(name).toHaveValue("");
    fireEvent.change(name, { target: { value: "Forked" } });
    fireEvent.submit(name.closest("form")!);
    await waitFor(() => expect(api.forkProject).toHaveBeenCalledWith("p", { revision_id: "r1", name: "Forked" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });
  it("cancels a fork with Escape without forking", async () => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "https://git.example/x", reference: "main", auth: { kind: "anonymous" } } });
    renderPage("source");
    fireEvent.click(await screen.findByRole("button", { name: "Create editable copy" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByRole("textbox", { name: /Copy name/ }), { target: { value: "Draft name" } });
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(api.forkProject).not.toHaveBeenCalled();
  });
  it("submits a fork once while it is in flight", async () => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "https://git.example/x", reference: "main", auth: { kind: "anonymous" } } });
    let finish!: (value: unknown) => void;
    api.forkProject.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    renderPage("source");
    fireEvent.click(await screen.findByRole("button", { name: "Create editable copy" }));
    const dialog = await screen.findByRole("dialog");
    const name = within(dialog).getByRole("textbox", { name: /Copy name/ });
    fireEvent.change(name, { target: { value: "Forked" } });
    fireEvent.submit(name.closest("form")!);
    await waitFor(() => expect(api.forkProject).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    const trigger = screen.getByRole("button", { name: "Create editable copy" });
    expect(trigger).toBeDisabled();
    fireEvent.click(trigger);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(api.forkProject).toHaveBeenCalledTimes(1);
    await act(async () => { finish({ ...project, id: "fork" }); await Promise.resolve(); });
  });
  it("renames the selected file through the dialog with its current path selected", async () => {
    renderPage();
    await screen.findByRole("textbox", { name: "Content" });
    await openFileActions("site.yml");
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
    const dialog = await screen.findByRole("dialog");
    const path = within(dialog).getByRole("textbox", { name: /New relative path/ });
    expect(path).toHaveValue("site.yml");
    expect(path).toHaveFocus();
    expect(path).toHaveProperty("selectionStart", 0);
    expect(path).toHaveProperty("selectionEnd", "site.yml".length);
    fireEvent.change(path, { target: { value: "main.yml" } });
    fireEvent.submit(path.closest("form")!);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.queryByText("site.yml")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(api.changeProjectRevision).toHaveBeenCalledWith("p", {
      base_revision_id: "r1",
      changes: [
        { operation: "delete", path: "site.yml" },
        { operation: "put", entry: { path: "main.yml", content: { kind: "file", bytes: [65], executable: true } } },
      ],
    }));
  });
  it("keeps an invalid rename open with its error and draft, then cancels with Escape", async () => {
    renderPage();
    await screen.findByRole("textbox", { name: "Content" });
    await openFileActions("site.yml");
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
    const dialog = await screen.findByRole("dialog");
    const path = within(dialog).getByRole("textbox", { name: /New relative path/ });
    fireEvent.change(path, { target: { value: "../escape.yml" } });
    fireEvent.submit(path.closest("form")!);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("That path is invalid or already exists.");
    expect(path).toHaveValue("../escape.yml");
    expect(path).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getAllByText("site.yml").length).toBeGreaterThan(0);
    expect(api.changeProjectRevision).not.toHaveBeenCalled();
  });
  it("adds a folder through the dialog and refuses an invalid path without losing it", async () => {
    renderPage();
    await screen.findByRole("textbox", { name: "Content" });
    fireEvent.click(screen.getByRole("button", { name: "Add folder" }));
    const dialog = await screen.findByRole("dialog");
    const path = within(dialog).getByRole("textbox", { name: /Folder relative path/ });
    expect(path).toHaveValue("");
    expect(path).toHaveFocus();
    fireEvent.change(path, { target: { value: "config//nested" } });
    fireEvent.submit(path.closest("form")!);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("That path is invalid or already exists.");
    expect(path).toHaveValue("config//nested");
    fireEvent.change(path, { target: { value: "config" } });
    fireEvent.submit(path.closest("form")!);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "config" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
  it("cancels a folder entry with Escape without adding it", async () => {
    renderPage();
    await screen.findByRole("textbox", { name: "Content" });
    fireEvent.click(screen.getByRole("button", { name: "Add folder" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByRole("textbox", { name: /Folder relative path/ }), { target: { value: "drafts" } });
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "drafts" })).not.toBeInTheDocument();
  });
  it("syncs the selected environment against the captured base and displays the returned revision", async () => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "https://git.example/x", reference: "main", auth: { kind: "anonymous" } } });
    api.listExecutionEnvironments.mockResolvedValue({ items: [{ id: "env", name: "Runner", enabled: true }] });
    api.syncProject.mockResolvedValue({ ...revision, revision_id: "r2" });
    renderPage("source");
    await screen.findByRole("option", { name: "Runner" });
    fireEvent.change(await screen.findByRole("combobox"), { target: { value: "env" } });
    fireEvent.click(screen.getByRole("button", { name: "Sync" }));
    await waitFor(() => expect(api.syncProject).toHaveBeenCalledWith("p", { base_revision_id: "r1", execution_environment_id: "env" }));
    expect(await screen.findByText("r2")).toBeInTheDocument();
  });
  it("retains the revision and source after sync failure", async () => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "https://git.example/x", reference: "main", auth: { kind: "anonymous" } } });
    api.listExecutionEnvironments.mockResolvedValue({ items: [{ id: "env", name: "Runner", enabled: true }] });
    api.syncProject.mockRejectedValue(new Error("offline"));
    renderPage("source");
    await screen.findByRole("option", { name: "Runner" });
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "env" } });
    fireEvent.click(screen.getByRole("button", { name: "Sync" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.getByText("r1")).toBeInTheDocument();
  });
  it("syncs an SSH record through the selected environment and shows the demo notice", async () => {
    demo.active = true;
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "ssh://git.example/x", reference: "main", auth: { kind: "ssh_record", ssh_record_id: "s" } } });
    api.listExecutionEnvironments.mockResolvedValue({ items: [{ id: "env", name: "Runner", enabled: true }] });
    api.syncProject.mockResolvedValue({ ...revision, revision_id: "r2" });
    renderPage("source");
    await screen.findByRole("option", { name: "Runner" });
    expect(screen.getByText(/Demo only: sync and fork use synthetic content/)).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "env" } });
    fireEvent.click(screen.getByRole("button", { name: "Sync" }));
    await waitFor(() => expect(api.syncProject).toHaveBeenCalledWith("p", { base_revision_id: "r1", execution_environment_id: "env" }));
    expect(await screen.findByText("r2")).toBeInTheDocument();
  });
  it("surfaces a refused SSH sync without adopting a revision", async () => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "ssh://git.example/x", reference: "main", auth: { kind: "ssh_record", ssh_record_id: "s" } } });
    api.listExecutionEnvironments.mockResolvedValue({ items: [{ id: "env", name: "Runner", enabled: true }] });
    api.syncProject.mockRejectedValue(new ApiError({ status: 403, message: "denied" }));
    renderPage("source");
    await screen.findByRole("option", { name: "Runner" });
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "env" } });
    fireEvent.click(screen.getByRole("button", { name: "Sync" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/sync failed/i);
    expect(screen.getByText("r1")).toBeInTheDocument();
  });
  it.each([
    ["authentication_failed", /credentials/i],
    ["host_key_rejected", /host key/i],
    ["reference_not_found", /branch or commit/i],
  ])("names the %s cause of a failed sync and keeps the current revision", async (code, cause) => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "ssh://git.example/x", reference: "main", auth: { kind: "ssh_record", ssh_record_id: "s" } } });
    api.listExecutionEnvironments.mockResolvedValue({ items: [{ id: "env", name: "Runner", enabled: true }] });
    api.syncProject.mockRejectedValue(new ApiError({ status: 502, message: "Project acquisition failed", reasonCode: code }));
    renderPage("source");
    await screen.findByRole("option", { name: "Runner" });
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "env" } });
    fireEvent.click(screen.getByRole("button", { name: "Sync" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(cause);
    expect(screen.getByText("r1")).toBeInTheDocument();
    expect(screen.queryByText("r2")).not.toBeInTheDocument();
  });
  it.each(["git_failed", "unexpected_failure"])("keeps the generic sync failure for the %s code", async (code) => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "ssh://git.example/x", reference: "main", auth: { kind: "ssh_record", ssh_record_id: "s" } } });
    api.listExecutionEnvironments.mockResolvedValue({ items: [{ id: "env", name: "Runner", enabled: true }] });
    api.syncProject.mockRejectedValue(new ApiError({ status: 502, message: "Project acquisition failed", reasonCode: code }));
    renderPage("source");
    await screen.findByRole("option", { name: "Runner" });
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "env" } });
    fireEvent.click(screen.getByRole("button", { name: "Sync" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/sync failed/i);
    expect(screen.getByText("r1")).toBeInTheDocument();
  });
  it("syncs from no base revision after a source change cleared the current revision", async () => {
    const gitProject = { ...project, source: { kind: "git" as const, url: "https://git.example/x", reference: "main", auth: { kind: "anonymous" as const } }, current_revision_id: "r1" };
    api.getProject.mockResolvedValue(gitProject);
    api.listExecutionEnvironments.mockResolvedValue({ items: [{ id: "env", name: "Runner", enabled: true }] });
    api.updateProject.mockResolvedValue({ ...gitProject, source: { ...gitProject.source, reference: "release" }, current_revision_id: null });
    api.syncProject.mockResolvedValue({ ...revision, revision_id: "r2" });
    renderPage("settings");
    fireEvent.change(await screen.findByDisplayValue("main"), { target: { value: "release" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(api.updateProject).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole("link", { name: "Source" }));
    await screen.findByRole("option", { name: "Runner" });
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "env" } });
    fireEvent.click(screen.getByRole("button", { name: "Sync" }));
    await waitFor(() => expect(api.syncProject).toHaveBeenCalledWith("p", { base_revision_id: null, execution_environment_id: "env" }));
  });
  it("saves settings from the returned project and keeps a failed draft", async () => {
    api.updateProject.mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce({ ...project, name: "Draft" });
    renderPage("settings");
    const name = await screen.findByRole("textbox", { name: "Name" });
    fireEvent.change(name, { target: { value: "Draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(name).toHaveValue("Draft");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    expect(name).toHaveValue("Draft");
    expect(api.updateProject).toHaveBeenLastCalledWith("p", { name: "Draft", description: null, source: { kind: "editable" } });
  });
  it("retains a cleared description if the server returns the old value", async () => {
    const original = { ...project, description: "Previous" };
    api.getProject.mockResolvedValue(original);
    api.updateProject.mockResolvedValue(original);
    renderPage("settings");
    const description = await screen.findByRole("textbox", { name: "Description" });
    fireEvent.change(description, { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Some project settings were not applied. Your changes are preserved.");
    expect(description).toHaveValue("");
    expect(api.updateProject).toHaveBeenCalledWith("p", { name: "Ops", description: null, source: { kind: "editable" } });
  });
  it.each(["UPDATE:PLAYBOOK_PROJECT", "CREATE:PLAYBOOK_PROJECT_REVISION"])("does not save revisions without %s", async (permission) => {
    renderPage("files", [permission]);
    expect(await screen.findByRole("textbox", { name: "Content" })).toHaveAttribute("readonly");
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(api.changeProjectRevision).not.toHaveBeenCalled();
  });
  it("denies settings mutations without project update", async () => {
    renderPage("settings", ["UPDATE:PLAYBOOK_PROJECT"]);
    expect(await screen.findByRole("textbox", { name: "Name" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(api.updateProject).not.toHaveBeenCalled();
  });
  it.each(["UPDATE:PLAYBOOK_PROJECT", "EXECUTE_NATIVE:PLAYBOOK_PROJECT", "READ:PLAYBOOK_EXECUTION_ENVIRONMENT"])("denies sync without %s", async (permission) => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "https://git.example/x", reference: "main", auth: { kind: "anonymous" } } });
    renderPage("source", [permission]);
    await screen.findByText("r1");
    expect(screen.queryByRole("button", { name: "Sync" })).not.toBeInTheDocument();
    expect(api.syncProject).not.toHaveBeenCalled();
  });
  it("denies fork without project create", async () => {
    renderPage("source", ["CREATE:PLAYBOOK_PROJECT"]);
    await screen.findByText("r1");
    expect(screen.queryByRole("button", { name: "Create editable copy" })).not.toBeInTheDocument();
    expect(api.forkProject).not.toHaveBeenCalled();
  });
  it("does not request a project when project read is denied", async () => {
    renderPage("files", ["READ:PLAYBOOK_PROJECT"]);
    await screen.findByText(/not found/i);
    expect(api.getProject).not.toHaveBeenCalled();
    expect(api.getProjectRevision).not.toHaveBeenCalled();
  });
  it("does not request a revision or expose fork when revision read is denied", async () => {
    renderPage("source", ["READ:PLAYBOOK_PROJECT_REVISION"]);
    await waitFor(() => expect(api.getProject).toHaveBeenCalled());
    await screen.findByText(/not found/i);
    expect(api.getProjectRevision).not.toHaveBeenCalled();
    expect(api.forkProject).not.toHaveBeenCalled();
  });
  it.each(["EXECUTE:SECRET_VARIABLE", "DELEGATE_CREDENTIALS:SECRET_VARIABLE"])("denies authenticated sync without %s", async (permission) => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "https://git.example/x", reference: "main", auth: { kind: "https_bearer", secret_id: "secret" } } });
    renderPage("source", [permission]);
    await screen.findByText("r1");
    expect(screen.queryByRole("button", { name: "Sync" })).not.toBeInTheDocument();
    expect(api.syncProject).not.toHaveBeenCalled();
  });
  it.each(["EXECUTE:SECRET_VARIABLE", "DELEGATE_CREDENTIALS:SECRET_VARIABLE"])("denies SSH record sync without %s", async (permission) => {
    api.getProject.mockResolvedValue({ ...project, source: { kind: "git", url: "ssh://git.example/x", reference: "main", auth: { kind: "ssh_record", ssh_record_id: "s" } } });
    renderPage("source", [permission]);
    await screen.findByText("r1");
    expect(screen.queryByRole("button", { name: "Sync" })).not.toBeInTheDocument();
    expect(api.syncProject).not.toHaveBeenCalled();
  });
});
