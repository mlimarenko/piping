import { useCallback, useEffect, useId, useRef, useState } from "react";
import type { InputHTMLAttributes } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowUp,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Copy,
  CornerDownRight,
  Download,
  File,
  FileText,
  FileUp,
  Folder,
  FolderOpen,
  FolderUp,
  Home,
  Info,
  ListChecks,
  Loader2,
  MoveRight,
  PanelBottom,
  PanelRight,
  RefreshCcw,
  Trash2,
  Undo2,
  X,
  XCircle,
} from "lucide-react";
import { ApiError } from "@/shared/api/client";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui-primitives/alert-dialog";
import { Button } from "@/shared/ui-primitives/button";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import { Input } from "@/shared/ui-primitives/input";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";
import {
  deleteSshSessionFiles,
  ensureSshSessionDirectory,
  listSshSessionFiles,
  moveSshSessionFiles,
  preflightSshSessionFileDownload,
  previewSshSessionFile,
  startSshSessionFileDownload,
  type SshFileEntry,
  type SshFileList,
  type SshFilePreview,
  uploadSshSessionFile,
} from "@/features/connections/session-files-api";
import type { FileBrowserPlacement } from "@/features/connections/components/connections/fileBrowserState";

interface SshFileBrowserPanelProps {
  session: LiveSession;
  placement?: FileBrowserPlacement;
  onChangePlacement?: (placement: FileBrowserPlacement) => void;
  onClose?: () => void;
  className?: string;
}

type TransferStatus =
  | "uploading"
  | "copying"
  | "preparing"
  | "download-started"
  | "done"
  | "failed"
  | "cancelled";

interface TransferJob {
  id: string;
  kind: "upload" | "download";
  name: string;
  path: string;
  status: TransferStatus;
  progress?: number;
  loaded?: number;
  total?: number;
  summary?: string;
  abort?: () => void;
  error?: string;
}

type PreviewState =
  | { status: "idle" }
  | { status: "loading"; path: string }
  | { status: "loaded"; preview: SshFilePreview }
  | { status: "skipped"; reason: string }
  | { status: "error"; path: string; message: string };

interface LocalUploadFile {
  file: File;
  relativePath: string;
}

interface LocalUploadBatch {
  files: LocalUploadFile[];
  directories: string[];
}

interface PreparedUploadFile {
  file: File;
  remotePath: string;
  displayPath: string;
}

interface PreparedUploadBatch {
  files: PreparedUploadFile[];
  directories: string[];
}

interface FileSystemEntryLike {
  isFile: boolean;
  isDirectory: boolean;
  name: string;
  file?: (success: (file: File) => void, error?: (error: unknown) => void) => void;
  createReader?: () => {
    readEntries: (
      success: (entries: FileSystemEntryLike[]) => void,
      error?: (error: unknown) => void,
    ) => void;
  };
}

interface DataTransferItemWithEntry extends DataTransferItem {
  getAsEntry?: () => FileSystemEntryLike | null;
}

interface FileWithRelativePath extends File {
  webkitRelativePath: string;
}

interface FileSystemHandleLike {
  kind: "file" | "directory";
  name: string;
}

interface FileSystemFileHandleLike extends FileSystemHandleLike {
  kind: "file";
  getFile: () => Promise<File>;
}

interface FileSystemDirectoryHandleLike extends FileSystemHandleLike {
  kind: "directory";
  values: () => AsyncIterable<FileSystemHandleLike>;
}

interface DirectoryPickerWindow extends Window {
  showDirectoryPicker?: () => Promise<FileSystemDirectoryHandleLike>;
}

const MAX_PARALLEL_FILE_UPLOADS = 2;
const MAX_PARALLEL_DIR_CREATES = 4;
const EMPTY_ENTRIES: SshFileEntry[] = [];
const INVALID_RELATIVE_SEGMENT_CHARS = new Set(["\\", "/", ":", "*", "?", '"', "<", ">", "|"]);
const TEXT_FILE_EXTENSIONS = new Set([
  ".bashrc",
  ".conf",
  ".cfg",
  ".css",
  ".csv",
  ".env",
  ".ini",
  ".js",
  ".json",
  ".log",
  ".md",
  ".rs",
  ".sh",
  ".sql",
  ".toml",
  ".ts",
  ".tsx",
  ".txt",
  ".xml",
  ".yaml",
  ".yml",
]);
const BINARY_FILE_EXTENSIONS = new Set([
  ".7z",
  ".avi",
  ".bmp",
  ".class",
  ".deb",
  ".dll",
  ".doc",
  ".docx",
  ".exe",
  ".gif",
  ".gz",
  ".ico",
  ".iso",
  ".jar",
  ".jpg",
  ".jpeg",
  ".mov",
  ".mp3",
  ".mp4",
  ".pdf",
  ".png",
  ".ppt",
  ".pptx",
  ".tar",
  ".tgz",
  ".webp",
  ".xls",
  ".xlsx",
  ".zip",
]);
const UNKNOWN_TEXT_PREVIEW_MAX_BYTES = 1024 * 1024;

const directoryInputProps = {
  directory: "",
  webkitdirectory: "",
} as InputHTMLAttributes<HTMLInputElement> & Record<string, string>;

async function runWithConcurrency<T>(
  items: T[],
  limit: number,
  worker: (item: T) => Promise<void>,
): Promise<void> {
  const queue = [...items];
  const workerCount = Math.min(Math.max(1, limit), queue.length);
  await Promise.all(
    Array.from({ length: workerCount }, async () => {
      while (queue.length > 0) {
        const item = queue.shift();
        if (item === undefined) return;
        await worker(item);
      }
    }),
  );
}

function remoteJoinSegments(directory: string, segments: string[]): string {
  const cleanSegments = segments.filter(Boolean);
  if (cleanSegments.length === 0) return directory || ".";
  const suffix = cleanSegments.join("/");
  if (directory === "/") return `/${suffix}`;
  return `${directory.replace(/\/+$/, "")}/${suffix}`;
}

function normalizeRemotePathKey(path: string): string {
  const trimmed = path.trim() || ".";
  if (trimmed === "/") return "/";
  return trimmed.replace(/\/+$/, "") || "/";
}

function sanitizeRelativeSegment(segment: string): string {
  const normalized = Array.from(segment.normalize("NFC").trim(), (char) => {
    const codePoint = char.codePointAt(0) ?? 0;
    return INVALID_RELATIVE_SEGMENT_CHARS.has(char) || codePoint < 0x20 || codePoint === 0x7f
      ? "_"
      : char;
  }).join("");
  if (!normalized || normalized === "." || normalized === "..") return "_";
  return normalized;
}

