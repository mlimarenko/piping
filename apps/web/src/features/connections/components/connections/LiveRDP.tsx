import { useEffect, useRef, useState } from "react";
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

interface LiveRDPProps {
  websocketUrl: string;
  token: string;
  isActive: boolean;
  onOpen?: () => void;
  onClose?: (clean: boolean, code?: number, reason?: string) => void;
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
  isActive,
  onOpen,
  onClose,
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
    onClipboardControllerChange,
    onRemoteClipboardText,
    onFilesystem,
  });
  useEffect(() => {
    callbacksRef.current = {
      onOpen,
      onClose,
      onClipboardControllerChange,
      onRemoteClipboardText,
      onFilesystem,
    };
  });

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    let cancelled = false;
    let resizeObserver: ResizeObserver | null = null;
    setPhase("connecting");
    setFailure(null);

    const handleResize = () => {
      const adapter = adapterRef.current;
      if (!adapter || !containerRef.current) return;
      const { width, height } = readContainerSize(containerRef.current);
      adapter.resize(width, height);
    };

    try {
      const { width, height } = readContainerSize(container);
      const adapter = attachRdpViewer({
        container,
        url: websocketUrl,
        token,
        width,
        height,
        onFilesystem: (fs) => {
          if (cancelled) return;
          callbacksRef.current.onFilesystem?.(fs);
        },
        onClipboardControllerChange: (controller) => {
          if (cancelled && controller) return;
          callbacksRef.current.onClipboardControllerChange?.(controller);
        },
        onRemoteClipboardText: (text) => {
          if (cancelled) return;
          callbacksRef.current.onRemoteClipboardText?.(text);
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
      });
      adapterRef.current = adapter;

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
      callbacksRef.current.onFilesystem?.(null);
      callbacksRef.current.onClipboardControllerChange?.(null);
    };
  }, [token, websocketUrl]);

  useEffect(() => {
    if (!isActive) {
      const focused = document.activeElement;
      if (focused instanceof HTMLElement && containerRef.current?.contains(focused)) focused.blur();
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      const adapter = adapterRef.current;
      const container = containerRef.current;
      if (!adapter || !container) return;
      const { width, height } = readContainerSize(container);
      adapter.resize(width, height);
      container.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [isActive]);

  return (
    <div className="relative h-full w-full bg-[hsl(220,15%,10%)]">
      <div
        ref={containerRef}
        tabIndex={0}
        className="h-full w-full cursor-none overflow-hidden outline-none"
        data-testid="live-rdp"
      />

      {phase === "connecting" && (
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-[hsl(220,15%,10%)]/80">
          <Loader2 className="h-6 w-6 animate-spin text-status-warning" />
          <p className="text-sm text-muted-foreground">{t("detachedSession.connecting")}</p>
        </div>
      )}

      {phase === "error" && failure && (
        <ConnectionFailureOverlay failure={failure} onRetry={onRetry} />
      )}
    </div>
  );
};

export default LiveRDP;
