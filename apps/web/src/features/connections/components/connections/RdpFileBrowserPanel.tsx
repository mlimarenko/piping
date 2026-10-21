import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowUp,
  Download,
  File,
  FileUp,
  Folder,
  FolderOpen,
  Home,
  Loader2,
  PanelBottom,
  PanelRight,
  RefreshCcw,
  X,
} from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";
import type {
  RdpFilesystem,
  RdpFsEntry,
} from "@/features/connections/adapters/ws/rdpFilesystem";
import type { FileBrowserPlacement } from "@/features/connections/components/connections/fileBrowserState";

const ROOT_PATH = "/";
const DEFAULT_WINDOWS_DRIVE_NAME = "Piping";

interface RdpFileBrowserPanelProps {
  session: LiveSession;
  filesystem: RdpFilesystem;
  placement?: FileBrowserPlacement;
  onChangePlacement?: (placement: FileBrowserPlacement) => void;
  onClose?: () => void;
  className?: string;
}

type TransferStatus = "uploading" | "done" | "failed";

interface TransferJob {
  id: string;
  kind: "upload" | "download";
  name: string;
  status: TransferStatus;
  progress?: number;
}

function parentPath(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const slash = trimmed.lastIndexOf("/");
  if (slash <= 0) return ROOT_PATH;
  return trimmed.slice(0, slash);
}

function jobId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `job-${Date.now().toString(16)}-${Math.random().toString(16).slice(2, 8)}`;
}

function windowsDriveLocation(driveName: string): string {
  const trimmed = driveName.trim();
  const baseName = trimmed && !/^shared drive$/i.test(trimmed)
    ? trimmed
    : DEFAULT_WINDOWS_DRIVE_NAME;
  return /on guacamole$/i.test(baseName)
    ? baseName
    : `${baseName} on Guacamole`;
}

/**
 * File browser for RDP sessions, backed by Guacamole RDPDR drive redirection.
 *
 * The browsed tree is the session's redirected drive (mounted inside Windows
 * as the "Piping" disk), not the remote filesystem at large: anything copied
 * onto that disk inside the session appears here, and uploads from here appear
 * inside the session. The Guacamole object stream supports list / download /
 * upload only — delete, move, and mkdir happen inside the remote session.
 */
