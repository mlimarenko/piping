import { lazy, Suspense } from "react";
import { Loader2 } from "lucide-react";
import { IS_DEMO } from "@/demo/env";
import { demoHostNameFromWsUrl } from "@/demo/session-url";
import type { SshTerminalController } from "@/features/connections/components/connections/LiveTerminal";
import type { RdpFilesystem } from "@/features/connections/adapters/ws/rdpFilesystem";
import type {
  RemoteClipboardController,
  RemoteClipboardTextHandler,
} from "@/features/connections/adapters/ws/remoteClipboard";
import {
  loadCctvConsole,
  loadLiveRDP,
  loadLiveTerminal,
  loadLiveVNC,
  type LiveSessionProtocol,
} from "@/features/connections/components/connections/liveSessionLoaders";

const LiveTerminal = lazy(loadLiveTerminal);
const LiveRDP = lazy(loadLiveRDP);
const LiveVNC = lazy(loadLiveVNC);
// CctvConsole is a named export (it also renders at a standalone route), so map
// it to the default shape `lazy` expects.
const CctvConsole = lazy(() => loadCctvConsole().then((m) => ({ default: m.CctvConsole })));
// DEMO MODE ONLY: a styled static "remote desktop" stands in for the live
// guacamole/noVNC canvas. Lazy so the mock never enters the normal app bundle.
const DemoRemoteDesktop = lazy(() => import("@/demo/MockRemoteDesktop"));

export type { LiveSessionProtocol };

interface LiveSessionViewerProps {
  sessionId: string;
  protocol: LiveSessionProtocol;
  websocketUrl: string;
  token: string;
  isActive: boolean;
  isVisible?: boolean;
  /** Connection-target id — required only for the `cctv` console (no websocket). */
  targetId?: string;
  onOpen?: () => void;
  /**
   * Called when the underlying transport closes. `clean` is true on a normal
   * close (WebSocket code 1000); `code` and `reason` carry the raw close
   * frame so callers can classify failures (network vs auth vs server error).
   */
  onClose?: (clean: boolean, code?: number, reason?: string) => void;
  onSshControllerChange?: (controller: SshTerminalController | null) => void;
  /** Re-open this session from the SSH disconnected overlay (SSH only). */
  onReconnect?: () => void;
  /** Host label for the localized SSH failure overlay (SSH only). */
  /** Redirected-drive availability callback (RDP only). */
  onRdpFilesystem?: (fs: RdpFilesystem | null) => void;
  /** Clipboard controller availability callback (RDP/VNC only). */
  onClipboardControllerChange?: (controller: RemoteClipboardController | null) => void;
  /** Remote clipboard text received from an RDP/VNC session. */
  onRemoteClipboardText?: RemoteClipboardTextHandler;
}

const ViewerFallback = () => (
  <div className="flex h-full w-full items-center justify-center">
    <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
  </div>
);

const LiveSessionViewer = ({
  sessionId,
  protocol,
  websocketUrl,
  token,
  isActive,
  isVisible = isActive,
  targetId,
  onOpen,
  onClose,
  onSshControllerChange,
  onReconnect,
  onRdpFilesystem,
  onClipboardControllerChange,
  onRemoteClipboardText,
}: LiveSessionViewerProps) => {
  switch (protocol) {
    case "cctv":
      // The CCTV console is a client-only REST panel (no proxied websocket): it
      // ignores websocketUrl/token and drives `/api/v1/cctv/{targetId}/*`.
      return (
        <Suspense fallback={<ViewerFallback />}>
          {/* Keyed on `sessionId`, matching every other protocol below: if this
              tab is ever reused for a different session/target, remounting
              resets all of CctvConsole's descendant state (selected channel,
              grid settings, per-view error/retry/aspect state) instead of
              leaking the previous device's state into the new one. */}
          <CctvConsole
            key={sessionId}
            targetId={targetId ?? ""}
            className="h-full w-full"
            active={isVisible}
          />
        </Suspense>
      );
    case "ssh":
      return (
        <Suspense fallback={<ViewerFallback />}>
          <LiveTerminal
            key={sessionId}
            websocketUrl={websocketUrl}
            token={token}
            isActive={isActive}
            isVisible={isVisible}
            onOpen={onOpen}
            onClose={(code, reason) => onClose?.(code === 1000, code, reason)}
            onControllerChange={onSshControllerChange}
            onReconnect={onReconnect}
          />
        </Suspense>
      );
    case "rdp":
      // DEMO MODE ONLY: no live guacamole relay — show a styled Windows desktop.
      if (IS_DEMO) {
        return (
          <Suspense fallback={<ViewerFallback />}>
            <DemoRemoteDesktop variant="windows" hostName={demoHostNameFromWsUrl(websocketUrl)} onReady={onOpen} />
          </Suspense>
        );
      }
      return (
        <Suspense fallback={<ViewerFallback />}>
          <LiveRDP
            key={sessionId}
            websocketUrl={websocketUrl}
            token={token}
            isActive={isActive}
            onOpen={onOpen}
            onClose={(clean, code, reason) => onClose?.(clean, code, reason)}
            onRetry={onReconnect}
            onFilesystem={onRdpFilesystem}
            onClipboardControllerChange={onClipboardControllerChange}
            onRemoteClipboardText={onRemoteClipboardText}
          />
        </Suspense>
      );
    case "vnc":
      // DEMO MODE ONLY: no live noVNC relay — show a styled Linux desktop.
      if (IS_DEMO) {
        return (
          <Suspense fallback={<ViewerFallback />}>
            <DemoRemoteDesktop variant="linux" hostName={demoHostNameFromWsUrl(websocketUrl)} onReady={onOpen} />
          </Suspense>
        );
      }
      return (
        <Suspense fallback={<ViewerFallback />}>
          <LiveVNC
            key={sessionId}
            sessionId={sessionId}
            websocketUrl={websocketUrl}
            token={token}
            isActive={isActive}
            onOpen={onOpen}
            onClose={(clean, reason) => onClose?.(clean, clean ? 1000 : 1006, reason)}
            onRetry={onReconnect}
            onClipboardControllerChange={onClipboardControllerChange}
            onRemoteClipboardText={onRemoteClipboardText}
          />
        </Suspense>
      );
  }
  const exhaustive: never = protocol;
  throw new Error(`Unsupported live session protocol: ${String(exhaustive)}`);
};

export default LiveSessionViewer;
