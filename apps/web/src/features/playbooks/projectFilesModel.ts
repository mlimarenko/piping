import type {
  ProjectFileResponse,
  UploadProjectFileRequest,
} from "@/features/playbooks/api";

/** One file in the client-side draft of a manual project's file set.
 *  `id` is stable across renames so every operation addresses exactly one entry. */
export interface ProjectFileDraft {
  id: string;
  path: string;
  content: string;
  executable: boolean;
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

let draftFileSequence = 0;

function nextDraftFileId(): string {
  draftFileSequence += 1;
  return `draft-file-${draftFileSequence}`;
}

/** Clone the server read-back into an editable draft. */
export function filesToDraft(files: ProjectFileResponse[]): ProjectFileDraft[] {
  return files.map((file) => ({
    id: nextDraftFileId(),
    path: file.path,
    content: file.content,
    executable: file.executable,
  }));
}

/** Shape the draft for the whole-set replace request body. */
export function draftToUpload(files: ProjectFileDraft[]): UploadProjectFileRequest[] {
  return files.map((file) => ({
    path: file.path,
    content: file.content,
    executable: file.executable,
  }));
}

/** Order-sensitive equality of the persisted fields (draft ids are ignored). */
export function filesEqual(a: ProjectFileDraft[], b: ProjectFileDraft[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((file, index) => {
    const other = b[index];
    return (
      file.path === other?.path &&
      file.content === other?.content &&
      file.executable === other?.executable
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
 *  whole-set replace endpoint requires at least one file, so an empty draft is
 *  not saveable. */
export function draftIssue(
  files: ProjectFileDraft[],
): "blankPath" | "duplicatePath" | "empty" | null {
  if (files.length === 0) return "empty";
  if (files.some((file) => !file.path.trim())) return "blankPath";
  if (duplicatePaths(files).size > 0) return "duplicatePath";
  return null;
}

/**
 * Build the folder tree implied by file paths. Folders are derived from path
 * segments and merged by name; every file entry gets its own node so duplicated
 * paths stay visible and individually selectable.
 */
export function buildFileTree(files: ProjectFileDraft[]): ProjectFileTreeNode[] {
  const roots: ProjectFileTreeNode[] = [];
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