const RdpFileBrowserPanel = ({
  session,
  filesystem,
  placement = "right",
  onChangePlacement,
  onClose,
  className,
}: RdpFileBrowserPanelProps) => {
  const { t } = useTranslation();
  const [path, setPath] = useState(ROOT_PATH);
  const [entries, setEntries] = useState<RdpFsEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [dragActive, setDragActive] = useState(false);
  const [transfers, setTransfers] = useState<TransferJob[]>([]);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const aliveRef = useRef(true);
  const windowsDrive = windowsDriveLocation(filesystem.name);

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  const refresh = useCallback(
    async (target: string) => {
      setLoading(true);
      setLoadError(null);
      try {
        const listed = await filesystem.list(target);
        if (!aliveRef.current) return;
        setEntries(listed);
      } catch (error) {
        if (!aliveRef.current) return;
        setEntries([]);
        setLoadError(error instanceof Error ? error.message : t("connections.rdpFiles.loadFailed"));
      } finally {
        if (aliveRef.current) setLoading(false);
      }
    },
    [filesystem, t],
  );

  useEffect(() => {
    // External-system sync: pull the directory listing from the Guacamole
    // object stream whenever the path changes; loading/error state mirrors
    // that async exchange and is not derivable from render inputs.
    // eslint-disable-next-line react-hooks/set-state-in-effect -- Guacamole RDPDR directory listing is an external async source; loading/entries/error mirror that exchange and cannot be derived from render inputs
    void refresh(path);
  }, [path, refresh]);

  const updateTransfer = useCallback((id: string, patch: Partial<TransferJob>) => {
    setTransfers((current) => current.map((job) => (job.id === id ? { ...job, ...patch } : job)));
  }, []);

  const uploadFiles = useCallback(
    async (files: File[]) => {
      if (files.length === 0) return;
      for (const file of files) {
        const id = jobId();
        setTransfers((current) => [
          { id, kind: "upload" as const, name: file.name, status: "uploading" as const, progress: 0 },
          ...current.slice(0, 7),
        ]);
        try {
          await filesystem.upload(path, file, (loaded, total) => {
            if (!aliveRef.current) return;
            updateTransfer(id, {
              progress: total > 0 ? Math.min(100, Math.round((loaded / total) * 100)) : undefined,
            });
          });
          if (!aliveRef.current) return;
          updateTransfer(id, { status: "done", progress: 100 });
        } catch (error) {
          if (!aliveRef.current) return;
          updateTransfer(id, { status: "failed" });
          notify(
            "error",
            t("connections.rdpFiles.uploadFailed", {
              name: file.name,
              message: error instanceof Error ? error.message : "",
            }),
            { source: "connections" },
          );
        }
      }
      if (aliveRef.current) void refresh(path);
    },
    [filesystem, path, refresh, t, updateTransfer],
  );

  const downloadEntry = useCallback(
    async (entry: RdpFsEntry) => {
      const id = jobId();
      setTransfers((current) => [
        { id, kind: "download" as const, name: entry.name, status: "uploading" as const },
        ...current.slice(0, 7),
      ]);
      try {
        const blob = await filesystem.download(entry);
        if (!aliveRef.current) return;
        const url = URL.createObjectURL(blob);
        const link = document.createElement("a");
        link.href = url;
        link.download = entry.name;
        link.rel = "noopener";
        document.body.append(link);
        link.click();
        link.remove();
        setTimeout(() => URL.revokeObjectURL(url), 30_000);
        updateTransfer(id, { status: "done", progress: 100 });
      } catch (error) {
        if (!aliveRef.current) return;
        updateTransfer(id, { status: "failed" });
        notify(
          "error",
          t("connections.rdpFiles.downloadFailed", {
            name: entry.name,
            message: error instanceof Error ? error.message : "",
          }),
          { source: "connections" },
        );
      }
    },
    [filesystem, t, updateTransfer],
  );

  const handleDrop = useCallback(
    (dataTransfer: DataTransfer) => {
      // The Guacamole object stream is flat (no mkdir over the wire), so
      // dropped directories are skipped. `webkitGetAsEntry` reliably tells
      // files from folders; fall back to plain files when unsupported.
      const items = Array.from(dataTransfer.items ?? []);
      let files: File[];
      let skipped = 0;
      if (items.length > 0 && typeof items[0]?.webkitGetAsEntry === "function") {
        files = [];
        for (const item of items) {
          const entry = item.webkitGetAsEntry();
          if (entry?.isDirectory) {
            skipped += 1;
            continue;
          }
          const file = item.getAsFile();
          if (file) files.push(file);
        }
      } else {
        files = Array.from(dataTransfer.files ?? []);
      }
      if (skipped > 0) {
        notify("warning", t("connections.rdpFiles.foldersSkipped", { count: skipped }), {
          source: "connections",
        });
      }
      void uploadFiles(files);
    },
    [t, uploadFiles],
  );

  const visibleEntries = filter.trim()
    ? entries.filter((entry) => entry.name.toLowerCase().includes(filter.trim().toLowerCase()))
    : entries;

  const atRoot = path === ROOT_PATH;

  return (
    <section
      aria-label={t("connections.rdpFiles.title")}
      data-testid="rdp-file-browser"
      className={cn(
        "relative flex h-full min-h-0 w-full flex-col overflow-hidden border-border/40 bg-background",
        placement === "right" ? "border-l" : "border-t",
        className,
      )}
      onDragOver={(event) => {
        event.preventDefault();
        setDragActive(true);
      }}
      onDragLeave={() => setDragActive(false)}
      onDrop={(event) => {
        event.preventDefault();
        setDragActive(false);
        handleDrop(event.dataTransfer);
      }}
    >
      {/* Header */}
      <div className="flex shrink-0 items-center gap-2 border-b border-border/40 px-2 py-1.5">
        <FolderOpen className="h-4 w-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-semibold leading-5">
            {t("connections.rdpFiles.title")}
          </p>
          <p className="truncate text-xs leading-4 text-muted-foreground">
            {t("connections.rdpFiles.driveLabel", {
              drive: windowsDrive,
              host: session.hostName,
            })}
          </p>
        </div>
        {onChangePlacement ? (
          <>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t("connections.rdpFiles.placementRight")}
              className={cn(placement === "right" && "bg-accent")}
              onClick={() => onChangePlacement("right")}
            >
              <PanelRight className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t("connections.rdpFiles.placementBottom")}
              className={cn(placement === "bottom" && "bg-accent")}
              onClick={() => onChangePlacement("bottom")}
            >
              <PanelBottom className="h-3.5 w-3.5" />
            </Button>
          </>
        ) : null}
        {onClose ? (
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("connections.rdpFiles.close")}
            onClick={onClose}
          >
            <X className="h-3.5 w-3.5" />
          </Button>
        ) : null}
      </div>

      {/* Toolbar */}
      <div className="flex shrink-0 items-center gap-1 border-b border-border/30 px-2 py-1">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t("connections.rdpFiles.home")}
          disabled={atRoot || loading}
          onClick={() => setPath(ROOT_PATH)}
        >
          <Home className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t("connections.rdpFiles.up")}
          disabled={atRoot || loading}
          onClick={() => setPath(parentPath(path))}
        >
          <ArrowUp className="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t("connections.rdpFiles.refresh")}
          disabled={loading}
          onClick={() => void refresh(path)}
        >
          <RefreshCcw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
        </Button>
        <span className="min-w-0 flex-1 truncate px-1 font-mono text-xs text-muted-foreground">
          {path}
        </span>
        <SearchInput
          value={filter}
          onValueChange={setFilter}
          placeholder={t("connections.rdpFiles.searchPlaceholder")}
          className="h-9 w-28 border-input bg-background text-sm"
        />
        <Button
          variant="outline"
          size="sm"
          className="h-9 gap-1 px-2 text-sm"
          onClick={() => fileInputRef.current?.click()}
        >
          <FileUp className="h-3 w-3" />
          {t("connections.rdpFiles.uploadCta")}
        </Button>
        <input
          ref={fileInputRef}
          type="file"
          multiple
          className="hidden"
          onChange={(event) => {
            const files = Array.from(event.target.files ?? []);
            event.target.value = "";
            void uploadFiles(files);
          }}
        />
      </div>

      {/* Entry list */}
      <div className="min-h-0 flex-1 overflow-y-auto">
        {loading ? (
          <div className="flex h-full items-center justify-center">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        ) : loadError ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 p-4 text-center">
            <p className="text-xs text-status-negative">{loadError}</p>
            <Button variant="outline" size="sm" onClick={() => void refresh(path)}>
              {t("connections.rdpFiles.refresh")}
            </Button>
          </div>
        ) : visibleEntries.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-1 p-4 text-center">
            <p className="text-xs text-muted-foreground">{t("connections.rdpFiles.empty")}</p>
            <p className="max-w-56 text-xs leading-4 text-muted-foreground/80">
              {t("connections.rdpFiles.windowsHint", { drive: windowsDrive })}
            </p>
          </div>
        ) : (
          <ul className="py-0.5">
            {visibleEntries.map((entry) => (
              <li key={entry.path}>
                <div
                  className={cn(
                    "group flex min-h-9 w-full items-center gap-2 px-2 py-1 text-left text-sm hover:bg-accent",
                    entry.isDirectory && "cursor-pointer",
                  )}
                  role={entry.isDirectory ? "button" : undefined}
                  tabIndex={entry.isDirectory ? 0 : undefined}
                  onClick={entry.isDirectory ? () => setPath(entry.path) : undefined}
                  onKeyDown={
                    entry.isDirectory
                      ? (event) => {
                          if (event.key === "Enter" || event.key === " ") setPath(entry.path);
                        }
                      : undefined
                  }
                >
                  {entry.isDirectory ? (
                    <Folder className="h-3.5 w-3.5 shrink-0 text-status-info" />
                  ) : (
                    <File className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  )}
                  <span className="min-w-0 flex-1 truncate">{entry.name}</span>
                  {!entry.isDirectory ? (
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="size-9 transition-colors"
                      aria-label={t("connections.rdpFiles.download", { name: entry.name })}
                      onClick={(event) => {
                        event.stopPropagation();
                        void downloadEntry(entry);
                      }}
                    >
                      <Download className="h-3.5 w-3.5" />
                    </Button>
                  ) : null}
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* Transfers */}
      {transfers.length > 0 ? (
        <div className="max-h-24 shrink-0 overflow-y-auto border-t border-border/30 px-2 py-1">
          {transfers.map((job) => (
            <div key={job.id} className="flex items-center gap-2 py-0.5 text-xs">
              {job.status === "uploading" ? (
                <Loader2 className="h-3 w-3 shrink-0 animate-spin text-muted-foreground" />
              ) : job.status === "done" ? (
                <Download
                  className={cn(
                    "h-3 w-3 shrink-0 text-status-positive",
                    job.kind === "upload" && "rotate-180",
                  )}
                />
              ) : (
                <X className="h-3 w-3 shrink-0 text-status-negative" />
              )}
              <span className="min-w-0 flex-1 truncate">{job.name}</span>
              {job.status === "uploading" && typeof job.progress === "number" ? (
                <span className="shrink-0 tabular-nums text-muted-foreground">{job.progress}%</span>
              ) : null}
            </div>
          ))}
        </div>
      ) : null}

      {/* Drop overlay */}
      {dragActive ? (
        <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center border-2 border-dashed border-[hsl(var(--accent))] bg-background">
          <p className="text-xs font-medium text-foreground">
            {t("connections.rdpFiles.dropHint", { path })}
          </p>
        </div>
      ) : null}
    </section>
  );
};

export default RdpFileBrowserPanel;
