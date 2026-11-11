import { setupServer } from "msw/node";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { handlers } from "../handlers";

const server = setupServer(...handlers);
const url = (path: string) => `${window.location.origin}/api/v1/playbooks/${path}`;
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

describe("synthetic automation demo", () => {
  it("lets the demo user browse project revisions and launch either inventory kind", async () => {
    const me = await (await fetch(`${window.location.origin}/api/v1/auth/me`)).json();
    for (const action of ["READ", "CREATE"]) {
      expect(me.capabilities).toContainEqual({ action, resource_type: "PLAYBOOK_PROJECT_REVISION" });
    }
    for (const action of ["EXECUTE_NATIVE", "DELEGATE_CREDENTIALS"]) {
      expect(me.capabilities).toContainEqual({ action, resource_type: "PLAYBOOK_JOB_TEMPLATE" });
    }
    const projects = await (await fetch(url("projects"))).json();
    const project = projects.items[0];
    const revision = await fetch(url(`projects/${project.id}/revisions/${project.current_revision_id}`));
    expect(revision.status).toBe(200);
    expect((await revision.json()).entries.some((entry: { path: string }) => entry.path === "site.yml")).toBe(true);
  });

  it("browses and saves immutable project revisions, preserving bytes, modes and links", async () => {
    const projects = await (await fetch(url("projects"))).json();
    const jobs = await (await fetch(url("job-templates"))).json();
    const project = projects.items[0];
    expect(project.name).toContain("Synthetic");
    expect(jobs.items[0].project_id).toBe(project.id);
    const revisionUrl = url(`projects/${project.id}/revisions`);
    const original = await (await fetch(`${revisionUrl}/${project.current_revision_id}`)).json();
    expect(original.entries[0].path).toBe("site.yml");
    const entries = [
      { path: "bin/check", content: { kind: "file", bytes: [0, 255, 10], executable: true } },
      { path: "link", content: { kind: "symlink", target: "bin/check" } },
    ];
    const save = (base_revision_id: string, changes: unknown[]) => fetch(revisionUrl, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ base_revision_id, changes }) });
    const response = await save(original.revision_id, entries.map(entry => ({ operation: "put", entry })));
    expect(response.status).toBe(201);
    const revision = await response.json();
    expect(revision.entries.slice(-2)).toEqual(entries);
    expect((await (await fetch(`${revisionUrl}/${original.revision_id}`)).json()).entries).toEqual(original.entries);
    expect((await save(original.revision_id, [])).status).toBe(409);
    expect((await save(revision.revision_id, [{ operation: "put", entry: { path: "../bad", content: { kind: "file", bytes: [256], executable: false } } }])).status).toBe(400);
    expect((await fetch(url(`projects/missing/revisions/${revision.revision_id}`))).status).toBe(404);
  });

  it("creates projects, simulates Git sync and forks immutable data", async () => {
    const send = (path: string, body: unknown) => fetch(url(path), { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
    const editable = await (await send("projects", { name: "Editable", source: { kind: "editable" } })).json();
    expect(editable.current_revision_id).toBeTruthy();
    expect((await (await fetch(url(`projects/${editable.id}/revisions/${editable.current_revision_id}`))).json()).entries).toEqual([]);
    const git = await (await send("projects", { name: "Synthetic Git", source: { kind: "git", url: "https://example.invalid/repo", reference: "main", auth: { kind: "anonymous" } } })).json();
    expect(git.description).toContain("no Git repository is contacted");
    expect(git.current_revision_id).toBeNull();
    const sync = () => send(`projects/${git.id}/sync`, { execution_environment_id: "00000000-0000-4000-8000-000000000205", base_revision_id: null });
    const revision = await (await sync()).json();
    expect(new TextDecoder().decode(new Uint8Array(revision.entries[0].content.bytes))).toContain("no Git repository was contacted");
    expect((await sync()).status).toBe(409);
    const fork = await (await send(`projects/${git.id}/fork`, { name: "Fork", revision_id: revision.revision_id })).json();
    expect(fork.description).toContain("Synthetic demo");
    expect(fork.source.kind).toBe("editable");
    expect((await (await fetch(url(`projects/${fork.id}/revisions/${fork.current_revision_id}`))).json()).entries).toEqual(revision.entries);
  });

  it("supports job, schedule and assignment CRUD and validates managed launch selection", async () => {
    const request = (path: string, method = "GET", body?: unknown) => fetch(url(path), { method, ...(body === undefined ? {} : { headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }) });
    const projectId = "00000000-0000-4000-8000-000000000196";
    const setId = "00000000-0000-4000-8000-000000000201";
    const targetId = "00000000-0000-4000-8001-000000000001";
    const hostResponse = await fetch(`${window.location.origin}/api/v1/connections/targets/${targetId}`);
    expect(hostResponse.status).toBe(200);
    expect((await hostResponse.json()).name).toBe("web-prod-01");
    const preview = await (await request(`target-sets/${setId}/preview`)).json();
    expect(preview.items.map((item: { target_id: string }) => item.target_id)).toContain(targetId);
    const template = { name: "Synthetic managed job", project_id: projectId, execution_environment_id: "00000000-0000-4000-8000-000000000205", workdir: ".", playbook_paths: ["site.yml"], revision: { kind: "current" }, inventory: { kind: "managed", target_set_id: setId }, options: { argv: [], env: {}, extra_vars: {}, secret_refs: {}, timeout_seconds: 60 } };
    const created = await request("job-templates", "POST", template);
    expect(created.status).toBe(201);
    const job = await created.json();
    expect((await (await request(`job-templates/${job.id}`, "PATCH", { name: "Updated synthetic job" })).json()).name).toBe("Updated synthetic job");
    const before = await (await request("run-groups")).json();
    for (const target_ids of [[], [targetId, targetId], ["00000000-0000-4000-8001-000000000002"], [targetId, "00000000-0000-4000-8001-000000000002"]]) expect((await request(`job-templates/${job.id}/launch`, "POST", { target_ids })).status).toBe(400);
    expect((await (await request("run-groups")).json()).total).toBe(before.total);
    const launchResponse = await request(`job-templates/${job.id}/launch`, "POST", { target_ids: [targetId] });
    expect(launchResponse.status).toBe(201);
    const launch = await launchResponse.json();
    expect(launch.run_group.metadata.synthetic).toBe(true);
    expect(launch.run_group.launch_spec.targets.map((target: { target_id: string }) => target.target_id)).toEqual([targetId]);
    expect(launch.runs[0].metadata.notice).toContain("No execution");
    const assignmentResponse = await request("assignments", "POST", { job_template_id: job.id, target_set_id: setId, scope_kind: "connection_target", scope_id: targetId });
    expect(assignmentResponse.status).toBe(201);
    const assignment = await assignmentResponse.json();
    expect((await (await request(`targets/${targetId}/assignments`)).json()).items[0].job_template.id).toBe(job.id);
    expect((await (await request(`assignments/${assignment.id}`, "PATCH", { visible: false })).json()).visible).toBe(false);
    const schedule = await (await request("schedules", "POST", { name: "Synthetic schedule", template_id: job.id, cron_expression: "0 9 * * 1", targets: { kind: "fixed", targets: [] } })).json();
    expect(schedule.targets.kind).toBe("fixed");
    expect((await (await request(`schedules/${schedule.id}`, "PATCH", { enabled: false })).json()).enabled).toBe(false);
    for (const path of [`assignments/${assignment.id}`, `schedules/${schedule.id}`, `job-templates/${job.id}`]) {
      expect((await request(path, "DELETE")).status).toBe(204);
      expect((await request(path)).status).toBe(404);
    }
    expect((await request("unknown/action", "POST", {})).status).toBe(501);
    expect((await request("schedules/anything/occurrences")).status).toBe(501);
    expect((await request("job-templates/00000000-0000-4000-8000-000000000197/launch", "POST", { target_ids: [targetId] })).status).toBe(400);
    expect((await request("target-sets", "POST", { name: "Dynamic", mode: "dynamic", selector: {} })).status).toBe(501);
  });
});
