import { describe, expect, it } from "vitest";

import {
  addFile,
  buildFileTree,
  draftIssue,
  draftToUpload,
  duplicatePaths,
  filesEqual,
  filesToDraft,
  nextFilePath,
  removeFile,
  updateFile,
  type ProjectFileDraft,
} from "./projectFilesModel";

let draftSequence = 0;

const draft = (overrides: Partial<ProjectFileDraft> = {}): ProjectFileDraft => {
  draftSequence += 1;
  return {
    id: `test-file-${draftSequence}`,
    path: "site.yml",
    content: "- hosts: all",
    executable: false,
    ...overrides,
  };
};

describe("projectFilesModel", () => {
  it("round-trips the server read-back into the upload request", () => {
    const files = [
      { path: "site.yml", content: "a", executable: false },
      { path: "roles/app/tasks/main.yml", content: "b", executable: true },
    ];
    const cloned = filesToDraft(files);
    expect(cloned.map(({ path, content, executable }) => ({ path, content, executable }))).toEqual(
      files,
    );
    expect(new Set(cloned.map((file) => file.id)).size).toBe(2);
    expect(draftToUpload(cloned)).toEqual(files);
  });

  it("compares drafts order-sensitively by path, content and executable", () => {
    expect(filesEqual([draft()], [draft()])).toBe(true);
    expect(filesEqual([draft()], [draft({ content: "other" })])).toBe(false);
    expect(filesEqual([draft()], [draft({ executable: true })])).toBe(false);
    expect(filesEqual([draft(), draft({ path: "b.yml" })], [draft()])).toBe(false);
    expect(
      filesEqual([draft(), draft({ path: "b.yml" })], [draft({ path: "b.yml" }), draft()]),
    ).toBe(false);
  });

  it("allocates a unique default path for a new file", () => {
    expect(nextFilePath([])).toBe("new-file.yml");
    expect(nextFilePath([draft({ path: "new-file.yml" })])).toBe("new-file-2.yml");
    expect(
      nextFilePath([draft({ path: "new-file.yml" }), draft({ path: "new-file-2.yml" })]),
    ).toBe("new-file-3.yml");
  });

  it("adds, renames and removes draft files without touching the rest", () => {
    const start = [draft({ path: "a.yml" }), draft({ path: "b.yml" })];
    const added = addFile(start, "new-file.yml");
    expect(added).toHaveLength(3);
    expect(added[2]).toMatchObject({ path: "new-file.yml", content: "", executable: false });
    expect(added[2]!.id).toBeTruthy();

    const renamed = updateFile(added, added[0]!.id, { path: "renamed.yml" });
    expect(renamed.map((file) => file.path)).toEqual([
      "renamed.yml",
      "b.yml",
      "new-file.yml",
    ]);
    expect(renamed[1]).toEqual(start[1]);

    const toggled = updateFile(renamed, renamed[1]!.id, { executable: true });
    expect(toggled[1]!.executable).toBe(true);

    const removed = removeFile(toggled, toggled[1]!.id);
    expect(removed.map((file) => file.path)).toEqual(["renamed.yml", "new-file.yml"]);
  });

  it("keeps same-path entries individually addressable after a colliding rename", () => {
    const base = filesToDraft([
      { path: "a.yml", content: "A", executable: false },
      { path: "b.yml", content: "B", executable: false },
    ]);
    const collided = updateFile(base, base[1]!.id, { path: "a.yml" });

    expect(duplicatePaths(collided)).toEqual(new Set(["a.yml"]));
    expect(draftIssue(collided)).toBe("duplicatePath");

    // Editing one entry by id must not touch the other same-path entry.
    const edited = updateFile(collided, collided[0]!.id, { content: "A2" });
    expect(edited[0]!.content).toBe("A2");
    expect(edited[1]!.content).toBe("B");

    // Deleting one entry removes exactly that entry.
    const removed = removeFile(collided, collided[0]!.id);
    expect(removed).toHaveLength(1);
    expect(removed[0]!.content).toBe("B");

    // The tree keeps both colliding files visible and selectable.
    const fileNodes = buildFileTree(collided).flatMap((node) =>
      node.kind === "file" ? [node] : node.children,
    );
    expect(fileNodes).toHaveLength(2);
    expect(new Set(fileNodes.map((node) => node.fileId)).size).toBe(2);
  });

  it("flags duplicate, blank and empty drafts", () => {
    expect(duplicatePaths([draft({ path: "a.yml" }), draft({ path: "a.yml" })])).toEqual(
      new Set(["a.yml"]),
    );
    expect(draftIssue([draft({ path: "a.yml" })])).toBeNull();
    expect(draftIssue([])).toBe("empty");
    expect(draftIssue([draft({ path: " " })])).toBe("blankPath");
    expect(draftIssue([draft({ path: "a.yml" }), draft({ path: "a.yml" })])).toBe(
      "duplicatePath",
    );
  });

  it("derives folders from path segments", () => {
    const tree = buildFileTree([
      draft({ path: "site.yml" }),
      draft({ path: "roles/app/tasks/main.yml" }),
      draft({ path: "roles/app/defaults/main.yml" }),
    ]);

    const names = tree.map((node) => `${node.kind}:${node.path}`);
    expect(names).toEqual(["folder:roles", "file:site.yml"]);

    const roles = tree.find((node) => node.path === "roles");
    expect(roles?.kind).toBe("folder");
    const app = roles?.children.find((node) => node.path === "roles/app");
    expect(app?.kind).toBe("folder");
    const tasks = app?.children.find((node) => node.path === "roles/app/tasks");
    expect(tasks?.kind).toBe("folder");
    expect(tasks?.children[0]).toMatchObject({
      kind: "file",
      name: "main.yml",
      path: "roles/app/tasks/main.yml",
    });
    expect(tasks?.children[0]?.fileId).toBeTruthy();
  });
});
