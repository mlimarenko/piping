import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ConnectionFailure } from "@/features/connections/connectionFailure";
import { connectionFailureFromWsClose } from "@/features/connections/connectionFailure";
import ConnectionFailureOverlay from "@/features/connections/components/connections/ConnectionFailureOverlay";
import { Loader2 } from "lucide-react";
import { attachRdpViewer, type GuacAdapter } from "@/features/connections/adapters/ws/rdpAdapter";
import type { RdpFilesystem } from "@/features/connections/adapters/ws/rdpFilesystem";
import type {
  RemoteClipboardController,
  RemoteClipboardTextHandler,
} from "@/features/connections/adapters/ws/remoteClipboard";
import type { SessionDisplayMode } from "@/features/connections/sessionViewPreferences";
import {
  capDisplayResolution,
  fitDisplayScale,
  type SessionDisplaySize,
} from "@/features/connections/components/connections/sessionDisplayGeometry";

interface LiveRDPProps {
  websocketUrl: string;
  token: string;
  readOnly?: boolean;
  /** VNC only: show a local crosshair until the session provides its own cursor. */
  fallbackCursor?: boolean;
  isActive: boolean;
  focusRequest?: number;
  /** How the remote framebuffer is presented in the panel. */
  displayMode?: SessionDisplayMode;
  /** Optional remote resolution ceiling (data-saver preset). */
  maxResolution?: SessionDisplaySize | null;
  onOpen?: () => void;
  onClose?: (clean: boolean, code?: number, reason?: string) => void;
  /** The tunnel's raw WebSocket close pair (code and reason as sent). */
  onWireClose?: (code: number, reason: string) => void;
  onClipboardControllerChange?: (controller: RemoteClipboardController | null) => void;
  onRemoteClipboardText?: RemoteClipboardTextHandler;
  /**
   * Reports the session's redirected drive (RDPDR) once guacd exposes it, and
   * `null` when the viewer is torn down. Powers the RDP file browser.
   */
  onFilesystem?: (fs: RdpFilesystem | null) => void;
  onRetry?: () => void;
}

function readContainerSize(container: HTMLElement): { width: number; height: number } {
  const rect = container.getBoundingClientRect();
  return {
    width: Math.max(1, Math.floor(rect.width)),
    height: Math.max(1, Math.floor(rect.height)),
  };
}

/**
 * Guacamole-backed RDP viewer. The backend performs the guacd handshake and
 * relays Guacamole instructions over the per-session WebSocket.
 */