function sanitizeRelativeSegments(path: string, fallbackName: string): string[] {
  const rawSegments = path
    .replaceAll("\\", "/")
    .split("/")
    .map((part) => part.trim())
    .filter(Boolean);
  const segments = (rawSegments.length > 0 ? rawSegments : [fallbackName]).map(sanitizeRelativeSegment);
  return segments.length > 0 ? segments : [sanitizeRelativeSegment(fallbackName)];
}

function parentDirectorySegments(segments: string[]): string[][] {
  const directories: string[][] = [];
  for (let index = 1; index < segments.length; index += 1) {
    directories.push(segments.slice(0, index));
  }
  return directories;
}

function buildUploadBatchFromFiles(files: File[]): LocalUploadBatch {
  return {
    files: files.map((file) => {
      const fileWithPath = file as FileWithRelativePath;
      return {
        file,
        relativePath: fileWithPath.webkitRelativePath || file.name,
      };
    }),
    directories: [],
  };
}

function prepareUploadBatch(batch: LocalUploadBatch, basePath: string): PreparedUploadBatch {
  const filePaths = new Set<string>();
  const directories = new Set<string>();
  const preparedFiles: PreparedUploadFile[] = [];

  for (const directory of batch.directories) {
    const segments = sanitizeRelativeSegments(directory, "folder");
    directories.add(remoteJoinSegments(basePath, segments));
    for (const parent of parentDirectorySegments(segments)) {
      directories.add(remoteJoinSegments(basePath, parent));
    }
  }

  for (const item of batch.files) {
    const segments = sanitizeRelativeSegments(item.relativePath, item.file.name || "upload.bin");
    const remotePath = remoteJoinSegments(basePath, segments);
    if (filePaths.has(remotePath)) {
      throw new Error(remotePath);
    }
    filePaths.add(remotePath);
    for (const parent of parentDirectorySegments(segments)) {
      directories.add(remoteJoinSegments(basePath, parent));
    }
    preparedFiles.push({
      file: item.file,
      remotePath,
      displayPath: segments.join("/"),
    });
  }

  return {
    files: preparedFiles,
    directories: Array.from(directories).sort((left, right) => left.localeCompare(right)),
  };
}

function readEntryFile(entry: FileSystemEntryLike): Promise<File> {
  if (!entry.file) return Promise.reject(new Error("File entry is not readable"));
  return new Promise((resolve, reject) => {
    entry.file?.(resolve, reject);
  });
}

async function readDirectoryEntries(entry: FileSystemEntryLike): Promise<FileSystemEntryLike[]> {
  const reader = entry.createReader?.();
  if (!reader) return [];
  const entries: FileSystemEntryLike[] = [];
  for (;;) {
    const chunk = await new Promise<FileSystemEntryLike[]>((resolve, reject) => {
      reader.readEntries(resolve, reject);
    });
    if (chunk.length === 0) break;
    entries.push(...chunk);
  }
  return entries;
}

async function collectEntryUploadBatch(
  entry: FileSystemEntryLike,
  parentPath: string,
  batch: LocalUploadBatch,
): Promise<void> {
  const relativePath = parentPath ? `${parentPath}/${entry.name}` : entry.name;
  if (entry.isDirectory) {
    batch.directories.push(relativePath);
    const children = await readDirectoryEntries(entry);
    await Promise.all(children.map((child) => collectEntryUploadBatch(child, relativePath, batch)));
    return;
  }
  if (entry.isFile) {
    const file = await readEntryFile(entry);
    batch.files.push({ file, relativePath });
  }
}

async function collectDroppedUploadBatch(dataTransfer: DataTransfer): Promise<LocalUploadBatch> {
  const items = Array.from(dataTransfer.items ?? []) as DataTransferItemWithEntry[];
  const entries = items
    .map(
      (item) =>
        item.getAsEntry?.() ??
        ((item.webkitGetAsEntry?.() ?? null) as FileSystemEntryLike | null),
    )
    .filter((entry): entry is FileSystemEntryLike => entry !== null);
  if (entries.length === 0) {
    return buildUploadBatchFromFiles(Array.from(dataTransfer.files ?? []));
  }
  const batch: LocalUploadBatch = { files: [], directories: [] };
  await Promise.all(entries.map((entry) => collectEntryUploadBatch(entry, "", batch)));
  return batch;
}

async function collectDirectoryHandleUploadBatch(
  handle: FileSystemDirectoryHandleLike,
  parentPath = "",
): Promise<LocalUploadBatch> {
  const relativePath = parentPath ? `${parentPath}/${handle.name}` : handle.name;
  const batch: LocalUploadBatch = { files: [], directories: [relativePath] };
  for await (const child of handle.values()) {
    if (child.kind === "directory") {
      const childBatch = await collectDirectoryHandleUploadBatch(
        child as FileSystemDirectoryHandleLike,
        relativePath,
      );
      batch.directories.push(...childBatch.directories);
      batch.files.push(...childBatch.files);
    } else if (child.kind === "file") {
      const file = await (child as FileSystemFileHandleLike).getFile();
      batch.files.push({ file, relativePath: `${relativePath}/${child.name}` });
    }
  }
  return batch;
}

