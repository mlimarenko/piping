import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { useSessionLaunch } from "@/features/connections/hooks/useSessionLaunch";
import SessionLaunchDialog from "@/features/connections/components/connections/SessionLaunchDialog";
import { ArrowLeft, Loader2, Monitor, RotateCcw, Terminal, Video, Wifi, WifiOff } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import LiveSessionViewer from "@/features/connections/components/connections/LiveSessionViewer";
import SessionHealthBar from "@/features/connections/components/connections/SessionHealthBar";
import SessionToolsMenu from "@/features/connections/components/connections/SessionToolsMenu";
import RdpFileBrowserPanel from "@/features/connections/components/connections/RdpFileBrowserPanel";
import SshFileBrowserPanel from "@/features/connections/components/connections/SshFileBrowserPanel";
import type { RdpFilesystem } from "@/features/connections/adapters/ws/rdpFilesystem";
import {
  writeBrowserClipboardText,
  type RemoteClipboardController,
} from "@/features/connections/adapters/ws/remoteClipboard";
import { closeSession as apiCloseSession, resumeSession } from "@/features/connections/api";
import {
  isTerminalSessionResumeError,
  SESSION_ATTACH_RACE_MAX_RETRIES,
  SESSION_RESUME_MAX_RETRIES,
  sessionAttachRaceRetryDelayMs,
  sessionResumeRetryDelayMs,
} from "@/features/connections/sessionRestore";
import type { ViewerSessionStatus } from "@/shared/api/types";
import {
  consumeInjectedDetachedHandoff,
  clearDetachedHandoff,
  readDetachedHandoff,
  writeDetachedHandoff,
  type DetachedHandoff,
} from "@/features/connections/detachedHandoff";
import {
  clampSshFileBrowserSize,
  loadSshFileBrowserState,
  saveSshFileBrowserState,
  type FileBrowserPlacement,
  type PersistedSshFileBrowserState,
} from "@/features/connections/components/connections/fileBrowserState";
import { useDocumentTitle } from "@/shared/hooks/useDocumentTitle";
import { useBranding } from "@/app/BrandingContext";
import { useSessionTouchLayout, useSessionViewport } from "@/features/connections/hooks/useSessionViewport";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";

type RestoreState =
  | { connectionId: string | null; phase: "restoring"; handoff: DetachedHandoff | null }
  | { connectionId: string | null; phase: "ready"; handoff: DetachedHandoff }
  | { connectionId: string | null; phase: "ended"; handoff: DetachedHandoff | null };

const DETACHED_FILE_BROWSER_SIZE: Record<FileBrowserPlacement, number> = {
  bottom: 176,
  right: 420,
};

const restoreStateFor = (
  connectionId: string | undefined,
  handoff: DetachedHandoff | null,
): RestoreState => {
  if (!connectionId) {
    return { connectionId: null, phase: "ended", handoff: null };
  }
  if (handoff?.sessionId === connectionId) {
    return { connectionId, phase: "ready", handoff };
  }
  return { connectionId, phase: "restoring", handoff };
};

