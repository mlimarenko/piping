import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Pause, Play, RefreshCw } from "lucide-react";
import { apiErrorFromResponse, API_BASE_URL, fetchWithSession } from "@/shared/api/client";
import { useDocumentActive } from "@/shared/hooks/useDocumentActive";
import { cn } from "@/shared/lib/utils";
import { Button } from "@/shared/ui-primitives/button";
import type { ConnectionFailure } from "@/features/connections/connectionFailure";
import ConnectionFailureOverlay from "./ConnectionFailureOverlay";
import { connectionFailureFromMediaError } from "./mediaConnectionFailure";

const DEFAULT_INTERVAL_MS = 3000;
const AUTO_RETRY_DELAY_MS = 1500;

function snapshotUrl(targetId: string, channel: number | undefined, tick: number): string {
  const params = new URLSearchParams({ t: String(tick) });
  if (channel != null) params.set("channel", String(channel));
  return `${API_BASE_URL}/api/v1/cctv/${encodeURIComponent(targetId)}/snapshot?${params}`;
}

async function requestSnapshot(url: string, signal: AbortSignal): Promise<Blob> {
  const response = await fetchWithSession(url, { credentials: "include", signal });
  if (!response.ok) throw await apiErrorFromResponse(response);
  return response.blob();
}

/** Live still-frame viewer with an abortable, cookie-authenticated media fetch. */
function DeviceSnapshotView({
  targetId,
  channel,
  intervalMs = DEFAULT_INTERVAL_MS,
  autoRefreshDefault = false,
  hideControls = false,
  className,
  active: activeProp = true,
}: Readonly<{
  targetId: string;
  channel?: number;
  intervalMs?: number;
  autoRefreshDefault?: boolean;
  hideControls?: boolean;
  className?: string;
  active?: boolean;
}>) {
  const { t } = useTranslation();
  const [tick, setTick] = useState(0);
  const [autoRefresh, setAutoRefresh] = useState(autoRefreshDefault);
  const [failure, setFailure] = useState<{
    key: string;
    value: ConnectionFailure;
  } | null>(null);
  const [objectUrl, setObjectUrl] = useState<{ key: string; url: string } | null>(null);
  const [autoRetried, setAutoRetried] = useState(false);
  const [aspectRatio, setAspectRatio] = useState("16 / 9");
  const documentActive = useDocumentActive();
  const active = activeProp && documentActive;

  const refresh = () => setTick((value) => value + 1);

  useEffect(() => {
    if (!autoRefresh || !active) return;
    const handle = window.setInterval(refresh, intervalMs);
    return () => window.clearInterval(handle);
  }, [autoRefresh, active, intervalMs]);

  const mediaKey = `${targetId}:${channel ?? "default"}:${tick}:${active ? "active" : "paused"}`;
  const visibleObjectUrl = objectUrl?.key === mediaKey ? objectUrl.url : null;
  const visibleFailure = failure?.key === mediaKey ? failure.value : null;

  useEffect(() => {
    if (!visibleFailure || autoRetried || !active) return;
    const handle = window.setTimeout(() => {
      setAutoRetried(true);
      refresh();
    }, AUTO_RETRY_DELAY_MS);
    return () => window.clearTimeout(handle);
  }, [visibleFailure, autoRetried, active]);

  useEffect(() => {
    if (!active) return;

    const controller = new AbortController();
    let createdUrl: string | undefined;
    void (async () => {
      try {
        const blob = await requestSnapshot(snapshotUrl(targetId, channel, tick), controller.signal);
        if (controller.signal.aborted) return;
        createdUrl = URL.createObjectURL(blob);
        setObjectUrl({ key: mediaKey, url: createdUrl });
        setAutoRetried(false);
      } catch (error: unknown) {
        if (!controller.signal.aborted) {
          setFailure({ key: mediaKey, value: connectionFailureFromMediaError(error) });
        }
      }
    })();

    return () => {
      controller.abort();
      if (createdUrl) URL.revokeObjectURL(createdUrl);
    };
  }, [targetId, channel, tick, active, mediaKey]);

  return (
    <div className={cn("flex flex-col gap-2", className)}>
      <div
        className="relative flex max-h-[75vh] w-full items-center justify-center overflow-hidden rounded-md border border-border/40 bg-black/80"
        style={{ aspectRatio }}
      >
        {visibleObjectUrl && (
          <img
            src={visibleObjectUrl}
            alt={t("connections.video.snapshotAlt", "Device snapshot")}
            className="h-full w-full object-contain pointer-events-none"
            onLoad={(event) => {
              const { naturalWidth, naturalHeight } = event.currentTarget;
              if (naturalWidth > 0 && naturalHeight > 0) {
                setAspectRatio(`${naturalWidth} / ${naturalHeight}`);
              }
            }}
            onError={() => setFailure({ key: mediaKey, value: { kind: "network" } })}
          />
        )}
        {visibleFailure && (
          <div className="absolute inset-0 z-10">
            <ConnectionFailureOverlay failure={visibleFailure} onRetry={refresh} />
          </div>
        )}
      </div>

      {!hideControls && (
        <div className="flex items-center justify-between gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-9 gap-2 text-sm"
            onClick={refresh}
          >
            <RefreshCw className="h-3 w-3" />
            {t("connections.video.refresh", "Refresh")}
          </Button>
          <Button
            type="button"
            variant={autoRefresh ? "secondary" : "ghost"}
            size="sm"
            className="h-9 gap-2 text-sm"
            aria-pressed={autoRefresh}
            onClick={() => setAutoRefresh((value) => !value)}
          >
            {autoRefresh ? <Pause className="h-3 w-3" /> : <Play className="h-3 w-3" />}
            {t("connections.video.autoRefresh", "Auto-refresh")}
          </Button>
        </div>
      )}
    </div>
  );
}

export default DeviceSnapshotView;
