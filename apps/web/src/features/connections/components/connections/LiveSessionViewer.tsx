import { lazy, Suspense, useEffect, useRef, useState } from "react";
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
  type LiveSessionProtocol,
} from "@/features/connections/components/connections/liveSessionLoaders";
import { useSessionViewSettings } from "@/features/connections/hooks/useSessionViewSettings";
import { sessionViewMaxResolution } from "@/features/connections/sessionViewPreferences";

const LiveTerminal = lazy(loadLiveTerminal);
const LiveRDP = lazy(loadLiveRDP);
// CctvConsole is a named export (it also renders at a standalone route), so map
// it to the default shape `lazy` expects.
const CctvConsole = lazy(() => loadCctvConsole().then((m) => ({ default: m.CctvConsole })));
// DEMO MODE ONLY: a styled static "remote desktop" stands in for the live
// Guacamole canvas. Lazy so the mock never enters the normal app bundle.
const DemoRemoteDesktop = lazy(() => import("@/demo/MockRemoteDesktop"));

export type { LiveSessionProtocol };

/**
 * The observation relay forwards exactly one lease-expiry close: WebSocket
 * 1000 with this reason, emitted by the proxy when the 120s observer
 * credential runs out. Every other close — an operator teardown, a policy or
 * transport failure, a revocation or generation change — stays terminal.
 */
const OBSERVER_LEASE_EXPIRED_REASON = "observer_lease_expired";
const OBSERVER_LEASE_EXPIRED_CLOSE_CODE = 1000;
/** Let the relay finish the expired leg before the fresh handshake. */
const OBSERVER_RENEWAL_DELAY_MS = 250;
/**
 * A lease that ends this soon after opening never served the viewer, so it is
 * not renewable; that keeps a rejected or broken handshake from looping.
 */
const OBSERVER_RENEWAL_MIN_STABLE_MS = 1_000;

/**
 * SSH sees the raw WebSocket close pair directly. The Guacamole tunnel folds a
 * reason-carrying close into `Status(NaN, reason)` and loses the code, so the
 * desktop adapter forwards the tunnel socket's original pair alongside its
 * status callbacks and this predicate is evaluated on that pair only.
 */
function isObserverLeaseExpired(code: number | undefined, reason: string | undefined): boolean {
  return code === OBSERVER_LEASE_EXPIRED_CLOSE_CODE && reason === OBSERVER_LEASE_EXPIRED_REASON;
}

/** Per-mount lease bookkeeping, pinned to the viewer inputs it was opened for. */
interface ObserverLease {
  identity: string;
  /** Completed renewals; part of the child key so a renewal remounts it. */
  count: number;
  /** When the current handshake opened, or null before it ever opened. */
  openedAt: number | null;
}