const DetachedSession = () => {
  const { t } = useTranslation();
  const isMobile = useSessionTouchLayout();
  const viewportRef = useSessionViewport(isMobile);
  const { connectionId } = useParams();
  const navigate = useNavigate();

  const initialHandoff = useMemo(
    () =>
      connectionId
        ? consumeInjectedDetachedHandoff(connectionId) ?? readDetachedHandoff(connectionId)
        : null,
    [connectionId],
  );
  const [restoreState, setRestoreState] = useState<RestoreState>(() =>
    restoreStateFor(connectionId, initialHandoff),
  );
  const activeRestoreState: RestoreState =
    restoreState.connectionId === (connectionId ?? null)
      ? restoreState
      : restoreStateFor(connectionId, initialHandoff);
  const handoff = activeRestoreState.handoff;
  const hostName = handoff?.hostName ?? "unknown";
  const protocol = handoff?.protocol ?? "ssh";
  const { appName } = useBranding();

  // Each detached connection owns its tab title so operators can tell several
  // open session tabs apart at a glance: `<host> · <PROTOCOL>[ · <user>]`,
  // prefixed with the instance name. The remote `username` is shown only when
  // the handoff carries it (see DetachedHandoff). Resolves from the
  // best-available handoff so even the restoring/ended states stay labelled.
  useDocumentTitle([
    appName,
    handoff?.hostName,
    handoff ? protocol.toUpperCase() : null,
    handoff?.username,
  ]);

  const [status, setStatus] = useState<ViewerSessionStatus>(
    connectionId ? "connecting" : "failed",
  );
  const [viewerAttempt, setViewerAttempt] = useState(0);
  const attachRaceRetryRef = useRef(0);
  const attachRaceTimerRef = useRef<number | undefined>(undefined);
  const handleViewerCloseRef =
    useRef<(clean: boolean, code?: number, reason?: string) => void>(() => {});
  const [fileBrowserOpen, setFileBrowserOpen] = useState(false);
  const [rdpFilesystem, setRdpFilesystem] = useState<RdpFilesystem | null>(null);
  const [clipboardController, setClipboardController] =
    useState<RemoteClipboardController | null>(null);
  const [fileBrowserState, setFileBrowserState] = useState(loadSshFileBrowserState);
  const readyHandoff =
    activeRestoreState.phase === "ready" ? activeRestoreState.handoff : null;
  const activeSession = useMemo<LiveSession | null>(() => {
    if (!readyHandoff) return null;
    return {
      id: readyHandoff.sessionId,
      hostId: readyHandoff.hostId,
      hostName: readyHandoff.hostName,
      protocol: readyHandoff.protocol,
      websocketUrl: readyHandoff.websocketUrl,
      token: readyHandoff.token,
      status,
      ...(readyHandoff.username ? { username: readyHandoff.username } : {}),
    };
  }, [readyHandoff, status]);

  const resetSessionTools = useCallback(() => {
    setFileBrowserOpen(false);
    setRdpFilesystem(null);
    setClipboardController(null);
  }, []);

  const updateFileBrowserState = useCallback((
    updater: (state: PersistedSshFileBrowserState) => PersistedSshFileBrowserState,
  ) => {
    setFileBrowserState((current) => {
      const next = updater(current);
      saveSshFileBrowserState(next);
      return next;
    });
  }, []);

  useEffect(() => {
    if (!connectionId) {
      return;
    }

    const storedHandoff =
      consumeInjectedDetachedHandoff(connectionId) ?? readDetachedHandoff(connectionId);
    attachRaceRetryRef.current = 0;
    if (attachRaceTimerRef.current !== undefined) {
      window.clearTimeout(attachRaceTimerRef.current);
      attachRaceTimerRef.current = undefined;
    }
    let cancelled = false;
    let timer: number | undefined;

    if (storedHandoff?.sessionId === connectionId) {
      window.queueMicrotask(() => {
        if (cancelled) return;
        resetSessionTools();
        // The CCTV console has no transport to open, so nothing ever flips the
        // status to connected — set it here instead of waiting on the viewer.
        setStatus(storedHandoff.protocol === "cctv" ? "connected" : "connecting");
        setRestoreState({ connectionId, phase: "ready", handoff: storedHandoff });
      });
      return () => {
        cancelled = true;
        if (attachRaceTimerRef.current !== undefined) {
          window.clearTimeout(attachRaceTimerRef.current);
          attachRaceTimerRef.current = undefined;
        }
      };
    }

    // A CCTV console tab is client-only: there is no backend session to
    // resume. Rebuild the handoff from the `cctv:<targetId>` id so even a
    // reload with cleared storage lands on a working console.
    if (connectionId.startsWith("cctv:")) {
      const targetId = connectionId.slice("cctv:".length);
      const synthesized: DetachedHandoff = {
        sessionId: connectionId,
        hostId: targetId,
        hostName: targetId,
        protocol: "cctv",
        websocketUrl: "",
        token: "",
      };
      window.queueMicrotask(() => {
        if (cancelled) return;
        resetSessionTools();
        setStatus("connected");
        writeDetachedHandoff(connectionId, synthesized);
        setRestoreState({ connectionId, phase: "ready", handoff: synthesized });
      });
      return () => {
        cancelled = true;
        if (attachRaceTimerRef.current !== undefined) {
          window.clearTimeout(attachRaceTimerRef.current);
          attachRaceTimerRef.current = undefined;
        }
      };
    }

    const restore = async (attempt: number) => {
      try {
        const resumed = await resumeSession(connectionId);
        if (cancelled) return;
        const nextHandoff: DetachedHandoff = {
          sessionId: resumed.session.id,
          hostId: resumed.session.hostId,
          hostName: storedHandoff?.hostName ?? resumed.session.hostName ?? resumed.session.hostId,
          protocol: resumed.session.protocol,
          websocketUrl: resumed.proxy.websocketUrl,
          token: resumed.proxy.token,
          ...(storedHandoff?.username ? { username: storedHandoff.username } : {}),
        };
        writeDetachedHandoff(connectionId, nextHandoff);
        setRestoreState({ connectionId, phase: "ready", handoff: nextHandoff });
        setViewerAttempt((attempt) => attempt + 1);
      } catch (error) {
        if (cancelled) return;
        if (!isTerminalSessionResumeError(error) && attempt < SESSION_RESUME_MAX_RETRIES) {
          timer = window.setTimeout(() => {
            void restore(attempt + 1);
          }, sessionResumeRetryDelayMs(attempt));
          return;
        }
        setRestoreState({ connectionId, phase: "ended", handoff: storedHandoff });
        setStatus("failed");
      }
    };

    window.queueMicrotask(() => {
      if (cancelled) return;
      resetSessionTools();
      setStatus("connecting");
      setRestoreState({ connectionId, phase: "restoring", handoff: storedHandoff });
      void restore(0);
    });

    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
      if (attachRaceTimerRef.current !== undefined) {
        window.clearTimeout(attachRaceTimerRef.current);
        attachRaceTimerRef.current = undefined;
      }
    };
  }, [connectionId, resetSessionTools]);

  const handleViewerOpen = useCallback(() => {
    setStatus("connected");
    attachRaceRetryRef.current = 0;
  }, []);

  const handleVisualRouteLost = useCallback(() => {
    resetSessionTools();
    setStatus("failed");
  }, [resetSessionTools]);

  const handleViewerClose = useCallback(
    (clean: boolean, _code?: number, reason?: string) => {
      if (clean) {
        resetSessionTools();
        setStatus("disconnected");
        return;
      }

      const attachRace =
        activeRestoreState.phase === "ready" &&
        connectionId &&
        reason?.toLowerCase().includes("already attached") === true;

      if (!attachRace || attachRaceRetryRef.current >= SESSION_ATTACH_RACE_MAX_RETRIES) {
        resetSessionTools();
        setStatus("failed");
        return;
      }

      const attempt = attachRaceRetryRef.current;
      attachRaceRetryRef.current += 1;
      setStatus("connecting");
      if (attachRaceTimerRef.current !== undefined) {
        window.clearTimeout(attachRaceTimerRef.current);
      }
      attachRaceTimerRef.current = window.setTimeout(() => {
        attachRaceTimerRef.current = undefined;
        void resumeSession(connectionId)
          .then((resumed) => {
            const nextHandoff: DetachedHandoff = {
              sessionId: resumed.session.id,
              hostId: resumed.session.hostId,
              hostName:
                activeRestoreState.handoff.hostName ??
                resumed.session.hostName ??
                resumed.session.hostId,
              protocol: resumed.session.protocol,
              websocketUrl: resumed.proxy.websocketUrl,
              token: resumed.proxy.token,
              ...(activeRestoreState.handoff.username
                ? { username: activeRestoreState.handoff.username }
                : {}),
            };
            writeDetachedHandoff(connectionId, nextHandoff);
            setRestoreState({ connectionId, phase: "ready", handoff: nextHandoff });
            setViewerAttempt((currentAttempt) => currentAttempt + 1);
          })
          .catch((error) => {
            if (
              !isTerminalSessionResumeError(error) &&
              attachRaceRetryRef.current < SESSION_ATTACH_RACE_MAX_RETRIES
            ) {
              handleViewerCloseRef.current(false, undefined, "already attached");
              return;
            }
            setRestoreState({
              connectionId,
              phase: "ended",
              handoff: activeRestoreState.handoff,
            });
            resetSessionTools();
            setStatus("failed");
          });
      }, sessionAttachRaceRetryDelayMs(attempt));
    },
    [activeRestoreState, connectionId, resetSessionTools],
  );

  useEffect(() => {
    handleViewerCloseRef.current = handleViewerClose;
  }, [handleViewerClose]);

  const handleRdpFilesystem = useCallback((fs: RdpFilesystem | null) => {
    setRdpFilesystem(fs);
    if (!fs) setFileBrowserOpen(false);
  }, []);

  const handleClipboardController = useCallback((controller: RemoteClipboardController | null) => {
    setClipboardController(controller);
  }, []);

  const handleRemoteClipboardText = useCallback((session: LiveSession, text: string) => {
    void writeBrowserClipboardText(text).then(
      () => {
        notify("success", t("connections.clipboard.received"), {
          description: t("connections.clipboard.receivedDescription", {
            protocol: session.protocol.toUpperCase(),
            host: session.hostName,
          }),
          source: "connections",
        });
      },
      () => {
        notify("warning", t("connections.clipboard.receiveFailed"), {
          description: t("connections.clipboard.receiveFailedDescription"),
          source: "connections",
        });
      },
    );
  }, [t]);

  const handleSendClipboard = useCallback(() => {
    if (!activeSession || !clipboardController) return;
    if (!navigator.clipboard?.readText) {
      notify("warning", t("connections.clipboard.sendUnavailable"), {
        description: t("connections.clipboard.sendUnavailableDescription"),
        source: "connections",
      });
      return;
    }
    void navigator.clipboard.readText().then(
      (text) => {
        if (!text) {
          notify("info", t("connections.clipboard.empty"), { source: "connections" });
          return;
        }
        try {
          clipboardController.sendText(text);
          notify("success", t("connections.clipboard.sent"), {
            description: t("connections.clipboard.sentDescription", {
              protocol: activeSession.protocol.toUpperCase(),
              host: activeSession.hostName,
            }),
            source: "connections",
          });
        } catch {
          notify("error", t("connections.clipboard.sendFailed"), {
            description: t("connections.clipboard.sendFailedDescription"),
            source: "connections",
          });
        }
      },
      () => {
        notify("warning", t("connections.clipboard.readFailed"), {
          description: t("connections.clipboard.readFailedDescription"),
          source: "connections",
        });
      },
    );
  }, [activeSession, clipboardController, t]);

  const sessionLaunch = useSessionLaunch<string>({
    isCurrent: oldId => activeRestoreState.phase === "ready" && activeSession?.id === oldId,
    accept: (response, oldId, username) => {
      if (activeRestoreState.phase !== "ready" || activeSession?.id !== oldId) return false;
      const next: DetachedHandoff = {
        sessionId: response.session.id, hostId: response.session.hostId,
        hostName: activeSession.hostName, protocol: response.session.protocol,
        websocketUrl: response.proxy.websocketUrl, token: response.proxy.token,
        ...(username ? { username } : {}),
      };
      writeDetachedHandoff(next.sessionId, next);
      clearDetachedHandoff(oldId);
      resetSessionTools();
      setStatus("connecting");
      setRestoreState({ connectionId: next.sessionId, phase: "ready", handoff: next });
      void navigate(`/connections/detached/${encodeURIComponent(next.sessionId)}`, { replace: true });
      return true;
    },
  });

  const handleReconnectSession = useCallback((session: LiveSession) => {
    if (!connectionId) return;
    if (session.protocol === "cctv") {
      // Client-only console: remount the viewer so every device query refetches.
      setStatus("connected");
      setViewerAttempt((attempt) => attempt + 1);
      return;
    }
    void sessionLaunch.retry({ ...session, protocol: session.protocol }, session.id);
  }, [connectionId, sessionLaunch]);

  const handleCloseSession = useCallback((sessionId: string) => {
    const lastHandoff = activeRestoreState.handoff;
    resetSessionTools();
    setStatus("disconnected");
    if (connectionId) clearDetachedHandoff(connectionId);
    if (sessionId !== connectionId) clearDetachedHandoff(sessionId);
    // CCTV console tabs have no backend session to close.
    if (lastHandoff?.protocol === "cctv" || sessionId.startsWith("cctv:")) {
      setRestoreState({
        connectionId: connectionId ?? sessionId,
        phase: "ended",
        handoff: lastHandoff,
      });
      return;
    }
    void apiCloseSession(sessionId).finally(() => {
      setRestoreState({
        connectionId: connectionId ?? sessionId,
        phase: "ended",
        handoff: lastHandoff,
      });
    });
  }, [activeRestoreState.handoff, connectionId, resetSessionTools]);

  const handleToggleFileBrowser = useCallback(() => {
    setFileBrowserOpen((current) => !current);
  }, []);

  if (!connectionId) {
    return (
      <div className="h-[var(--app-vh)] bg-background">
        <div className="flex h-full items-center justify-center">
          <div className="border border-border bg-card rounded-lg p-8 text-center">
            <p className="text-foreground font-medium mb-2">
              {t("detachedSession.invalidId")}
            </p>
            <Link to="/connections">
              <Button variant="outline" size="sm" className="gap-1.5">
                <ArrowLeft className="h-3 w-3" />
                {t("detachedSession.backToWorkspace")}
              </Button>
            </Link>
          </div>
        </div>
      </div>
    );
  }

  if (activeRestoreState.phase !== "ready") {
    const restoring = activeRestoreState.phase === "restoring";
    return (
      <div className="h-[var(--app-vh)] bg-background">
        <div className="flex h-full items-center justify-center">
          <div className="border border-border bg-card rounded-lg p-8 text-center">
            <div className="mb-3 flex justify-center">
              {restoring ? (
                <Loader2
                  className="h-6 w-6 animate-spin text-status-warning"
                  data-testid="detached-session-restoring"
                />
              ) : (
                <WifiOff className="h-6 w-6 text-status-negative" />
              )}
            </div>
            <p className="text-foreground font-medium mb-2">
              {restoring
                ? t("detachedSession.restoring")
                : t("connections.sessionEnded")}
            </p>
            {!restoring && (
              <Link to="/connections">
                <Button variant="outline" size="sm" className="gap-1.5">
                  <ArrowLeft className="h-3 w-3" />
                  {t("detachedSession.backToWorkspace")}
                </Button>
              </Link>
            )}
          </div>
        </div>
      </div>
    );
  }
  const activeHandoff = activeRestoreState.handoff;
  if (!activeSession) return null;
  const fileBrowserPlacement = fileBrowserState.placement;
  const fileBrowserSize = clampSshFileBrowserSize(
    fileBrowserPlacement,
    fileBrowserState.sizes[fileBrowserPlacement] ?? DETACHED_FILE_BROWSER_SIZE[fileBrowserPlacement],
  );
  const sessionSupportsFileBrowser =
    activeSession.status === "connected" &&
    (activeSession.protocol === "ssh" ||
      (activeSession.protocol === "rdp" && !!rdpFilesystem));
  const showFileBrowser = fileBrowserOpen && sessionSupportsFileBrowser;

  return (
    <div ref={viewportRef} className={cn("h-[var(--app-vh)] w-screen overflow-hidden bg-background text-sm", isMobile && "fixed inset-x-0 pt-[env(safe-area-inset-top)]")}
      style={isMobile ? { height: "var(--session-viewport-height, 100dvh)", top: "var(--session-viewport-top, 0px)" } : undefined}>
      <SessionLaunchDialog controller={sessionLaunch} />
      <div className="flex h-full min-h-0 flex-col">
        <header className="flex h-11 md:h-9 min-w-0 shrink-0 items-center justify-between gap-2 border-b border-border bg-card px-2">
          <div className="flex min-w-0 items-center gap-2">
            <Link to="/connections">
              <Button
                variant="ghost"
                size="icon"
                className="size-11 md:size-9"
                aria-label={t("detachedSession.back")}
              >
                <ArrowLeft className="h-3.5 w-3.5" />
              </Button>
            </Link>
            {protocol === "ssh" ? (
              <Terminal className="h-3.5 w-3.5 text-status-positive" />
            ) : protocol === "cctv" ? (
              <Video className="h-3.5 w-3.5 text-accent-foreground" />
            ) : (
              <Monitor className="h-3.5 w-3.5 text-status-info" />
            )}
            <span className="truncate text-sm font-medium text-foreground">{hostName}</span>
            <span className="font-mono text-xs uppercase text-muted-foreground">
              {protocol}
            </span>
          </div>
          <div className="flex items-center gap-1.5 text-xs">
            {status === "connecting" && (
              <>
                <Loader2 className="h-3 w-3 animate-spin text-status-warning" />
                <span className="text-status-warning">
                  {t("detachedSession.connecting")}
                </span>
              </>
            )}
            {status === "connected" && (
              <>
                <Wifi className="h-3 w-3 text-status-positive" />
                <span className="text-status-positive">
                  {t("detachedSession.connected")}
                </span>
              </>
            )}
            {status === "disconnected" && (
              <>
                <WifiOff className="h-3 w-3 text-muted-foreground" />
                <span className="text-muted-foreground">
                  {t("detachedSession.disconnected")}
                </span>
              </>
            )}
            {status === "failed" && (
              <>
                <WifiOff className="h-3 w-3 text-status-negative" />
                <span className="text-status-negative">
                  {t("detachedSession.failed")}
                </span>
              </>
            )}
            {activeSession.protocol !== "cctv" && (
              <SessionHealthBar
                sessionId={activeHandoff.sessionId}
                enabled={activeRestoreState.phase === "ready"}
                viewerStatus={status}
                className="size-11 md:size-9"
                onConnectionLost={activeSession.protocol === "rdp" || activeSession.protocol === "vnc"
                  ? handleVisualRouteLost : undefined}
              />
            )}
            <SessionToolsMenu
              session={activeSession}
              onDetachSession={() => undefined}
              onReconnectSession={handleReconnectSession}
              onCloseSession={handleCloseSession}
              onCloseAllSessions={() => handleCloseSession(activeSession.id)}
              isFileBrowserOpen={showFileBrowser}
              isRdpDriveReady={!!rdpFilesystem}
              isVncSshFileBrowserReady={false}
              isClipboardReady={!!clipboardController}
              onSendClipboard={clipboardController ? handleSendClipboard : undefined}
              onToggleFileBrowser={
                activeSession.protocol === "ssh" || activeSession.protocol === "rdp"
                  ? handleToggleFileBrowser
                  : undefined
              }
              showDetachAction={false}
              showCloseAllAction={false}
            />
          </div>
        </header>

        <div
          className={cn(
            "flex min-h-0 flex-1 overflow-hidden",
            showFileBrowser && fileBrowserPlacement === "right" ? "flex-row" : "flex-col",
          )}
        >
          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            <div className="relative min-h-0 flex-1">
              <LiveSessionViewer
                key={`${activeHandoff.sessionId}:${viewerAttempt}`}
                sessionId={activeHandoff.sessionId}
                protocol={activeHandoff.protocol}
                websocketUrl={activeHandoff.websocketUrl}
                token={activeHandoff.token}
                targetId={activeHandoff.hostId}
                isActive
                onOpen={handleViewerOpen}
                onClose={handleViewerClose}
                onReconnect={() => handleReconnectSession(activeSession)}
                onRdpFilesystem={
                  activeHandoff.protocol === "rdp" ? handleRdpFilesystem : undefined
                }
                onClipboardControllerChange={
                  activeHandoff.protocol === "rdp" || activeHandoff.protocol === "vnc"
                    ? handleClipboardController
                    : undefined
                }
                onRemoteClipboardText={
                  activeHandoff.protocol === "rdp" || activeHandoff.protocol === "vnc"
                    ? (text) => handleRemoteClipboardText(activeSession, text)
                    : undefined
                }
              />
              {(status === "disconnected" || status === "failed") &&
                (activeSession.protocol === "rdp" || activeSession.protocol === "vnc") && (
                  <div className="absolute inset-0 z-20 flex flex-col items-center justify-center gap-3 bg-background p-4 text-center">
                    <WifiOff className="h-8 w-8 text-muted-foreground" />
                    <div className="space-y-1">
                      <p className="text-sm font-medium text-foreground">
                        {t(
                          status === "failed"
                            ? "connections.connectionLost"
                            : "connections.sessionEnded",
                        )}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {activeSession.protocol.toUpperCase()} · {activeSession.hostName}
                      </p>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-8 gap-1.5 text-sm"
                      onClick={() => handleReconnectSession(activeSession)}
                    >
                      <RotateCcw className="h-3.5 w-3.5" />
                      {t("connections.reconnectSession")}
                    </Button>
                  </div>
              )}
            </div>
          </div>
          {showFileBrowser ? (
            <div
              className={cn(
                "min-h-0 min-w-0 overflow-hidden border-border bg-background",
                fileBrowserPlacement === "right" ? "border-l" : "border-t",
              )}
              style={
                fileBrowserPlacement === "right"
                  ? { width: fileBrowserSize }
                  : { height: fileBrowserSize }
              }
            >
              {activeSession.protocol === "rdp" && rdpFilesystem ? (
                <RdpFileBrowserPanel
                  session={activeSession}
                  filesystem={rdpFilesystem}
                  placement={fileBrowserPlacement}
                  onChangePlacement={(placement) => {
                    updateFileBrowserState((current) => ({
                      ...current,
                      placement,
                      sizes: {
                        ...current.sizes,
                        [placement]: clampSshFileBrowserSize(
                          placement,
                          current.sizes[placement],
                        ),
                      },
                    }));
                  }}
                  onClose={() => setFileBrowserOpen(false)}
                />
              ) : (
                <SshFileBrowserPanel
                  session={activeSession}
                  placement={fileBrowserPlacement}
                  onChangePlacement={(placement) => {
                    updateFileBrowserState((current) => ({
                      ...current,
                      placement,
                      sizes: {
                        ...current.sizes,
                        [placement]: clampSshFileBrowserSize(
                          placement,
                          current.sizes[placement],
                        ),
                      },
                    }));
                  }}
                  onClose={() => setFileBrowserOpen(false)}
                />
              )}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
};

export default DetachedSession;