const LiveRDP = ({
  websocketUrl,
  token,
  readOnly = false,
  fallbackCursor = false,
  isActive,
  focusRequest = 0,
  displayMode = "fit",
  maxResolution = null,
  onOpen,
  onClose,
  onWireClose,
  onClipboardControllerChange,
  onRemoteClipboardText,
  onFilesystem,
  onRetry,
}: LiveRDPProps) => {
  const { t } = useTranslation();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const adapterRef = useRef<GuacAdapter | null>(null);
  const [phase, setPhase] = useState<"connecting" | "connected" | "error">("connecting");
  const [failure, setFailure] = useState<ConnectionFailure | null>(null);
  // The Guacamole adapter outlives individual renders and invokes these props
  // asynchronously; holding the latest values in a ref keeps the attach effect
  // from tearing down the connection when a parent re-render creates new
  // callback identities.
  const callbacksRef = useRef({
    onOpen,
    onClose,
    onWireClose,
    onClipboardControllerChange,
    onRemoteClipboardText,
    onFilesystem,
  });
  useEffect(() => {
    callbacksRef.current = {
      onOpen,
      onClose,
      onWireClose,
      onClipboardControllerChange,
      onRemoteClipboardText,
      onFilesystem,
    };
  });

  // Geometry changes do not reconnect: the adapter scales the existing
  // framebuffer, and only asks guacd for a new remote size in fit mode.
  const viewRef = useRef({ displayMode, maxResolution });
  const applyViewport = useCallback(() => {
    const adapter = adapterRef.current;
    const container = containerRef.current;
    if (!adapter || !container) return;
    const display = adapter.getDisplaySize();
    if (display.width <= 0 || display.height <= 0) return;
    const scale =
      viewRef.current.displayMode === "fit"
        ? fitDisplayScale(readContainerSize(container), display)
        : 1;
    adapter.setDisplayScale(scale);
  }, []);

  const requestRemoteSize = useCallback(() => {
    const adapter = adapterRef.current;
    const container = containerRef.current;
    if (!adapter || !container || readOnly || viewRef.current.displayMode !== "fit") return;
    const size = readContainerSize(container);
    // A collapsed or not-yet-laid-out panel must not shrink the remote desktop.
    if (size.width < 32 || size.height < 32) return;
    const target = capDisplayResolution(size, viewRef.current.maxResolution);
    adapter.resize(target.width, target.height);
  }, [readOnly]);

  useEffect(() => {
    viewRef.current = { displayMode, maxResolution };
    requestRemoteSize();
    applyViewport();
  }, [applyViewport, displayMode, maxResolution, requestRemoteSize]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    let cancelled = false;
    let resizeObserver: ResizeObserver | null = null;
    setPhase("connecting");
    setFailure(null);

    const handleResize = () => {
      requestRemoteSize();
      applyViewport();
    };

    try {
      const viewport = viewRef.current;
      const viewportSize = readContainerSize(container);
      const hasLayout = viewportSize.width >= 32 && viewportSize.height >= 32;
      const requested = capDisplayResolution(viewportSize, viewport.maxResolution);
      const adapter = attachRdpViewer({
        container,
        url: websocketUrl,
        token,
        readOnly,
        fallbackCursor,
        // In actual-size mode the connection config chooses the remote size;
        // the browser must not replace it with the panel geometry.
        width: viewport.displayMode === "fit" && hasLayout ? requested.width : undefined,
        height: viewport.displayMode === "fit" && hasLayout ? requested.height : undefined,
        onDisplayResize: () => {
          if (cancelled) return;
          // The remote framebuffer changed, so scale the new pixels to the panel.
          applyViewport();
        },
        onFilesystem: (fs) => {
          if (cancelled) return;
          if (!readOnly) callbacksRef.current.onFilesystem?.(fs);
        },
        onClipboardControllerChange: (controller) => {
          if (cancelled && controller) return;
          if (!readOnly) callbacksRef.current.onClipboardControllerChange?.(controller);
        },
        onRemoteClipboardText: (text) => {
          if (cancelled) return;
          if (!readOnly) callbacksRef.current.onRemoteClipboardText?.(text);
        },
        onOpen: () => {
          if (cancelled) return;
          setPhase("connected");
          callbacksRef.current.onOpen?.();
        },
        onClose: (code, reason) => {
          if (cancelled) return;
          const clean = code === 0;
          if (!clean) {
            setPhase("error");
            setFailure(connectionFailureFromWsClose(code, reason));
          }
          callbacksRef.current.onClose?.(clean, code, reason);
        },
        onWireClose: (code, reason) => {
          if (cancelled) return;
          callbacksRef.current.onWireClose?.(code, reason);
        },
      });
      adapterRef.current = adapter;
      requestRemoteSize();
      applyViewport();

      if (typeof ResizeObserver !== "undefined") {
        resizeObserver = new ResizeObserver(handleResize);
        resizeObserver.observe(container);
      }
    } catch {
      const failure = connectionFailureFromWsClose(undefined, undefined);
      // The adapter attach is an imperative external-system operation; reflect
      // its synchronous failure in the viewer state.
      // eslint-disable-next-line react-hooks/set-state-in-effect -- guacd attach is an imperative external-system call with no event to source this failure from; the synchronous catch must surface it before paint
      setPhase("error");
      setFailure(failure);
      callbacksRef.current.onClose?.(false);
    }

    return () => {
      cancelled = true;
      resizeObserver?.disconnect();
      resizeObserver = null;
      adapterRef.current?.dispose();
      adapterRef.current = null;
      // The drive object dies with the guac client — drop the stale handle.
      if (!readOnly) callbacksRef.current.onFilesystem?.(null);
      if (!readOnly) callbacksRef.current.onClipboardControllerChange?.(null);
    };
  }, [applyViewport, fallbackCursor, readOnly, requestRemoteSize, token, websocketUrl]);

  useEffect(() => {
    if (!isActive) {
      const focused = document.activeElement;
      if (focused instanceof HTMLElement && containerRef.current?.contains(focused)) focused.blur();
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      const container = containerRef.current;
      if (!container) return;
      requestRemoteSize();
      applyViewport();
      container.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [applyViewport, focusRequest, isActive, requestRemoteSize]);

  return (
    <div className="relative h-full w-full bg-[hsl(220,15%,10%)]">
      <div
        ref={containerRef}
        tabIndex={0}
        className="relative flex h-full w-full overflow-auto outline-none"
        data-testid="live-rdp"
      />

      {phase === "connecting" && (
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-[hsl(220,15%,10%)]/80">
          <Loader2 className="h-6 w-6 animate-spin text-status-warning" />
          <p className="text-sm text-muted-foreground">{t("detachedSession.connecting")}</p>
        </div>
      )}

      {phase === "error" && failure && (
        <ConnectionFailureOverlay failure={failure} onRetry={readOnly ? undefined : onRetry} />
      )}
    </div>
  );
};

export default LiveRDP;
