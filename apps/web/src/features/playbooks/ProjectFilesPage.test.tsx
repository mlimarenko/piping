import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";

import ProjectFilesPage from "./ProjectFilesPage";
import type { PlaybookProject } from "./api";

const apiMock = vi.hoisted(() => ({
  getProject: vi.fn(),
  getProjectFiles: vi.fn(),
  uploadProjectFiles: vi.fn(),
}));

vi.mock("./api", () => ({
  getProject: apiMock.getProject,
  getProjectFiles: apiMock.getProjectFiles,
  uploadProjectFiles: apiMock.uploadProjectFiles,
}));

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({ notify: notifyMock }));

vi.mock("./CodeEditor", () => ({
  default: ({
    value,
    onChange,
    readOnly,
    "aria-label": ariaLabel,
  }: {
    value: string;
    onChange: (value: string) => void;
    readOnly?: boolean;
    "aria-label"?: string;
  }) => (
    <textarea
      aria-label={ariaLabel ?? "code"}
      readOnly={readOnly}
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

const manualProject: PlaybookProject = {
  id: "project-1",
  name: "Ops project",
  description: "Playbooks from SCM",
  scm_type: "manual",
  scm_url: null,
  scm_branch: null,
  scm_ref: null,
  playbook_base_path: "playbooks",
  credential_id: null,
  default_execution_environment_id: null,
  sync_policy: "manual",
  sync_status: "never_synced",
  last_sync_revision: null,
  last_sync_error: null,
  last_synced_at: null,
  metadata: {},
  created_at: "2026-06-06T00:00:00Z",
  updated_at: "2026-06-06T00:00:00Z",
};

const storedFiles = [
  { path: "site.yml", content: "- hosts: all", executable: false },
  { path: "roles/app/tasks/main.yml", content: "# tasks", executable: true },
];

function LocationProbe() {
  const location = useLocation();
  return <span data-testid="location-path">{location.pathname}</span>;
}

function renderPage(project: PlaybookProject = manualProject, files = storedFiles) {
  apiMock.getProject.mockResolvedValue(project);
  apiMock.getProjectFiles.mockResolvedValue({ files });
  apiMock.uploadProjectFiles.mockResolvedValue(project);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <MemoryRouter initialEntries={["/automation/projects/project-1/files"]}>
          <Routes>
            <Route path="/automation/projects/:id/files" element={<ProjectFilesPage />} />
            <Route path="/automation/projects" element={<p>Projects list</p>} />
          </Routes>
          <LocationProbe />
        </MemoryRouter>
      </AllPermissions>
    </QueryClientProvider>,
  );
}

describe("ProjectFilesPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("loads a manual project and saves the whole edited set", async () => {
    renderPage();
    expect(await screen.findByRole("heading", { name: "Ops project" })).toBeInTheDocument();

    const editor = await screen.findByRole("textbox", { name: "Content" });
    expect(editor).toHaveValue("- hosts: all");

    fireEvent.change(editor, { target: { value: "- hosts: web" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(apiMock.uploadProjectFiles).toHaveBeenCalledWith("project-1", [
        { path: "site.yml", content: "- hosts: web", executable: false },
        { path: "roles/app/tasks/main.yml", content: "# tasks", executable: true },
      ]),
    );
    expect(notifyMock).toHaveBeenCalledWith("success", "Project files saved");
  });

  it("adds, renames and deletes draft files without saving", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "Ops project" });

    fireEvent.click(screen.getByRole("button", { name: "Add file" }));
    const pathInput = screen.getByRole("textbox", { name: "Path" });
    expect(pathInput).toHaveValue("new-file.yml");

    fireEvent.change(pathInput, { target: { value: "extra/tasks.yml" } });
    expect(screen.getByText("tasks.yml")).toBeInTheDocument();
    expect(screen.getByText("extra")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(screen.queryByText("tasks.yml")).not.toBeInTheDocument();

    expect(apiMock.uploadProjectFiles).not.toHaveBeenCalled();
  });

  it("freezes file mutations during save and retains the draft after failure", async () => {
    renderPage();
    let fail!: (error: Error) => void;
    apiMock.uploadProjectFiles.mockImplementation(() => new Promise((_resolve, reject) => { fail = reject; }));
    const content = await screen.findByRole("textbox", { name: "Content" });
    const path = screen.getByRole("textbox", { name: "Path" });
    fireEvent.change(content, { target: { value: "# edited draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(path).toBeDisabled());
    expect(content).toHaveAttribute("readonly");
    expect(screen.getByRole("button", { name: "Add file" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: /executable/i })).toBeDisabled();
    // Custom editors can emit a callback even after their DOM becomes read-only.
    fireEvent.change(content, { target: { value: "# ignored pending edit" } });
    fail(new Error("Save failed"));
    await waitFor(() => expect(path).toBeEnabled());
    expect(content).toHaveValue("# edited draft");
    fireEvent.change(content, { target: { value: "# retry draft" } });
    expect(content).toHaveValue("# retry draft");
    expect(apiMock.uploadProjectFiles).toHaveBeenCalledTimes(1);
  });

  it("guards dirty edits on exit with the discard dialog", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "Ops project" });

    const editor = screen.getByRole("textbox", { name: "Content" });
    fireEvent.change(editor, { target: { value: "- hosts: web" } });

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Discard" }));

    expect(await screen.findByText("Projects list")).toBeInTheDocument();
    expect(apiMock.uploadProjectFiles).not.toHaveBeenCalled();
  });

  it("keeps editing when the discard dialog is dismissed", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "Ops project" });

    fireEvent.change(screen.getByRole("textbox", { name: "Content" }), {
      target: { value: "- hosts: web" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Continue editing" }));

    expect(screen.getByRole("textbox", { name: "Content" })).toHaveValue("- hosts: web");
    expect(screen.getByTestId("location-path")).toHaveTextContent(
      "/automation/projects/project-1/files",
    );
  });

  it("shows stored files read-only for SCM projects", async () => {
    renderPage({ ...manualProject, scm_type: "git", scm_url: "https://git.example/ops.git" });

    expect(
      await screen.findByText(/File management applies to manual projects only/),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add file" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Content" })).toHaveAttribute("readonly");
    expect(screen.getByRole("textbox", { name: "Path" })).toHaveAttribute("readonly");
  });

  it("shows a plain empty state for an SCM project with no files", async () => {
    renderPage({ ...manualProject, scm_type: "git", scm_url: "https://git.example/ops.git" }, []);

    const emptyTexts = await screen.findAllByText("No files yet.");
    expect(emptyTexts.length).toBeGreaterThanOrEqual(1);
    expect(screen.queryByText("At least one file is required.")).not.toBeInTheDocument();
  });

  it("blocks saving once the last file is deleted", async () => {
    renderPage(manualProject, [storedFiles[0]!]);
    await screen.findByRole("heading", { name: "Ops project" });

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    expect(await screen.findByText("At least one file is required.")).toBeInTheDocument();
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    fireEvent.click(save);
    expect(apiMock.uploadProjectFiles).not.toHaveBeenCalled();
  });

  it("flags a colliding rename without losing either file", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "Ops project" });

    fireEvent.click(screen.getByText("main.yml"));
    const pathInput = screen.getByRole("textbox", { name: "Path" });
    fireEvent.change(pathInput, { target: { value: "site.yml" } });

    expect(await screen.findByText("Two files share this path.")).toBeInTheDocument();
    expect(screen.getAllByText("site.yml").length).toBeGreaterThanOrEqual(2);
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(apiMock.uploadProjectFiles).not.toHaveBeenCalled();
  });
});
