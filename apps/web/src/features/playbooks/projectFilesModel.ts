import type { components } from "@/shared/api/schema.gen";

type ProjectEntry = components["schemas"]["ProjectEntry"];
type RevisionChange = components["schemas"]["RevisionChange"];

/** One file in the client-side draft of a manual project's file set.
 *  `id` is stable across renames so every operation addresses exactly one entry. */
export interface ProjectFileDraft {
  id: string;
  path: string;
  content: string;
  executable: boolean;
  /** Binary uploads are retained as bytes, not decoded as text. */
  bytes?: Uint8Array;
  symlinkTarget?: string;
}

/** A node in the path-derived file tree (folders are synthetic). */
export interface ProjectFileTreeNode {
  /** Path segment shown in the tree. */
  name: string;
  /** Full path for a file; accumulated folder prefix for a folder. */
  path: string;
  kind: "folder" | "file";
  children: ProjectFileTreeNode[];
  /** Draft identity of the file this node renders (file nodes only). */
  fileId?: string;
}

/** Shared TanStack Query key for one immutable project revision. */
export function projectRevisionQueryKey(projectId: string, revisionId: string) {
  return ["playbooks", "project", projectId, "revision", revisionId] as const;
}

let draftFileSequence = 0;

function nextDraftFileId(): string {
  draftFileSequence += 1;
  return `draft-file-${draftFileSequence}`;
}

/** Clone the server read-back into an editable draft. */
export function filesToDraft(files: ProjectEntry[]): ProjectFileDraft[] {
  return files.map((entry) => {
    const content = entry.content;
    if (!validProjectPath(entry.path)) throw new Error("Invalid project entry path");
    if (content.kind === "symlink") {
      const parts = entry.path.split("/").slice(0, -1);
      if (!content.target || content.target.startsWith("/") || content.target.includes("\\") || content.target.includes("\0")) throw new Error("Invalid symbolic link target");
      for (const part of content.target.split("/")) {
        if (!part || part === ".") continue;
        if (part === "..") { if (!parts.length) throw new Error("Symbolic link escapes project"); parts.pop(); }
        else parts.push(part);
      }
      return { id: nextDraftFileId(), path: entry.path, content: "", executable: false, symlinkTarget: content.target };
    }
    if (content.kind !== "file" || !Array.isArray(content.bytes) || content.bytes.some((byte) => !Number.isInteger(byte) || byte < 0 || byte > 255)) throw new Error("Invalid project file bytes");
    const bytes = new Uint8Array(content.bytes);
    try {
      const text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
      if (!text.includes("\0")) return { id: nextDraftFileId(), path: entry.path, content: text, executable: content.executable };
    } catch { /* Invalid UTF-8 is an opaque binary file. */ }
    return { id: nextDraftFileId(), path: entry.path, content: "", executable: content.executable, bytes };
  });
}

export function draftEntries(files: ProjectFileDraft[]): ProjectEntry[] {
  return files.map((file) => ({ path: file.path, content: file.symlinkTarget !== undefined
    ? { kind: "symlink", target: file.symlinkTarget }
    : { kind: "file", bytes: Array.from(file.bytes ?? new TextEncoder().encode(file.content)), executable: file.executable } }));
}

/** Delete changed old paths before putting replacements, including subtree renames. */
export function revisionChanges(baseline: ProjectFileDraft[], draft: ProjectFileDraft[]): RevisionChange[] {
  const old = draftEntries(baseline);
  const next = draftEntries(draft);
  const byPath = (entries: ProjectEntry[]) => new Map(entries.map((entry) => [entry.path, JSON.stringify(entry.content)]));
  const before = byPath(old);
  const after = byPath(next);
  return [
    ...old.filter((entry) => before.get(entry.path) !== after.get(entry.path)).map((entry): RevisionChange => ({ operation: "delete", path: entry.path })),
    ...next.filter((entry) => before.get(entry.path) !== after.get(entry.path)).map((entry): RevisionChange => ({ operation: "put", entry })),
  ];
}

