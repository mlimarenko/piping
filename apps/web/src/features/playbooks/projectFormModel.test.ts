import { describe, expect, it } from "vitest";
import { emptyProjectForm, projectSource, validProjectForm } from "./projectFormModel";

describe("project source form", () => {
  it("builds editable and authenticated Git requests and validates references", () => {
    const form = { ...emptyProjectForm(), name: "Ops" };
    expect(projectSource(form)).toEqual({ kind: "editable" });
    const git = { ...form, sourceKind: "git" as const, url: "https://git.example/x", reference: "main", authKind: "https_basic" as const, username: "alice", secretId: "11111111-1111-1111-1111-111111111111" };
    expect(validProjectForm(git)).toBe(true);
    expect(projectSource(git)).toEqual({ kind: "git", url: git.url, reference: "main", auth: { kind: "https_basic", username: "alice", secret_id: git.secretId } });
    expect(validProjectForm({ ...git, secretId: "bad" })).toBe(false);
    expect(projectSource({ ...git, authKind: "https_bearer" })).toEqual({ kind: "git", url: git.url, reference: "main", auth: { kind: "https_bearer", secret_id: git.secretId } });
    expect(projectSource({ ...git, authKind: "ssh_record", sshRecordId: git.secretId })).toEqual({ kind: "git", url: git.url, reference: "main", auth: { kind: "ssh_record", ssh_record_id: git.secretId } });
  });
});