interface LiveSessionViewerProps {
  sessionId: string;
  protocol: LiveSessionProtocol;
  websocketUrl: string;
  token: string;
  readOnly?: boolean;
  isActive: boolean;
  focusRequest?: number;
  isVisible?: boolean;
  /** Connection-target id — enables per-host view preferences and the CCTV console. */
  hostId?: string;
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
  readOnly = false,
  isActive,
  focusRequest = 0,
  isVisible = isActive,
  hostId,
  targetId,
  onOpen,
  onClose,
  onSshControllerChange,
  onReconnect,
  onRdpFilesystem,
  onClipboardControllerChange,
  onRemoteClipboardText,
}: LiveSessionViewerProps) => {
  const view = useSessionViewSettings(hostId ?? targetId ?? "", protocol);

  // Observation lease renewal. The proxy credential lives 120s; when it runs
  // out the relay closes the observe socket with the exact pair matched above,
  // and the viewer re-handshakes the SAME public observe route. That handshake
  // rechecks the current user/session/target grants server-side and issues a
  // fresh short-lived credential. Nothing here resumes the operator's session
  // or launches a target — the child remounts on its existing URL.
  const identity = `${protocol}\u0000${sessionId}\u0000${websocketUrl}\u0000${token}\u0000${readOnly ? "observe" : "operator"}`;
  const [lease, setLease] = useState<ObserverLease>({ identity, count: 0, openedAt: null });
  const renewals = lease.identity === identity ? lease.count : 0;
  const openedAt = lease.identity === identity ? lease.openedAt : null;
  const renewalTimer = useRef<number | undefined>(undefined);

  // Closing, unmounting, deactivating or re-pointing the viewer cancels a
  // pending renewal so no socket opens behind a closed observation panel.
  useEffect(() => () => {
    if (renewalTimer.current === undefined) return;
    window.clearTimeout(renewalTimer.current);
    renewalTimer.current = undefined;
  }, [identity, isActive]);

  const handleOpen = () => {
    setLease({ identity, count: renewals, openedAt: Date.now() });
    onOpen?.();
  };

  /** The one place a read-only observation may renew its lease. */
  const scheduleObserverRenewal = (code?: number, reason?: string) => {
    if (!readOnly || !isActive || renewalTimer.current !== undefined) return;
    if (!isObserverLeaseExpired(code, reason)) return;
    // Only a lease that actually served the viewer may renew itself.
    if (openedAt === null || Date.now() - openedAt < OBSERVER_RENEWAL_MIN_STABLE_MS) return;
    renewalTimer.current = window.setTimeout(() => {
      renewalTimer.current = undefined;
      setLease({ identity, count: renewals + 1, openedAt: null });
    }, OBSERVER_RENEWAL_DELAY_MS);
  };

  const handleClose = (clean: boolean, code?: number, reason?: string) => {
    onClose?.(clean, code, reason);
    // SSH's raw pair arrives here; Guacamole reports only a status, so its
    // renewal decision comes from the adapter's wire-close callback below.
    if (protocol === "ssh") scheduleObserverRenewal(code, reason);
  };

  const handleWireClose = (code: number, reason: string) => {
    scheduleObserverRenewal(code, reason);
  };

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
            key={`${sessionId}:${renewals}`}
            websocketUrl={websocketUrl}
            token={token}
            readOnly={readOnly}
            isActive={isActive}
            focusRequest={focusRequest}
            isVisible={isVisible}
            onOpen={handleOpen}
            onClose={(code, reason) => handleClose(code === 1000, code, reason)}
            onControllerChange={readOnly ? undefined : onSshControllerChange}
            onReconnect={readOnly ? undefined : onReconnect}
          />
        </Suspense>
      );
    case "rdp":
    case "vnc":
      // DEMO MODE ONLY: no live guacamole relay — show a styled Windows desktop.
      if (IS_DEMO) {
        return (
          <Suspense fallback={<ViewerFallback />}>
            <DemoRemoteDesktop variant={protocol === "rdp" ? "windows" : "linux"} hostName={demoHostNameFromWsUrl(websocketUrl)} onReady={onOpen} />
          </Suspense>
        );
      }
      return (
        <Suspense fallback={<ViewerFallback />}>
          <LiveRDP
            key={`${sessionId}:${renewals}`}
            websocketUrl={websocketUrl}
            token={token}
            readOnly={readOnly}
            fallbackCursor={protocol === "vnc"}
            isActive={isActive}
            focusRequest={focusRequest}
            displayMode={view.displayMode}
            maxResolution={sessionViewMaxResolution(view.quality)}
            onOpen={handleOpen}
            onClose={handleClose}
            onWireClose={handleWireClose}
            onRetry={readOnly ? undefined : onReconnect}
            onFilesystem={readOnly ? undefined : onRdpFilesystem}
            onClipboardControllerChange={readOnly ? undefined : onClipboardControllerChange}
            onRemoteClipboardText={readOnly ? undefined : onRemoteClipboardText}
          />
        </Suspense>
      );
  }
  const exhaustive: never = protocol;
  throw new Error(`Unsupported live session protocol: ${String(exhaustive)}`);
};

export default LiveSessionViewer;
