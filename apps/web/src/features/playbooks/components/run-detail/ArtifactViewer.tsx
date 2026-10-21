import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Download } from "lucide-react";

import { Button } from "@/shared/ui-primitives/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import { type PlaybookArtifact } from "@/features/playbooks/api";
import { formatArtifactTypeLabel, formatSize } from "./artifactFormat";

// ---------------------------------------------------------------------------
// ArtifactViewer — modal dialog showing artifact content with download
// ---------------------------------------------------------------------------

export function ArtifactViewer({
  artifact,
  onClose,
  onCloseAutoFocus,
}: {
  artifact: PlaybookArtifact;
  onClose: () => void;
  onCloseAutoFocus: (event: Event) => void;
}) {
  const { t } = useTranslation();

  const renderedContent = useMemo(() => {
    if (artifact.content_json !== undefined && artifact.content_json !== null) {
      try {
        return JSON.stringify(artifact.content_json, null, 2);
      } catch {
        // content_json is unknown; fall through to content_text.
      }
    }
    if (typeof artifact.content_text === "string") {
      return artifact.content_text;
    }
    return null;
  }, [artifact]);

  const isJson =
    artifact.content_type === "application/json" ||
    (artifact.content_json !== undefined && artifact.content_json !== null);

  const handleDownload = () => {
    if (renderedContent === null) return;
    const blob = new Blob([renderedContent], { type: artifact.content_type || "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = artifact.name;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
      <DialogContent
        onCloseAutoFocus={onCloseAutoFocus}
        className="flex h-[80dvh] max-h-[90dvh] max-w-3xl flex-col gap-0 overflow-y-auto bg-card p-0"
        description={t("playbooks.runDetail.artifactViewerDescription")}
      >
        <DialogHeader className="flex shrink-0 flex-row items-center gap-3 border-b border-border/30 px-4 py-3">
          <div className="min-w-0 flex-1">
            <DialogTitle className="truncate text-sm font-semibold text-foreground">
              {artifact.name}
            </DialogTitle>
            <p className="mt-0.5 flex flex-wrap gap-2 text-xs text-muted-foreground">
              <span>{formatArtifactTypeLabel(artifact.artifact_type, t)}</span>
              <span aria-hidden>·</span>
              <span>{formatSize(artifact.content_size_bytes)}</span>
              {isJson ? (
                <>
                  <span aria-hidden>·</span>
                  <span className="font-mono text-xs">JSON</span>
                </>
              ) : null}
            </p>
          </div>
          {renderedContent !== null ? (
            <Button
              variant="outline"
              size="sm"
              className="shrink-0 gap-2"
              onClick={handleDownload}
            >
              <Download className="h-3.5 w-3.5" />
              {t("download")}
            </Button>
          ) : null}
        </DialogHeader>
        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          {renderedContent !== null ? (
            <pre className="min-h-full whitespace-pre-wrap break-words font-mono text-sm leading-relaxed text-foreground">
              {renderedContent}
            </pre>
          ) : (
            <p className="py-8 text-center text-xs text-muted-foreground">
              {t("playbooks.runDetail.artifactNoContent")}
            </p>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