/** Order-sensitive equality of the persisted fields (draft ids are ignored). */
export function filesEqual(a: ProjectFileDraft[], b: ProjectFileDraft[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((file, index) => {
    const other = b[index];
    return (
      file.path === other?.path &&
      file.content === other?.content &&
      file.executable === other?.executable &&
      file.symlinkTarget === other?.symlinkTarget &&
      (file.bytes === undefined ? other?.bytes === undefined : other?.bytes !== undefined && file.bytes.length === other.bytes.length && file.bytes.every((byte, i) => byte === other.bytes?.[i]))
    );
  });
}

/** A default path for a newly added draft file, unique within the set. */
export function nextFilePath(files: ProjectFileDraft[]): string {
  const taken = new Set(files.map((file) => file.path));
  if (!taken.has("new-file.yml")) return "new-file.yml";
  let index = 2;
  while (taken.has(`new-file-${index}.yml`)) index += 1;
  return `new-file-${index}.yml`;
}

export function addFile(files: ProjectFileDraft[], path: string): ProjectFileDraft[] {
  return [...files, { id: nextDraftFileId(), path, content: "", executable: false }];
}

/** Patch the file with this id; a changed `path` value renames it. */
export function updateFile(
  files: ProjectFileDraft[],
  id: string,
  patch: Partial<Omit<ProjectFileDraft, "id">>,
): ProjectFileDraft[] {
  return files.map((file) => (file.id === id ? { ...file, ...patch } : file));
}

export function removeFile(files: ProjectFileDraft[], id: string): ProjectFileDraft[] {
  return files.filter((file) => file.id !== id);
}

/** Rename a file or every descendant of a directory, without changing identities. */
export function renameProjectPath(files: ProjectFileDraft[], from: string, to: string): ProjectFileDraft[] {
  if (!validProjectPath(from) || !validProjectPath(to) || to.startsWith(`${from}/`)) return files;
  const result = files.map((file) => file.path === from || file.path.startsWith(`${from}/`)
    ? { ...file, path: to + file.path.slice(from.length) } : file);
  return draftIssue(result) ? files : result;
}

export function removeProjectPath(files: ProjectFileDraft[], path: string): ProjectFileDraft[] {
  return files.filter((file) => file.path !== path && !file.path.startsWith(`${path}/`));
}

export function validProjectPath(path: string): boolean {
  return path.length > 0 && !path.startsWith("/") && !path.includes("\\") &&
    path.split("/").every((part) => part !== "" && part !== "." && part !== ".." && !part.includes("\0"));
}

/** Paths that appear on more than one draft file. */
export function duplicatePaths(files: ProjectFileDraft[]): Set<string> {
  const seen = new Set<string>();
  const duplicates = new Set<string>();
  for (const file of files) {
    if (seen.has(file.path)) duplicates.add(file.path);
    seen.add(file.path);
  }
  return duplicates;
}

/** Validation code for the current draft, or null when it can be saved. The
 *  empty revisions are valid. */
export function draftIssue(
  files: ProjectFileDraft[],
): "blankPath" | "invalidPath" | "duplicatePath" | null {
  if (files.some((file) => !file.path.trim())) return "blankPath";
  if (files.some((file) => !validProjectPath(file.path))) return "invalidPath";
  if (duplicatePaths(files).size > 0) return "duplicatePath";
  const paths = new Set(files.map((file) => file.path));
  if (files.some((file) => file.path.split("/").slice(0, -1).some((_, index, parts) => paths.has(parts.slice(0, index + 1).join("/"))))) return "duplicatePath";
  return null;
}

/**
 * Build the folder tree implied by file paths. Folders are derived from path
 * segments and merged by name; every file entry gets its own node so duplicated
 * paths stay visible and individually selectable.
 */
export function buildFileTree(files: ProjectFileDraft[], folders: string[] = []): ProjectFileTreeNode[] {
  const roots: ProjectFileTreeNode[] = [];
  for (const folder of folders) {
    if (!validProjectPath(folder)) continue;
    let level = roots;
    let prefix = "";
    for (const segment of folder.split("/")) {
      prefix = prefix ? `${prefix}/${segment}` : segment;
      let node = level.find((candidate) => candidate.kind === "folder" && candidate.name === segment);
      if (!node) {
        node = { name: segment, path: prefix, kind: "folder", children: [] };
        level.push(node);
      }
      level = node.children;
    }
  }
  const ordered = [...files].sort((a, b) => a.path.localeCompare(b.path));
  for (const file of ordered) {
    const segments = file.path.split("/").filter(Boolean);
    if (segments.length === 0) continue;
    let level = roots;
    let prefix = "";
    segments.forEach((segment, index) => {
      const isFile = index === segments.length - 1;
      const path = prefix ? `${prefix}/${segment}` : segment;
      if (isFile) {
        level.push({ name: segment, path, kind: "file", children: [], fileId: file.id });
      } else {
        let node = level.find(
          (candidate) => candidate.name === segment && candidate.kind === "folder",
        );
        if (!node) {
          node = { name: segment, path, kind: "folder", children: [] };
          level.push(node);
        }
        level = node.children;
      }
      prefix = path;
    });
  }
  return roots;
}
