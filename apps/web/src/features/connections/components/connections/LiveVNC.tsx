import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import type { ConnectionFailure } from "@/features/connections/connectionFailure";
import {
  connectionFailureFromApiError,
  connectionFailureFromWsClose,
} from "@/features/connections/connectionFailure";
import ConnectionFailureOverlay from "@/features/connections/components/connections/ConnectionFailureOverlay";
import { getSessionClientCredentials, type SessionClientCredentials } from "@/features/connections/api";
import { attachVncViewer, type VncAdapter } from "@/features/connections/adapters/ws/vncAdapter";
import { waitForViewerPaint } from "@/features/connections/components/connections/viewerPaint";
import type {
  RemoteClipboardController,
  RemoteClipboardTextHandler,
} from "@/features/connections/adapters/ws/remoteClipboard";

interface LiveVNCProps {
  sessionId: string;
  websocketUrl: string;
  token: string;
  isActive: boolean;
  onOpen?: () => void;
  onClose?: (clean: boolean, reason?: string) => void;
  onClipboardControllerChange?: (controller: RemoteClipboardController | null) => void;
  onRemoteClipboardText?: RemoteClipboardTextHandler;
  onRetry?: () => void;
}

/**
 * noVNC-backed VNC viewer. Fetches resolved credentials from the
 * client-credentials endpoint (so UUID secret refs become plain-text
 * passwords), then attaches the noVNC RFB connection.
 */
const LiveVNC = ({
  sessionId,
  websocketUrl,
  token,
  isActive,
  onOpen,
  onClose,
  onClipboardControllerChange,
  onRemoteClipboardText,
  onRetry,
}: LiveVNCProps) => {
  const { t } = useTranslation();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [phase, setPhase] = useState<"loading" | "connecting" | "error">("loading");
  const [failure, setFailure] = useState<ConnectionFailure | null>(null);
  const adapterRef = useRef<VncAdapter | null>(null);
  const activeRef = useRef(isActive);
  const resizeFrameRef = useRef<number | null>(null);
  const resizeSettleFrameRef = useRef<number | null>(null);
  // Callback props are invoked by the long-lived noVNC adapter, so the attach
  // effect must not re-run (dispose + reconnect) when their identity changes.
  // Keep the latest values in a ref, updated after every render, and depend
  // only on the connection identity below.
  const callbacksRef = useRef({
    onOpen,
    onClose,
    onClipboardControllerChange,
    onRemoteClipboardText,
  });
  useEffect(() => {
    callbacksRef.current = {
      onOpen,
      onClose,
      onClipboardControllerChange,
      onRemoteClipboardText,
    };
  });

  const cancelScheduledResize = useCallback(() => {
    if (resizeFrameRef.current !== null) {
      window.cancelAnimationFrame(resizeFrameRef.current);
      resizeFrameRef.current = null;
    }
    if (resizeSettleFrameRef.current !== null) {
      window.cancelAnimationFrame(resizeSettleFrameRef.current);
      resizeSettleFrameRef.current = null;
    }
  }, []);

  const requestAdapterResize = useCallback((settleLayout = false) => {
    cancelScheduledResize();
    resizeFrameRef.current = window.requestAnimationFrame(() => {
      resizeFrameRef.current = null;
      if (!settleLayout) {
        adapterRef.current?.resize();
        return;
      }
      resizeSettleFrameRef.current = window.requestAnimationFrame(() => {
        resizeSettleFrameRef.current = null;
        adapterRef.current?.resize();
      });
    });
  }, [cancelScheduledResize]);

  useEffect(() => {
    return () => cancelScheduledResize();
  }, [cancelScheduledResize]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => requestAdapterResize());
    observer.observe(container);
    return () => observer.disconnect();
  }, [requestAdapterResize]);

  useEffect(() => {
    if (!containerRef.current) return;
    let cancelled = false;

    const run = async () => {
      setPhase("loading");
      setFailure(null);

      let password: string | undefined;
      try {
        const creds: SessionClientCredentials = await getSessionClientCredentials(sessionId);
        password = creds.credentials.password;
      } catch (error) {
        if (cancelled) return;
        setPhase("error");
        setFailure(connectionFailureFromApiError(error));
        return;
      }

      if (cancelled || !containerRef.current) return;
      setPhase("connecting");
      await waitForViewerPaint();
      if (cancelled || !containerRef.current) return;

      try {
        const adapter = await attachVncViewer({
          container: containerRef.current,
          url: websocketUrl,
          token,
          password,
          onOpen: () => {
            if (!cancelled) callbacksRef.current.onOpen?.();
          },
          onClose: (clean, reason) => {
            if (cancelled) return;
            if (!clean) {
              setPhase("error");
              setFailure(connectionFailureFromWsClose(1006, reason));
            }
            callbacksRef.current.onClose?.(clean, reason);
          },
          onClipboardControllerChange: (controller) => {
            if (cancelled && controller) return;
            callbacksRef.current.onClipboardControllerChange?.(controller);
          },
          onRemoteClipboardText: (text) => {
            if (!cancelled) callbacksRef.current.onRemoteClipboardText?.(text);
          },
        });
        if (cancelled) {
          adapter.dispose();
          return;
        }
        adapterRef.current = adapter;
        adapter.setInputEnabled(activeRef.current);
        requestAdapterResize(true);
      } catch {
        if (cancelled) return;
        setPhase("error");
        setFailure(connectionFailureFromWsClose(undefined, undefined));
      }
    };

    void run();

    return () => {
      cancelled = true;
      cancelScheduledResize();
      adapterRef.current?.dispose();
      adapterRef.current = null;
      callbacksRef.current.onClipboardControllerChange?.(null);
    };
  }, [cancelScheduledResize, requestAdapterResize, sessionId, token, websocketUrl]);

  useEffect(() => {
    activeRef.current = isActive;
    adapterRef.current?.setInputEnabled(isActive);
    if (!isActive) {
      const focused = document.activeElement;
      if (focused instanceof HTMLElement && containerRef.current?.contains(focused)) focused.blur();
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      adapterRef.current?.resize();
      adapterRef.current?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [isActive]);

  return (
    <div className="relative h-full w-full overflow-hidden bg-[hsl(220,15%,10%)]">
      <div
        ref={containerRef}
        tabIndex={0}
        className="h-full w-full min-h-0 min-w-0 overflow-hidden outline-none"
        data-testid="live-vnc"
      />

      {phase === "loading" && (
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-2">
          <Loader2 className="h-6 w-6 animate-spin text-status-warning" />
          <p className="text-sm text-muted-foreground">{t("connections.vncLoading")}</p>
        </div>
      )}

      {phase === "error" && failure && (
        <ConnectionFailureOverlay failure={failure} onRetry={onRetry} />
      )}
    </div>
  );
};

export default LiveVNC;
