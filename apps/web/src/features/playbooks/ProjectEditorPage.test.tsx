import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useParams } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import ProjectEditorPage from "./ProjectEditorPage";

const api = vi.hoisted(() => ({ createProject: vi.fn(), listExecutionEnvironments: vi.fn() }));
const authApi = vi.hoisted(() => ({ secrets: vi.fn(), records: vi.fn() }));
const notify = vi.hoisted(() => vi.fn());
vi.mock("./api", () => api);
vi.mock("@/shared/lib/notifications", () => ({ notify }));
vi.mock("@/features/automation-secrets/api", () => ({ AUTOMATION_SECRETS_QUERY_KEY: ["automation-secrets"], listAutomationSecrets: authApi.secrets }));
vi.mock("@/features/connections/connection-config/api", () => ({ listRecords: authApi.records }));

function Destination() { const { projectId } = useParams(); return <p>Created project {projectId}</p>; }
function renderPage(allow = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const content = <MemoryRouter initialEntries={["/playbooks/projects/new"]}><Routes><Route path="/playbooks/projects/new" element={<ProjectEditorPage />} /><Route path="/playbooks/projects/:projectId/files" element={<Destination />} /></Routes></MemoryRouter>;
  render(<QueryClientProvider client={client}>{allow ? <AllPermissions>{content}</AllPermissions> : <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: (action, resource) => !(action === "CREATE" && resource === "PLAYBOOK_PROJECT"), canAny: () => false, hasRole: () => false }}>{content}</PermissionsContext.Provider>}</QueryClientProvider>);
}

describe("create project", () => {
  beforeEach(() => { vi.resetAllMocks(); api.listExecutionEnvironments.mockResolvedValue({ items: [] }); authApi.secrets.mockResolvedValue([]); authApi.records.mockResolvedValue([]); });
  it("uses the returned project identity after creation", async () => {
    api.createProject.mockResolvedValue({ id: "created", name: "Ops", source: { kind: "editable" } });
    renderPage();
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Ops" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(api.createProject).toHaveBeenCalledWith({ name: "Ops", description: null, source: { kind: "editable" } }));
    expect(await screen.findByText("Created project created")).toBeInTheDocument();
    expect(api.listExecutionEnvironments).not.toHaveBeenCalled();
  });
  it("retains the entered draft on failure", async () => {
    api.createProject.mockRejectedValue({ message: "offline" });
    renderPage();
    const name = screen.getByRole("textbox", { name: "Name" });
    fireEvent.change(name, { target: { value: "Draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(notify).toHaveBeenCalledWith("error", "Could not create the project. Your changes are preserved."));
    expect(name).toHaveValue("Draft");
  });
  it("does not create without project create permission", () => {
    renderPage(false);
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Draft" } });
    expect(screen.getByRole("button", { name: "Create" })).toBeDisabled();
    expect(api.createProject).not.toHaveBeenCalled();
  });

  it("selects a Git secret by name and saves its UUID", async () => {
    const secretId = "22222222-2222-4222-8222-222222222222";
    authApi.secrets.mockResolvedValue([{ id: secretId, key: "deploy-token", description: "Git deploy", has_value: true }]);
    api.createProject.mockResolvedValue({ id: "created", name: "Ops", source: { kind: "git" } });
    renderPage();
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Ops" } });
    fireEvent.click(screen.getByRole("combobox", { name: "SCM type" }));
    fireEvent.click(await screen.findByRole("option", { name: "Git" }));
    fireEvent.change(screen.getByRole("textbox", { name: "SCM URL" }), { target: { value: "https://git.example/ops.git" } });
    fireEvent.click(screen.getByRole("combobox", { name: "Authentication" }));
    fireEvent.click(await screen.findByRole("option", { name: "HTTPS bearer" }));
    fireEvent.click(screen.getByRole("combobox", { name: "Secret" }));
    fireEvent.click(await screen.findByRole("option", { name: /deploy-token/ }));
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(api.createProject).toHaveBeenCalledWith(expect.objectContaining({
      name: "Ops",
      source: expect.objectContaining({ kind: "git", auth: { kind: "https_bearer", secret_id: secretId } }),
    })));
  });

  it("shows a load failure instead of an empty secret list", async () => {
    authApi.secrets.mockRejectedValue({ message: "offline" });
    renderPage();
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Ops" } });
    fireEvent.click(screen.getByRole("combobox", { name: "SCM type" }));
    fireEvent.click(await screen.findByRole("option", { name: "Git" }));
    fireEvent.click(screen.getByRole("combobox", { name: "Authentication" }));
    fireEvent.click(await screen.findByRole("option", { name: "HTTPS bearer" }));
    expect(await screen.findByText("Secrets could not be loaded; enter the secret ID in advanced options.")).toBeInTheDocument();
    expect(screen.queryByText("No secrets available")).not.toBeInTheDocument();
  });
});
