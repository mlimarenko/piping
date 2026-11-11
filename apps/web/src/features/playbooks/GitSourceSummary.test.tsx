import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { GitSourceSummary } from "./GitSourceSummary";
import type { Project, ProjectRevision, RevisionSelection } from "./api";

const api = vi.hoisted(() => ({ revision: vi.fn() }));
vi.mock("./api", () => ({ getProjectRevision: api.revision }));

const project: Project = { id: "project", name: "Operations", source: { kind: "git", url: "https://git.example.test/ops.git", reference: "main", auth: { kind: "anonymous" } }, current_revision_id: "revision-a", created_at: "", updated_at: "" };
const a: ProjectRevision = { project_id: project.id, revision_id: "revision-a", source_commit_oid: "a".repeat(40), tree_sha256: "1".repeat(64), created_at: "2026-11-01T10:00:00Z", entries: [] };
const b: ProjectRevision = { ...a, revision_id: "revision-b", source_commit_oid: "b".repeat(40), created_at: "2026-11-02T10:00:00Z" };

function setup(selection: RevisionSelection, initial = project) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = (value: Project) => <QueryClientProvider client={client}><MemoryRouter><GitSourceSummary project={value} selection={selection} /></MemoryRouter></QueryClientProvider>;
  const result = render(view(initial));
  return { update: (value: Project) => result.rerender(view(value)) };
}

describe("Git source selection", () => {
  beforeEach(() => {
    api.revision.mockReset().mockImplementation((_projectId: string, revisionId: string) => Promise.resolve(revisionId === a.revision_id ? a : b));
  });

  it("follows the successfully synchronized revision and refreshes the displayed commit", async () => {
    const { update } = setup({ kind: "current" });
    expect(await screen.findByText(a.source_commit_oid!)).toBeInTheDocument();
    expect(screen.getByText("Follow branch")).toBeInTheDocument();
    expect(screen.getByText("main")).toBeInTheDocument();
    update({ ...project, current_revision_id: b.revision_id });
    expect(await screen.findByText(b.source_commit_oid!)).toBeInTheDocument();
    expect(screen.queryByText(a.source_commit_oid!)).not.toBeInTheDocument();
    expect(screen.getByText("Last successful sync").parentElement?.querySelector("time")).toHaveAttribute("datetime", b.created_at);
  });

  it("keeps the pinned commit while reporting the newer successful project sync", async () => {
    const { update } = setup({ kind: "fixed", revision_id: a.revision_id });
    expect(await screen.findByText(a.source_commit_oid!)).toBeInTheDocument();
    update({ ...project, current_revision_id: b.revision_id });
    await vi.waitFor(() => expect(screen.getByText("Last successful sync").parentElement?.querySelector("time")).toHaveAttribute("datetime", b.created_at));
    expect(screen.getByText(a.source_commit_oid!)).toBeInTheDocument();
    expect(screen.queryByText(b.source_commit_oid!)).not.toBeInTheDocument();
    expect(screen.getByText("Pin commit")).toBeInTheDocument();
  });

  it("names an unsynchronized source and links to the existing sync page", () => {
    setup({ kind: "current" }, { ...project, current_revision_id: null });
    expect(screen.getByText("Not synchronized")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Synchronize the project");
    expect(screen.getByRole("link", { name: "Open project to sync" })).toHaveAttribute("href", "/playbooks/projects/project/source");
    expect(api.revision).not.toHaveBeenCalled();
  });

  it("does not substitute the current commit when the pinned revision cannot be read", async () => {
    api.revision.mockImplementation((_projectId: string, revisionId: string) => revisionId === a.revision_id ? Promise.reject(new Error("private upstream diagnostic")) : Promise.resolve(b));
    setup({ kind: "fixed", revision_id: a.revision_id }, { ...project, current_revision_id: b.revision_id });
    expect(await screen.findByRole("alert")).toHaveTextContent("No other revision is substituted");
    const commit = screen.getByText("Selected commit").parentElement!;
    expect(within(commit).getByText("Unavailable")).toBeInTheDocument();
    expect(screen.queryByText(b.source_commit_oid!)).not.toBeInTheDocument();
    expect(screen.queryByText("private upstream diagnostic")).not.toBeInTheDocument();
  });
});
