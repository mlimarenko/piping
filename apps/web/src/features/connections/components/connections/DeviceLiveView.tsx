import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { PauseCircle } from "lucide-react";
import { apiErrorFromResponse, fetchWithSession } from "@/shared/api/client";
import { useDocumentActive } from "@/shared/hooks/useDocumentActive";
import { cn } from "@/shared/lib/utils";
import type { ConnectionFailure } from "@/features/connections/connectionFailure";
import ConnectionFailureOverlay from "./ConnectionFailureOverlay";
import { deviceStreamUrl } from "./deviceStreamUrl";
import { connectionFailureFromMediaError } from "./mediaConnectionFailure";
import { MjpegMultipartParser, multipartBoundary } from "./mjpegMultipartParser";

const AUTO_RETRY_DELAY_MS = 5000;

async function pumpMjpeg(
  response: Response,
  signal: AbortSignal,
  onFrame: (blob: Blob) => void,
): Promise<void> {
  const boundary = multipartBoundary(response.headers.get("Content-Type"));
  if (!boundary || !response.body) throw new Error("Invalid MJPEG response");

  const parser = new MjpegMultipartParser(boundary);
  const reader = response.body.getReader();
  const cancelReader = () => {
    void reader.cancel().catch(() => undefined);
  };
  signal.addEventListener("abort", cancelReader, { once: true });
  try {
    while (!signal.aborted) {
      const { done, value } = await reader.read();
      if (done) {
        if (signal.aborted) return;
        throw new Error("MJPEG stream ended");
      }
      for (const frame of parser.push(value)) {
        const frameBytes = new Uint8Array(frame);
        onFrame(new Blob([frameBytes], { type: "image/jpeg" }));
      }
    }
  } finally {
    signal.removeEventListener("abort", cancelReader);
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}

async function requestMjpeg(url: string, signal: AbortSignal): Promise<Response> {
  const response = await fetchWithSession(url, { credentials: "include", signal });
  if (!response.ok) throw await apiErrorFromResponse(response);
  return response;
}

/** Live motion video backed by an abortable, bounded browser-side MJPEG pump. */
function DeviceLiveView({
  targetId,
  channel,
  fps,
  maxWidth,
  className,
  overlay,
  active: activeProp = true,
}: Readonly<{
  targetId: string;
  channel?: number;
  fps?: number;
  maxWidth?: number;
  className?: string;
  overlay?: React.ReactNode;
  active?: boolean;
}>) {
  const { t } = useTranslation();
  const [attempt, setAttempt] = useState(0);
  const [failure, setFailure] = useState<{
    key: string;
    value: ConnectionFailure;
  } | null>(null);
  const [frameUrl, setFrameUrl] = useState<{ key: string; url: string } | null>(null);
  const [aspectRatio, setAspectRatio] = useState("16 / 9");
  const autoRetryUsed = useRef(false);
  const documentActive = useDocumentActive();
  const active = activeProp && documentActive;
  const mediaKey = `${targetId}:${channel ?? "default"}:${fps ?? "default"}:${maxWidth ?? "default"}:${attempt}:${active ? "active" : "paused"}`;
  const visibleFrameUrl = frameUrl?.key === mediaKey ? frameUrl.url : null;
  const visibleFailure = failure?.key === mediaKey ? failure.value : null;

  const reconnect = () => {
    autoRetryUsed.current = false;
    setAttempt((value) => value + 1);
  };

  useEffect(() => {
    if (!visibleFailure || autoRetryUsed.current || !active) return;
    const handle = window.setTimeout(() => {
      autoRetryUsed.current = true;
      setAttempt((value) => value + 1);
    }, AUTO_RETRY_DELAY_MS);
    return () => window.clearTimeout(handle);
  }, [visibleFailure, active]);

  useEffect(() => {
    if (!active) return;

    const controller = new AbortController();
    let currentFrameUrl: string | undefined;

    void (async () => {
      try {
        const response = await requestMjpeg(
          deviceStreamUrl(targetId, { channel, fps, maxWidth, attempt }),
          controller.signal,
        );
        await pumpMjpeg(response, controller.signal, (blob) => {
          if (controller.signal.aborted) return;
          const nextFrameUrl = URL.createObjectURL(blob);
          if (currentFrameUrl) URL.revokeObjectURL(currentFrameUrl);
          currentFrameUrl = nextFrameUrl;
          setFrameUrl({ key: mediaKey, url: nextFrameUrl });
          autoRetryUsed.current = false;
        });
      } catch (error: unknown) {
        if (!controller.signal.aborted) {
          setFailure({ key: mediaKey, value: connectionFailureFromMediaError(error) });
        }
      }
    })();

    return () => {
      controller.abort();
      if (currentFrameUrl) URL.revokeObjectURL(currentFrameUrl);
    };
  }, [targetId, channel, fps, maxWidth, attempt, active, mediaKey]);

  return (
    <div
      className={cn(
        "relative flex max-h-[75vh] w-full items-center justify-center overflow-hidden rounded-md border border-border/40 bg-black/80",
        className,
      )}
      style={{ aspectRatio }}
    >
      {active && visibleFrameUrl && (
        <img
          src={visibleFrameUrl}
          alt={t("connections.video.liveAlt", "Live video")}
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
      {!active && (
        <div className="absolute inset-0 z-10 flex flex-col items-center justify-center gap-1.5 px-4 text-center">
          <PauseCircle className="h-5 w-5 text-muted-foreground" />
          <p className="max-w-xs text-xs leading-snug text-muted-foreground text-balance">
            {t(
              "connections.video.livePausedBackground",
              "Paused while this window is in the background — click here to resume.",
            )}
          </p>
        </div>
      )}
      {visibleFailure && active && (
        <div className="absolute inset-0 z-10">
          <ConnectionFailureOverlay failure={visibleFailure} onRetry={reconnect} />
        </div>
      )}
      {overlay}
    </div>
  );
}

export default DeviceLiveView;