function formatBytes(value?: number | null): string {
  if (value === undefined || value === null) return "-";
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let size = value / 1024;
  let unitIndex = 0;
  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }
  return `${size.toFixed(size >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}

function formatDate(value?: string | null): string {
  if (!value) return "-";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "-";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "short",
    timeStyle: "short",
  }).format(date);
}

function lowerFileExtension(name: string): string {
  const lowerName = name.toLowerCase();
  if (lowerName.startsWith(".") && lowerName.indexOf(".", 1) === -1) return lowerName;
  const dotIndex = lowerName.lastIndexOf(".");
  return dotIndex >= 0 ? lowerName.slice(dotIndex) : "";
}

function shouldAttemptTextPreview(entry: SshFileEntry): boolean {
  if (entry.kind !== "file") return false;
  const extension = lowerFileExtension(entry.name);
  if (TEXT_FILE_EXTENSIONS.has(extension)) return true;
  if (BINARY_FILE_EXTENSIONS.has(extension)) return false;
  return (entry.size ?? 0) <= UNKNOWN_TEXT_PREVIEW_MAX_BYTES;
}

function statusIcon(status: TransferStatus) {
  if (status === "failed") return XCircle;
  if (status === "uploading" || status === "copying" || status === "preparing") return Loader2;
  return CheckCircle2;
}

const SshFileBrowserPanel = ({
  session,
  placement = "right",
  onChangePlacement,
  onClose,
  className,
}: SshFileBrowserPanelProps) => {
  const { t } = useTranslation();
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const directoryInputRef = useRef<HTMLInputElement | null>(null);
  const loadSequenceRef = useRef(0);
  const loadPathRef = useRef<((path: string) => Promise<void>) | null>(null);
  const previewSequenceRef = useRef(0);
  const targetPathRef = useRef(".");
  const displayedPathRef = useRef(".");
  const pendingUploadRefreshPathRef = useRef<string | null>(null);
  const transferListId = useId();
  const [currentPath, setCurrentPath] = useState(".");
  const [pathDraft, setPathDraft] = useState(".");
  const [searchDraft, setSearchDraft] = useState("");
  const [listing, setListing] = useState<SshFileList | null>(null);
  const [loading, setLoading] = useState(false);
  const [dragActive, setDragActive] = useState(false);
  const [transfersExpanded, setTransfersExpanded] = useState(false);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(() => new Set());
  const [remotePreviewState, setRemotePreviewState] = useState<PreviewState>({ status: "idle" });
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [moveDialogOpen, setMoveDialogOpen] = useState(false);
  const [moveDestination, setMoveDestination] = useState(".");
  const [movingSelection, setMovingSelection] = useState(false);
  const [deletingSelection, setDeletingSelection] = useState(false);
  const [jobs, setJobs] = useState<TransferJob[]>([]);

  const updateJob = useCallback((id: string, patch: Partial<TransferJob>) => {
    setJobs((items) => items.map((item) => (item.id === id ? { ...item, ...patch } : item)));
  }, []);

  const describeTransferError = useCallback(
    (error: unknown) => {
      if (error instanceof Error && error.name === "AbortError") {
        return t("connections.toolsMenu.uploadCancelled");
      }
      if (error instanceof ApiError) {
        if (error.code === "PAYLOAD_TOO_LARGE") return t("connections.toolsMenu.transferTooLarge");
        if (error.code === "TOO_MANY_REQUESTS") return t("connections.toolsMenu.transferBusy");
      }
      return error instanceof Error ? error.message : t("connections.toolsMenu.transferFailed");
    },
    [t],
  );

  const loadPath = useCallback(
    async (path: string) => {
      const requestedPath = path.trim() || ".";
      const requestId = loadSequenceRef.current + 1;
      loadSequenceRef.current = requestId;
      targetPathRef.current = normalizeRemotePathKey(requestedPath);
      setLoading(true);
      try {
        const next = await listSshSessionFiles(session.id, requestedPath);
        if (loadSequenceRef.current !== requestId) return;
        const nextPath = next.path || requestedPath;
        const nextPathKey = normalizeRemotePathKey(nextPath);
        setListing({
          ...next,
          path: nextPath,
          entries: Array.isArray(next.entries) ? next.entries : [],
        });
        setSelectedPath(null);
        setSelectedPaths(new Set());
        setSelectionMode(false);
        setRemotePreviewState({ status: "idle" });
        targetPathRef.current = nextPathKey;
        displayedPathRef.current = nextPathKey;
        if (pendingUploadRefreshPathRef.current) {
          const pendingRefreshPath = pendingUploadRefreshPathRef.current;
          pendingUploadRefreshPathRef.current = null;
          if (normalizeRemotePathKey(pendingRefreshPath) === nextPathKey) {
            window.setTimeout(() => {
              void loadPathRef.current?.(pendingRefreshPath);
            }, 0);
          }
        }
        setCurrentPath(nextPath);
        setPathDraft(nextPath);
      } catch (error) {
        if (loadSequenceRef.current !== requestId) return;
        targetPathRef.current = displayedPathRef.current;
        const message =
          error instanceof Error ? error.message : t("connections.toolsMenu.fileBrowserLoadFailed");
        notify("error", t("connections.toolsMenu.fileBrowserLoadFailed"), { description: message });
        if (
          pendingUploadRefreshPathRef.current &&
          normalizeRemotePathKey(pendingUploadRefreshPathRef.current) === displayedPathRef.current
        ) {
          const pendingRefreshPath = pendingUploadRefreshPathRef.current;
          pendingUploadRefreshPathRef.current = null;
          window.setTimeout(() => {
            void loadPathRef.current?.(pendingRefreshPath);
          }, 0);
        }
      } finally {
        if (loadSequenceRef.current === requestId) setLoading(false);
      }
    },
    [session.id, t],
  );

  useEffect(() => {
    loadPathRef.current = loadPath;
    return () => {
      if (loadPathRef.current === loadPath) loadPathRef.current = null;
    };
  }, [loadPath]);

  useEffect(() => {
    const timeoutId = window.setTimeout(() => {
      void loadPath(".");
    }, 0);
    return () => {
      window.clearTimeout(timeoutId);
      loadSequenceRef.current += 1;
    };
  }, [loadPath]);

  const addJob = (job: Omit<TransferJob, "id">): string => {
    const id = `${Date.now().toString(16)}-${Math.random().toString(16).slice(2)}`;
    setJobs((items) => [{ ...job, id }, ...items].slice(0, 16));
    setTransfersExpanded(true);
    return id;
  };

  const refreshUploadDirectory = async (basePath: string, basePathKey: string) => {
    if (targetPathRef.current === basePathKey) {
      await loadPath(basePath);
    } else if (displayedPathRef.current === basePathKey) {
      pendingUploadRefreshPathRef.current = basePath;
    }
  };

  const uploadBatch = async (batch: LocalUploadBatch) => {
    if (batch.files.length === 0 && batch.directories.length === 0) return;
    const basePath = listing?.path ?? currentPath;
    const basePathKey = normalizeRemotePathKey(basePath);
    let prepared: PreparedUploadBatch;
    try {
      prepared = prepareUploadBatch(batch, basePath);
    } catch (error) {
      notify("error", t("connections.toolsMenu.uploadCollision"), {
        description:
          error instanceof Error ? error.message : t("connections.toolsMenu.uploadCollisionHint"),
      });
      return;
    }

    let directoryJobId: string | null = null;
    if (prepared.directories.length > 0) {
      directoryJobId = addJob({
        kind: "upload",
        name: t("connections.toolsMenu.directories"),
        path: basePath,
        status: "preparing",
        summary: t("connections.toolsMenu.creatingDirectories", {
          count: prepared.directories.length,
        }),
      });
      let created = 0;
      try {
        await runWithConcurrency(prepared.directories, MAX_PARALLEL_DIR_CREATES, async (path) => {
          await ensureSshSessionDirectory(session.id, path);
          created += 1;
          if (directoryJobId) {
            updateJob(directoryJobId, {
              summary: t("connections.toolsMenu.creatingDirectoriesProgress", {
                done: created,
                total: prepared.directories.length,
              }),
            });
          }
        });
        updateJob(directoryJobId, {
          status: "done",
          summary: t("connections.toolsMenu.directoriesReady", {
            count: prepared.directories.length,
          }),
        });
      } catch (error) {
        updateJob(directoryJobId, {
          status: "failed",
          error: describeTransferError(error),
        });
        return;
      }
    }

    await runWithConcurrency(prepared.files, MAX_PARALLEL_FILE_UPLOADS, async (item) => {
      const controller = new AbortController();
      const id = addJob({
        kind: "upload",
        name: item.file.name,
        path: item.remotePath,
        status: "uploading",
        progress: 0,
        abort: () => controller.abort(),
        summary: item.displayPath,
      });
      try {
        await uploadSshSessionFile(session.id, item.remotePath, item.file, {
          signal: controller.signal,
          onProgress: (progress) => {
            updateJob(id, {
              loaded: progress.loaded,
              total: progress.total,
              progress: progress.percent,
            });
          },
          onRemoteCopy: () => {
            updateJob(id, { status: "copying", progress: 100, abort: undefined });
          },
        });
        updateJob(id, { status: "done", progress: 100, abort: undefined });
      } catch (error) {
        updateJob(id, {
          status: error instanceof Error && error.name === "AbortError" ? "cancelled" : "failed",
          abort: undefined,
          error: describeTransferError(error),
        });
      }
    });

    await refreshUploadDirectory(basePath, basePathKey);
  };

  const uploadFiles = async (files: File[]) => {
    await uploadBatch(buildUploadBatchFromFiles(files));
  };

  const uploadDirectoryFromPicker = async () => {
    const picker = (window as DirectoryPickerWindow).showDirectoryPicker;
    if (picker) {
      try {
        const handle = await picker.call(window);
        await uploadBatch(await collectDirectoryHandleUploadBatch(handle));
        return;
      } catch (error) {
        if (error instanceof Error && error.name === "AbortError") return;
        notify("error", t("connections.toolsMenu.folderUploadFailed"), {
          description: error instanceof Error ? error.message : undefined,
        });
      }
    }
    directoryInputRef.current?.click();
  };

  const downloadEntry = async (entry: SshFileEntry) => {
    const isDirectory = entry.kind === "directory";
    const id = addJob({
      kind: "download",
      name: entry.name,
      path: entry.path,
      status: "preparing",
      summary: isDirectory ? t("connections.toolsMenu.archiveDownload") : undefined,
    });
    try {
      const preflight = await preflightSshSessionFileDownload(session.id, entry.path, {
        archive: isDirectory,
      });
      updateJob(id, {
        status: "download-started",
        total: preflight.bytes ?? preflight.logical_bytes ?? undefined,
        summary: isDirectory
          ? t("connections.toolsMenu.archiveReady", {
              count: preflight.entry_count ?? 0,
              size: formatBytes(preflight.logical_bytes),
            })
          : undefined,
      });
      startSshSessionFileDownload(session.id, entry.path, preflight.file_name, {
        archive: isDirectory,
      });
    } catch (error) {
      updateJob(id, {
        status: "failed",
        error: describeTransferError(error),
      });
    }
  };

  const copySelectedPath = async (entry: SshFileEntry) => {
    try {
      await navigator.clipboard.writeText(entry.path);
      notify("success", t("connections.toolsMenu.pathCopied"));
    } catch (error) {
      notify("error", error instanceof Error ? error.message : t("connections.toolsMenu.transferFailed"));
    }
  };

  const toggleEntrySelection = (entry: SshFileEntry, checked?: boolean) => {
    setSelectedPath(entry.path);
    setSelectionMode(true);
    setSelectedPaths((current) => {
      const next = new Set(current);
      const shouldSelect = checked ?? !next.has(entry.path);
      if (shouldSelect) {
        next.add(entry.path);
      } else {
        next.delete(entry.path);
      }
      return next;
    });
  };

  const toggleFilteredSelection = (checked: boolean) => {
    setSelectedPaths((current) => {
      const next = new Set(current);
      for (const entry of filteredEntries) {
        if (checked) {
          next.add(entry.path);
        } else {
          next.delete(entry.path);
        }
      }
      return next;
    });
  };

  const clearSelection = () => {
    setSelectedPaths(new Set());
  };

  const exitSelectionMode = () => {
    setSelectionMode(false);
    setSelectedPaths(new Set());
  };

  const downloadSelectedEntries = async () => {
    const targets = selectedEntries.length > 0 ? selectedEntries : selectedEntry ? [selectedEntry] : [];
    for (const entry of targets) {
      await downloadEntry(entry);
    }
  };

  const openMoveSelectionDialog = () => {
    setMoveDestination(listing?.path ?? currentPath);
    setMoveDialogOpen(true);
  };

  const selectEntryForAction = (entry: SshFileEntry) => {
    setSelectedPath(entry.path);
    setSelectionMode(true);
    setSelectedPaths(new Set([entry.path]));
  };

  const openMoveEntryDialog = (entry: SshFileEntry) => {
    selectEntryForAction(entry);
    setMoveDestination(listing?.path ?? currentPath);
    setMoveDialogOpen(true);
  };

  const openDeleteEntryDialog = (entry: SshFileEntry) => {
    selectEntryForAction(entry);
    setDeleteDialogOpen(true);
  };

  const moveSelectedEntries = async () => {
    const paths = selectedEntries.map((entry) => entry.path);
    if (paths.length === 0) return;
    const destination = moveDestination.trim();
    if (!destination) return;
    setMovingSelection(true);
    try {
      await moveSshSessionFiles(session.id, paths, destination);
      notify("success", t("connections.toolsMenu.moveDone", { count: paths.length }));
      setMoveDialogOpen(false);
      setSelectedPaths(new Set());
      setSelectionMode(false);
      setSelectedPath(null);
      await loadPath(currentPath);
    } catch (error) {
      notify("error", t("connections.toolsMenu.moveFailed"), {
        description: error instanceof Error ? error.message : undefined,
      });
    } finally {
      setMovingSelection(false);
    }
  };

  const deleteSelectedEntries = async () => {
    const paths = selectedEntries.map((entry) => entry.path);
    if (paths.length === 0) return;
    setDeletingSelection(true);
    try {
      await deleteSshSessionFiles(session.id, paths);
      notify("success", t("connections.toolsMenu.deleteDone", { count: paths.length }));
      setDeleteDialogOpen(false);
      setSelectedPaths(new Set());
      setSelectionMode(false);
      setSelectedPath(null);
      await loadPath(currentPath);
    } catch (error) {
      notify("error", t("connections.toolsMenu.deleteFailed"), {
        description: error instanceof Error ? error.message : undefined,
      });
    } finally {
      setDeletingSelection(false);
    }
  };

  const goToDraftPath = () => {
    const next = pathDraft.trim() || ".";
    void loadPath(next);
  };

  const entries = listing?.entries ?? EMPTY_ENTRIES;
  const selectedEntry = entries.find((entry) => entry.path === selectedPath) ?? null;
  const selectedEntries = entries.filter((entry) => selectedPaths.has(entry.path));
  const normalizedSearch = searchDraft.trim().toLowerCase();
  const filteredEntries =
    normalizedSearch.length === 0
      ? entries
      : entries.filter((entry) =>
          [entry.name, entry.path, entry.kind, entry.permissions ?? ""]
            .join(" ")
            .toLowerCase()
            .includes(normalizedSearch),
        );
  const allFilteredSelected =
    filteredEntries.length > 0 && filteredEntries.every((entry) => selectedPaths.has(entry.path));
  const fileGridClassName = selectionMode
    ? "grid-cols-[1.75rem_minmax(0,1fr)_5.25rem_5rem]"
    : "grid-cols-[minmax(0,1fr)_5.25rem_5rem]";

  const activeTransferCount = jobs.filter((job) =>
    job.status === "uploading" || job.status === "copying" || job.status === "preparing",
  ).length;

  const previewState: PreviewState = (() => {
    if (!selectedEntry) return { status: "idle" };
    if (selectedEntry.kind !== "file") {
      return { status: "skipped", reason: t("connections.toolsMenu.previewDirectories") };
    }
    if (!shouldAttemptTextPreview(selectedEntry)) {
      return { status: "skipped", reason: t("connections.toolsMenu.previewTextOnly") };
    }
    const statePath =
      remotePreviewState.status === "loaded"
        ? remotePreviewState.preview.path
        : remotePreviewState.status === "loading" || remotePreviewState.status === "error"
          ? remotePreviewState.path
          : null;
    return statePath === selectedEntry.path
      ? remotePreviewState
      : { status: "loading", path: selectedEntry.path };
  })();

  useEffect(() => {
    const entry = selectedEntry;
    const sequence = previewSequenceRef.current + 1;
    previewSequenceRef.current = sequence;
    if (!entry || entry.kind !== "file" || !shouldAttemptTextPreview(entry)) return;
    void previewSshSessionFile(session.id, entry.path)
      .then((preview) => {
        if (previewSequenceRef.current === sequence) {
          setRemotePreviewState({ status: "loaded", preview });
        }
      })
      .catch((error: unknown) => {
        if (previewSequenceRef.current !== sequence) return;
        const message =
          error instanceof ApiError && error.status === 400
            ? t("connections.toolsMenu.previewTextOnly")
            : error instanceof Error
              ? error.message
              : t("connections.toolsMenu.previewFailed");
        setRemotePreviewState({ status: "error", path: entry.path, message });
      });
  }, [selectedEntry, session.id, t]);

  const transferStatusLabel = (job: TransferJob): string => {
    switch (job.status) {
      case "uploading":
        return job.progress === undefined
          ? t("connections.toolsMenu.uploading")
          : t("connections.toolsMenu.uploadingPercent", { percent: job.progress });
      case "copying":
        return t("connections.toolsMenu.copyingToHost");
      case "preparing":
        return job.kind === "download"
          ? t("connections.toolsMenu.preparingDownload")
          : t("connections.toolsMenu.preparingUpload");
      case "download-started":
        return t("connections.toolsMenu.browserDownloadStarted");
      case "done":
        return t("connections.toolsMenu.transferDone");
      case "cancelled":
        return t("connections.toolsMenu.uploadCancelled");
      case "failed":
        return t("connections.toolsMenu.transferFailed");
      default:
        return job.kind === "upload"
          ? t("connections.toolsMenu.uploading")
          : t("connections.toolsMenu.downloading");
    }
  };

  return (
    <>
      <section
      className={cn(
        "flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-background",
        className,
      )}
      aria-label={t("connections.toolsMenu.fileBrowserTitle")}
    >
      <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border/60 px-2 py-1.5">
        <Button
          variant="ghost"
          size="icon-sm"
          className="h-8 w-8"
          onClick={() => void loadPath(".")}
          title={t("connections.toolsMenu.home")}
          aria-label={t("connections.toolsMenu.home")}
        >
          <Home className="h-4 w-4" />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className="h-8 w-8"
          onClick={() => listing?.parent && void loadPath(listing.parent)}
          disabled={!listing?.parent}
          title={t("connections.toolsMenu.up")}
          aria-label={t("connections.toolsMenu.up")}
        >
          <ArrowUp className="h-4 w-4" />
        </Button>
        <div className="min-w-[11rem] flex-[2_1_18rem]">
          <Input
            value={pathDraft}
            onChange={(event) => setPathDraft(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") goToDraftPath();
            }}
            aria-label={t("connections.toolsMenu.remotePath")}
            className="h-8 font-mono text-sm"
          />
        </div>
        <Button
          variant="outline"
          size="icon-sm"
          className="h-8 w-8"
          onClick={goToDraftPath}
          title={t("connections.toolsMenu.go")}
          aria-label={t("connections.toolsMenu.go")}
        >
          <CornerDownRight className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          className="h-8 w-8"
          onClick={() => void loadPath(currentPath)}
          title={t("connections.toolsMenu.refresh")}
          aria-label={t("connections.toolsMenu.refresh")}
        >
          <RefreshCcw className={cn("h-4 w-4", loading && "animate-spin")} />
        </Button>
        <SearchInput
          value={searchDraft}
          onValueChange={setSearchDraft}
          aria-label={t("connections.toolsMenu.searchFiles")}
          placeholder={t("connections.toolsMenu.searchFiles")}
          className="border-input bg-background"
          containerClassName="min-w-[10rem] flex-[1_1_13rem]"
        />
        <Button
          variant="default"
          size="icon-sm"
          className="h-8 w-8 shrink-0"
          onClick={() => fileInputRef.current?.click()}
          title={t("connections.toolsMenu.uploadFiles")}
          aria-label={t("connections.toolsMenu.uploadFiles")}
        >
          <FileUp className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="outline"
          size="icon-sm"
          className="h-8 w-8 shrink-0"
          onClick={() => void uploadDirectoryFromPicker()}
          title={t("connections.toolsMenu.uploadFolder")}
          aria-label={t("connections.toolsMenu.uploadFolder")}
        >
          <FolderUp className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant={selectionMode ? "secondary" : "outline"}
          size="icon-sm"
          className="h-8 w-8 shrink-0"
          onClick={() => {
            if (selectionMode) {
              exitSelectionMode();
            } else {
              setSelectionMode(true);
            }
          }}
          title={
            selectionMode
              ? t("connections.toolsMenu.exitSelection")
              : t("connections.toolsMenu.enableSelection")
          }
          aria-label={
            selectionMode
              ? t("connections.toolsMenu.exitSelection")
              : t("connections.toolsMenu.enableSelection")
          }
        >
          <ListChecks className="h-3.5 w-3.5" />
        </Button>
        {selectionMode ? (
          <div className="flex min-w-0 shrink-0 items-center gap-1 rounded-md border border-border/60 bg-muted px-1 py-0.5">
            <span className="px-1 text-xs font-medium text-muted-foreground">
              {t("connections.toolsMenu.selectedCount", { count: selectedEntries.length })}
            </span>
            <Button
              variant="ghost"
              size="icon-sm"
              className="h-8 w-8"
              onClick={() => void downloadSelectedEntries()}
              disabled={selectedEntries.length === 0}
              title={t("connections.toolsMenu.downloadSelected")}
              aria-label={t("connections.toolsMenu.downloadSelected")}
            >
              <Download className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              className="h-8 w-8"
              onClick={openMoveSelectionDialog}
              disabled={selectedEntries.length === 0}
              title={t("connections.toolsMenu.moveSelected")}
              aria-label={t("connections.toolsMenu.moveSelected")}
            >
              <MoveRight className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              className="h-8 w-8 text-status-negative hover:text-status-negative"
              onClick={() => setDeleteDialogOpen(true)}
              disabled={selectedEntries.length === 0}
              title={t("connections.toolsMenu.deleteSelected")}
              aria-label={t("connections.toolsMenu.deleteSelected")}
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              className="h-8 w-8"
              onClick={selectedEntries.length > 0 ? clearSelection : exitSelectionMode}
              title={
                selectedEntries.length > 0
                  ? t("connections.toolsMenu.clearSelection")
                  : t("connections.toolsMenu.exitSelection")
              }
              aria-label={
                selectedEntries.length > 0
                  ? t("connections.toolsMenu.clearSelection")
                  : t("connections.toolsMenu.exitSelection")
              }
            >
              <Undo2 className="h-3.5 w-3.5" />
            </Button>
          </div>
        ) : null}
        {onChangePlacement ? (
          <div className="ml-auto flex shrink-0 items-center rounded-md border border-border/60 bg-muted p-0.5">
            <Button
              variant={placement === "right" ? "secondary" : "ghost"}
              size="icon-sm"
              className="h-8 w-8"
              onClick={() => onChangePlacement("right")}
              title={t("connections.toolsMenu.placeRight")}
              aria-label={t("connections.toolsMenu.placeRight")}
            >
              <PanelRight className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant={placement === "bottom" ? "secondary" : "ghost"}
              size="icon-sm"
              className="h-8 w-8"
              onClick={() => onChangePlacement("bottom")}
              title={t("connections.toolsMenu.placeBottom")}
              aria-label={t("connections.toolsMenu.placeBottom")}
            >
              <PanelBottom className="h-3.5 w-3.5" />
            </Button>
          </div>
        ) : null}
        {onClose ? (
          <Button
            variant="ghost"
            size="icon-sm"
            className="h-8 w-8"
            onClick={onClose}
            title={t("close")}
            aria-label={t("close")}
          >
            <X className="h-4 w-4" />
          </Button>
        ) : null}
        <input
          ref={fileInputRef}
          type="file"
          multiple
          className="hidden"
          onChange={(event) => {
            const files = Array.from(event.currentTarget.files ?? []);
            event.currentTarget.value = "";
            void uploadFiles(files);
          }}
        />
        <input
          ref={directoryInputRef}
          {...directoryInputProps}
          type="file"
          multiple
          className="hidden"
          onChange={(event) => {
            const files = Array.from(event.currentTarget.files ?? []);
            event.currentTarget.value = "";
            void uploadFiles(files);
          }}
        />
      </div>

      <div
        className={cn(
          "grid min-h-0 flex-1 overflow-hidden",
          selectedEntry && placement === "bottom" && "grid-cols-[minmax(0,1fr)_18rem]",
          selectedEntry && placement === "right" && "grid-rows-[minmax(0,1fr)_auto]",
        )}
      >
        <div className="flex min-h-0 min-w-0 flex-col overflow-hidden">
          <div
            className={cn(
              "relative min-h-0 flex-1 overflow-auto px-2 py-1.5",
              dragActive && "bg-primary/5 outline outline-1 outline-primary/40",
            )}
            onDragOver={(event) => {
              event.preventDefault();
              setDragActive(true);
            }}
            onDragLeave={() => setDragActive(false)}
            onDrop={(event) => {
              event.preventDefault();
              setDragActive(false);
              void collectDroppedUploadBatch(event.dataTransfer)
                .then(uploadBatch)
                .catch((error: unknown) => {
                  notify("error", t("connections.toolsMenu.folderUploadFailed"), {
                    description: error instanceof Error ? error.message : undefined,
                  });
                });
            }}
          >
            {dragActive ? (
              <div className="pointer-events-none absolute inset-3 z-10 flex items-center justify-center rounded-md border border-dashed border-primary/60 bg-background text-sm font-medium text-foreground shadow-sm">
                {t("connections.toolsMenu.dropFilesHere")}
              </div>
            ) : null}
            <div
              className={cn(
                "grid items-center border-b border-border/60 px-2 pb-1.5 text-xs font-medium text-muted-foreground",
                fileGridClassName,
              )}
            >
              {selectionMode ? (
                <Checkbox
                  checked={allFilteredSelected}
                  disabled={filteredEntries.length === 0}
                  onCheckedChange={(checked) => toggleFilteredSelection(checked === true)}
                  aria-label={t("connections.toolsMenu.selectAll")}
                  className="h-3.5 w-3.5"
                />
              ) : null}
              <span>{t("connections.toolsMenu.name")}</span>
              <span>{t("connections.toolsMenu.size")}</span>
              <span className="sr-only">{t("connections.toolsMenu.actions")}</span>
            </div>
            {loading ? (
              <div className="flex h-28 items-center justify-center">
                <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
              </div>
            ) : filteredEntries.length === 0 ? (
              <div className="flex h-28 items-center justify-center text-xs text-muted-foreground">
                {entries.length === 0
                  ? t("connections.toolsMenu.fileBrowserEmpty")
                  : t("connections.toolsMenu.fileBrowserNoMatches")}
              </div>
            ) : (
              <div>
                {filteredEntries.map((entry) => {
                  const isDirectory = entry.kind === "directory";
                  const Icon = isDirectory ? Folder : File;
                  const selected = selectedEntry?.path === entry.path;
                  const selectedForAction = selectedPaths.has(entry.path);
                  return (
                    <div
                      key={entry.path}
                      className={cn(
                        "grid min-h-9 items-center gap-1.5 rounded-md px-2 py-1 text-sm hover:bg-muted",
                        fileGridClassName,
                        selectedForAction && "bg-muted",
                        selected && "bg-primary/10 text-primary hover:bg-primary/10",
                      )}
                      role="row"
                      aria-selected={selected}
                      aria-label={entry.name}
                      tabIndex={0}
                      onClick={() => {
                        if (selectionMode) {
                          toggleEntrySelection(entry);
                          return;
                        }
                        setSelectedPath(entry.path);
                      }}
                      onKeyDown={(event) => {
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          if (selectionMode) {
                            toggleEntrySelection(entry);
                            return;
                          }
                          setSelectedPath(entry.path);
                        }
                      }}
                      onDoubleClick={() => {
                        if (isDirectory && !selectionMode) void loadPath(entry.path);
                      }}
                    >
                      {selectionMode ? (
                        <Checkbox
                          checked={selectedForAction}
                          onCheckedChange={(checked) => toggleEntrySelection(entry, checked === true)}
                          onClick={(event) => event.stopPropagation()}
                          aria-label={t("connections.toolsMenu.selectEntry", { name: entry.name })}
                          className="h-3.5 w-3.5"
                        />
                      ) : null}
                      <div className="flex min-w-0 items-center gap-2">
                        <Icon className={cn("h-4 w-4 shrink-0", isDirectory && "text-primary")} />
                        <span className="min-w-0">
                          <span className="block truncate">{entry.name}</span>
                          <span className="block truncate text-xs leading-3 text-muted-foreground">
                            {formatDate(entry.modified_at)}
                          </span>
                        </span>
                      </div>
                      <span className="truncate text-xs text-muted-foreground">
                        {formatBytes(entry.size)}
                      </span>
                      <span className="flex justify-end gap-0.5">
                        {isDirectory ? (
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            className="h-8 w-8"
                            onClick={(event) => {
                              event.stopPropagation();
                              void loadPath(entry.path);
                            }}
                            title={t("connections.toolsMenu.openFolder")}
                            aria-label={t("connections.toolsMenu.openFolder")}
                          >
                            <FolderOpen className="h-4 w-4" />
                          </Button>
                        ) : null}
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          className="h-8 w-8"
                          onClick={(event) => {
                            event.stopPropagation();
                            void downloadEntry(entry);
                          }}
                          title={
                            isDirectory
                              ? t("connections.toolsMenu.downloadFolder")
                              : t("connections.toolsMenu.download")
                          }
                          aria-label={
                            isDirectory
                              ? t("connections.toolsMenu.downloadFolder")
                              : t("connections.toolsMenu.download")
                          }
                        >
                          <Download className="h-4 w-4" />
                        </Button>
                      </span>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          <aside className={cn("shrink-0 border-t border-border/60", jobs.length === 0 && "hidden")}>
            <button
              type="button"
              className="flex h-8 w-full items-center gap-2 px-2.5 text-left text-sm text-muted-foreground hover:bg-muted"
              aria-expanded={transfersExpanded}
              aria-controls={transferListId}
              onClick={() => setTransfersExpanded((open) => !open)}
            >
              {transfersExpanded ? (
                <ChevronDown className="h-3.5 w-3.5 shrink-0" />
              ) : (
                <ChevronUp className="h-3.5 w-3.5 shrink-0" />
              )}
              <span className="font-medium">{t("connections.toolsMenu.transfers")}</span>
              <span className="min-w-0 flex-1 truncate">
                {jobs.length === 0
                  ? t("connections.toolsMenu.transferEmpty")
                  : activeTransferCount > 0
                    ? t("connections.toolsMenu.transferActiveCount", { count: activeTransferCount })
                    : t("connections.toolsMenu.transferCount", { count: jobs.length })}
              </span>
            </button>
            <div
              id={transferListId}
              hidden={!transfersExpanded || jobs.length === 0}
              className="max-h-24 space-y-0.5 overflow-auto px-2 pb-1.5"
            >
              {transfersExpanded && jobs.length > 0
                ? jobs.map((job) => {
                    const StatusIcon = statusIcon(job.status);
                    const isRunning =
                      job.status === "uploading" ||
                      job.status === "copying" ||
                      job.status === "preparing";
                    const total = job.total ? formatBytes(job.total) : "";
                    return (
                      <div
                        key={job.id}
                        className="relative grid min-h-8 grid-cols-[auto_minmax(7rem,1fr)_minmax(10rem,1.5fr)_auto] items-center gap-2 rounded-md border border-border/60 px-2 py-0.5 text-sm"
                      >
                        <StatusIcon
                          className={cn(
                            "h-3.5 w-3.5 shrink-0",
                            isRunning && "animate-spin text-muted-foreground",
                            (job.status === "done" || job.status === "download-started") &&
                              "text-status-positive",
                            job.status === "failed" && "text-status-negative",
                            job.status === "cancelled" && "text-muted-foreground",
                          )}
                        />
                        <span className="min-w-0 truncate font-medium">{job.name}</span>
                        <span className="min-w-0 truncate text-muted-foreground">
                          {job.summary ?? transferStatusLabel(job)}
                          {total ? ` · ${total}` : ""}
                          {" · "}
                          {job.path}
                        </span>
                        {job.abort ? (
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            className="h-6 w-6"
                            onClick={job.abort}
                            title={t("connections.toolsMenu.cancelTransfer")}
                            aria-label={t("connections.toolsMenu.cancelTransfer")}
                          >
                            <X className="h-3.5 w-3.5" />
                          </Button>
                        ) : (
                          <span className="h-6 w-6" />
                        )}
                        {job.progress !== undefined && job.kind === "upload" ? (
                          <div className="absolute inset-x-1 bottom-0 h-0.5 overflow-hidden rounded-full bg-muted">
                            <div
                              className="h-full rounded-full bg-primary transition-[width]"
                              style={{ width: `${Math.max(0, Math.min(100, job.progress))}%` }}
                            />
                          </div>
                        ) : null}
                        {job.error ? (
                          <div className="col-span-full truncate text-status-negative">{job.error}</div>
                        ) : null}
                      </div>
                    );
                  })
                : null}
            </div>
          </aside>
        </div>

        {selectedEntry ? (
          <aside
            className={cn(
              "min-w-0 overflow-auto border-border/60 bg-muted p-2 text-xs",
              placement === "bottom" ? "border-l" : "border-t",
            )}
            aria-label={t("connections.toolsMenu.details")}
          >
            <div className="mb-2 flex items-center gap-2">
              <Info className="h-4 w-4 shrink-0 text-primary" />
              <div className="min-w-0 flex-1 truncate font-medium">{selectedEntry.name}</div>
              <Button
                variant="ghost"
                size="icon-sm"
                className="h-8 w-8"
                onClick={() => setSelectedPath(null)}
                title={t("close")}
                aria-label={t("close")}
              >
                <X className="h-3.5 w-3.5" />
              </Button>
            </div>
            <div className="grid gap-1.5">
              <DetailRow label={t("connections.toolsMenu.type")} value={selectedEntry.kind} />
              <DetailRow label={t("connections.toolsMenu.size")} value={formatBytes(selectedEntry.size)} />
              <DetailRow
                label={t("connections.toolsMenu.modified")}
                value={formatDate(selectedEntry.modified_at)}
              />
              <DetailRow
                label={t("connections.toolsMenu.permissions")}
                value={selectedEntry.permissions ?? "-"}
              />
              <DetailRow label={t("connections.toolsMenu.path")} value={selectedEntry.path} mono />
            </div>
            <PreviewPane state={previewState} />
            <div className="mt-3 flex flex-wrap gap-1.5">
              {selectedEntry.kind === "directory" ? (
                <Button
                  variant="outline"
                  size="sm"
                  className="h-8 gap-1.5 px-2 text-sm"
                  onClick={() => void loadPath(selectedEntry.path)}
                >
                  <FolderOpen className="h-3.5 w-3.5" />
                  {t("connections.toolsMenu.openFolder")}
                </Button>
              ) : null}
              <Button
                variant="outline"
                size="sm"
                className="h-8 gap-1.5 px-2 text-sm"
                onClick={() => void downloadEntry(selectedEntry)}
              >
                <Download className="h-3.5 w-3.5" />
                {selectedEntry.kind === "directory"
                  ? t("connections.toolsMenu.downloadFolder")
                  : t("connections.toolsMenu.download")}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className="h-8 gap-1.5 px-2 text-sm"
                onClick={() => void copySelectedPath(selectedEntry)}
              >
                <Copy className="h-3.5 w-3.5" />
                {t("connections.toolsMenu.copyPath")}
              </Button>
              {!selectionMode ? (
                <>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-8 gap-1.5 px-2 text-sm"
                    onClick={() => openMoveEntryDialog(selectedEntry)}
                  >
                    <MoveRight className="h-3.5 w-3.5" />
                    {t("connections.toolsMenu.move")}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-8 gap-1.5 px-2 text-sm text-status-negative hover:text-status-negative"
                    onClick={() => openDeleteEntryDialog(selectedEntry)}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                    {t("connections.toolsMenu.delete")}
                  </Button>
                </>
              ) : null}
            </div>
          </aside>
        ) : null}
      </div>
      </section>
      <Dialog open={moveDialogOpen} onOpenChange={setMoveDialogOpen}>
        <DialogContent
          className="max-w-md gap-3 p-4"
          description={t("connections.toolsMenu.moveSelectedDescription", {
            count: selectedEntries.length,
          })}
        >
          <DialogHeader>
            <DialogTitle className="text-base">
              {t("connections.toolsMenu.moveSelectedTitle")}
            </DialogTitle>
            <DialogDescription>
              {t("connections.toolsMenu.moveSelectedDescription", {
                count: selectedEntries.length,
              })}
            </DialogDescription>
          </DialogHeader>
          <Input
            value={moveDestination}
            onChange={(event) => setMoveDestination(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !movingSelection) {
                void moveSelectedEntries();
              }
            }}
            className="font-mono text-xs"
            aria-label={t("connections.toolsMenu.moveDestination")}
            placeholder={t("connections.toolsMenu.moveDestination")}
          />
          <DialogFooter>
            <Button variant="outline" onClick={() => setMoveDialogOpen(false)}>
              {t("cancel")}
            </Button>
            <Button onClick={() => void moveSelectedEntries()} disabled={movingSelection}>
              {movingSelection ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
              {t("connections.toolsMenu.move")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <AlertDialog open={deleteDialogOpen} onOpenChange={setDeleteDialogOpen}>
        <AlertDialogContent className="max-w-md gap-3 p-4">
          <AlertDialogHeader>
            <AlertDialogTitle className="text-base">
              {t("connections.toolsMenu.deleteSelectedTitle")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("connections.toolsMenu.deleteSelectedDescription", {
                count: selectedEntries.length,
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deletingSelection}>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction
              className="bg-status-negative text-white hover:bg-status-negative/90"
              disabled={deletingSelection}
              onClick={(event) => {
                event.preventDefault();
                void deleteSelectedEntries();
              }}
            >
              {deletingSelection ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
              {t("connections.toolsMenu.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
};

const PreviewPane = ({ state }: { state: PreviewState }) => {
  const { t } = useTranslation();
  if (state.status === "idle") return null;
  return (
    <div className="mt-3 rounded-md border border-border/60 bg-background">
      <div className="flex min-h-8 items-center gap-2 border-b border-border/60 px-2 text-sm font-medium text-muted-foreground">
        <FileText className="h-3.5 w-3.5 shrink-0 text-primary" />
        <span className="min-w-0 flex-1 truncate">{t("connections.toolsMenu.preview")}</span>
        {state.status === "loaded" ? (
          <span className="shrink-0">
            {formatBytes(state.preview.preview_bytes)}
            {state.preview.truncated ? ` · ${t("connections.toolsMenu.previewTruncated")}` : ""}
          </span>
        ) : null}
      </div>
      {state.status === "loading" ? (
        <div className="flex h-20 items-center justify-center text-xs text-muted-foreground">
          <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          {t("connections.toolsMenu.previewLoading")}
        </div>
      ) : state.status === "loaded" ? (
        <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-words p-2 font-mono text-xs leading-4 text-foreground">
          {state.preview.content || t("connections.toolsMenu.previewEmpty")}
        </pre>
      ) : (
        <div className="p-2 text-xs text-muted-foreground">
          {state.status === "skipped" ? state.reason : state.message}
        </div>
      )}
    </div>
  );
};

const DetailRow = ({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) => (
  <div className="grid grid-cols-[5rem_minmax(0,1fr)] items-start gap-2">
    <span className="text-muted-foreground">{label}</span>
    <span className={cn("min-w-0 break-words text-right", mono && "font-mono text-xs")}>
      {value}
    </span>
  </div>
);

export default SshFileBrowserPanel;
