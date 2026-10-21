import { useCallback, useEffect, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";
import { ArrowLeft } from "lucide-react";
import { useTranslation } from "react-i18next";

import ConnectionsPanel from "@/features/connections/components/connections/ConnectionsPanel";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";
import { closeSession as apiCloseSession } from "@/features/connections/api";
import { useSessionLaunch } from "@/features/connections/hooks/useSessionLaunch";
import SessionLaunchDialog from "@/features/connections/components/connections/SessionLaunchDialog";
import { defaultSessionPaneLayout, replacePaneSession } from "@/features/connections/components/connections/sessionPaneLayout";
import {
  clearDetachedHandoff,
  consumeInjectedDetachedGroupHandoff,
  readDetachedHandoff,
  writeDetachedHandoff,
  type DetachedHandoff,
} from "@/features/connections/detachedHandoff";
import { Button } from "@/shared/ui-primitives/button";
import { notify } from "@/shared/lib/notifications";
import { useSessionTouchLayout } from "@/features/connections/hooks/useSessionViewport";

/**
 * Group detached-window page. Mirrors the single-session detached page
 * (`/connections/detached/:id`) but holds a whole tab strip in one new window.
 *
 * Bootstrap order matches the single-detached page: the injected window
 * payload (`__PIPING_DETACHED_GROUP_HANDOFF__`, written by the parent right
 * after `window.open`) wins because it is the only source guaranteed to carry
 * fresh proxy URLs and tokens; a hard reload of THIS window falls back to the
 * per-session `localStorage` handoffs each individual session writes (those
 * may be slightly stale and the user can hit reconnect).
 */
function handoffToLiveSession(h: DetachedHandoff): LiveSession {
  return {
    id: h.sessionId,
    hostId: h.hostId,
    hostName: h.hostName,
    protocol: h.protocol,
    // CCTV console tabs have no transport to open, so nothing would ever
    // flip them out of "connecting" — they are ready as soon as they render.
    status: h.protocol === "cctv" ? "connected" : "connecting",
    websocketUrl: h.websocketUrl,
    token: h.token,
    ...(h.username ? { username: h.username } : {}),
  };
}

function readInitialSessions(orderedIds: string[]): LiveSession[] {
  const injected = consumeInjectedDetachedGroupHandoff();
  if (injected) {
    const byId = new Map(injected.map((h) => [h.sessionId, h] as const));
    const out: DetachedHandoff[] = [];
    const seen = new Set<string>();
    // Honour the URL order so reload-from-bookmark keeps the parent's tab order.
    for (const id of orderedIds) {
      const h = byId.get(id);
      if (h && !seen.has(h.sessionId)) {
        out.push(h);
        seen.add(h.sessionId);
      }
    }
    // Defensive: any injected handoff that did not appear in the URL still gets
    // rendered (a popup-blocker quirk could in principle deliver one without
    // the other).
    for (const h of injected) {
      if (!seen.has(h.sessionId)) {
        out.push(h);
        seen.add(h.sessionId);
      }
    }
    return out.map(handoffToLiveSession);
  }
  const out: LiveSession[] = [];
  for (const id of orderedIds) {
    const h = readDetachedHandoff(id);
    if (h) out.push(handoffToLiveSession(h));
  }
  return out;
}

export default function DetachedGroup() {
  const isMobile = useSessionTouchLayout();
  const navigate = useNavigate();
  const { t } = useTranslation();
  const [params] = useSearchParams();
  const orderedIds = (params.get("ids") ?? "")
    .split(",")
    .map((value) => decodeURIComponent(value.trim()))
    .filter(Boolean);

  const [sessions, setSessions] = useState<LiveSession[]>(() => readInitialSessions(orderedIds));
  const [activeSessionId, setActiveSessionId] = useState<string | null>(
    () => sessions[0]?.id ?? null,
  );
  const [paneLayout, setPaneLayout] = useState(() => defaultSessionPaneLayout(sessions[0]?.id ?? null));

  // ConnectionsPanel reads `panelSize` as raw pixels (CSS `height: <n>`), so
  // the standalone window has to keep it equal to the viewport height. Without
  // this the panel collapses to its default split-screen size and the SSH/RDP
  // viewer renders as a single line.
  const [viewportHeight, setViewportHeight] = useState<number>(() =>
    typeof window === "undefined" ? 800 : window.innerHeight,
  );
  useEffect(() => {
    const onResize = () => setViewportHeight(window.innerHeight);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const dropLocalSession = useCallback(
    (id: string) => {
      clearDetachedHandoff(id);
      setSessions((prev) => {
        const next = prev.filter((s) => s.id !== id);
        if (activeSessionId === id) {
          setActiveSessionId(next[0]?.id ?? null);
        }
        return next;
      });
    },
    [activeSessionId],
  );

  const handleCloseSession = useCallback(
    (id: string) => {
      // CCTV console tabs are client-only — nothing to close on the backend.
      const session = sessions.find((s) => s.id === id);
      if (session?.protocol !== "cctv") {
        void apiCloseSession(id).catch(() => undefined);
      }
      dropLocalSession(id);
    },
    [dropLocalSession, sessions],
  );

  const handleCloseAllSessions = useCallback(() => {
    for (const session of sessions) {
      if (session.protocol !== "cctv") {
        void apiCloseSession(session.id).catch(() => undefined);
      }
      clearDetachedHandoff(session.id);
    }
    setSessions([]);
    setActiveSessionId(null);
  }, [sessions]);

  const sessionLaunch = useSessionLaunch<string>({
    isCurrent: oldId => sessions.some(item => item.id === oldId),
    accept: (response, oldId, username) => {
      const previous = sessions.find(item => item.id === oldId);
      if (!previous) return false;
      const next: LiveSession = { ...previous, id: response.session.id, status: "connecting", websocketUrl: response.proxy.websocketUrl, token: response.proxy.token, username };
      writeDetachedHandoff(next.id, { sessionId: next.id, hostId: next.hostId, hostName: next.hostName, protocol: next.protocol, websocketUrl: next.websocketUrl, token: next.token, ...(username ? { username } : {}) });
      clearDetachedHandoff(oldId);
      setSessions(current => current.map(item => item.id === oldId ? next : item));
      setActiveSessionId(current => current === oldId ? next.id : current);
      setPaneLayout(current => replacePaneSession(current, oldId, next.id));
      void navigate({ search: `?ids=${sessions.map(item => encodeURIComponent(item.id === oldId ? next.id : item.id)).join(",")}` }, { replace: true });
      return true;
    },
  });

  const handleReconnectSession = useCallback(
    (session: LiveSession) => {
      if (session.protocol === "cctv") {
        // Client-only console: no backend session to resume — remount the tab
        // (fresh id-keyed viewer) is unnecessary; just mark it connected.
        setSessions((prev) =>
          prev.map((s) => (s.id === session.id ? { ...s, status: "connected" as const } : s)),
        );
        return;
      }
      void sessionLaunch.retry({ ...session, protocol: session.protocol }, session.id);
    },
    [sessionLaunch],
  );

  const handleSessionStatusChange = useCallback(
    (id: string, status: LiveSession["status"]) => {
      setSessions((prev) =>
        prev.map((s) => {
          if (s.id !== id) return s;
          if (s.status === "reconnecting" && status !== "connected") return s;
          if (s.status === status) return s;
          return { ...s, status };
        }),
      );
    },
    [],
  );

  /**
   * Detach a single tab out of this group window into its own dedicated window
   * (the same surface a single-detach from the in-app panel opens). Mirrors
   * `openDetachedSession` from the in-app workspace: write the per-session
   * handoff, open the single-detached page, inject the payload, and drop the
   * tab from this group so its WS is released for the new window to adopt.
   */
  const handleDetachSession = useCallback(
    (id: string) => {
      const session = sessions.find((s) => s.id === id);
      if (!session) return;
      const handoff: DetachedHandoff = {
        sessionId: session.id,
        hostName: session.hostName,
        hostId: session.hostId,
        protocol: session.protocol,
        websocketUrl: session.websocketUrl,
        token: session.token,
        ...(session.username ? { username: session.username } : {}),
      };
      writeDetachedHandoff(session.id, handoff);
      const detachedPath = `/connections/detached/${encodeURIComponent(session.id)}`;
      const detachedWindow = window.open(detachedPath, "_blank");
      if (!detachedWindow) {
        notify("warning", t("connections.detachSession"), {
          description: t("connections.popupBlocked"),
          source: "connections",
        });
        return;
      }
      try {
        detachedWindow.__PIPING_DETACHED_HANDOFF__ = handoff;
      } catch {
        // Cross-origin shouldn't happen for our own route; the single-detached
        // page still recovers from localStorage.
      }
      // Drop from this group; the single detached window now owns the WS.
      setSessions((prev) => {
        const next = prev.filter((s) => s.id !== id);
        if (activeSessionId === id) {
          setActiveSessionId(next[0]?.id ?? null);
        }
        return next;
      });
    },
    [activeSessionId, sessions, t],
  );

  const handleReorderSessions = useCallback((fromIndex: number, toIndex: number) => {
    setSessions((prev) => {
      if (
        fromIndex < 0 || fromIndex >= prev.length ||
        toIndex < 0 || toIndex >= prev.length ||
        fromIndex === toIndex
      ) return prev;
      const next = prev.slice();
      const [moved] = next.splice(fromIndex, 1);
      next.splice(toIndex, 0, moved!);
      return next;
    });
  }, []);

  if (sessions.length === 0) {
    return (
      <div className="fixed inset-0 flex flex-col items-center justify-center gap-3 bg-background p-6 text-center">
        <p className="text-sm text-muted-foreground">{t("connections.detachedGroupEmpty")}</p>
        <Button asChild variant="secondary" size="sm">
          <Link to="/connections">
            <ArrowLeft className="mr-1.5 h-4 w-4" />
            {t("connections.detachedGroupBack")}
          </Link>
        </Button>
      </div>
    );
  }

  return (
    <div className="fixed inset-0 flex min-h-0 flex-col bg-background text-sm text-foreground">
      <SessionLaunchDialog controller={sessionLaunch} />
      <ConnectionsPanel
        paneLayout={paneLayout}
        onPaneLayoutChange={setPaneLayout}
        mobile={isMobile}
        sessions={sessions}
        activeSessionId={activeSessionId}
        onSelectSession={(id) => setActiveSessionId(id)}
        onCloseSession={handleCloseSession}
        onCloseAllSessions={handleCloseAllSessions}
        onDetachSession={handleDetachSession}
        onReorderSessions={handleReorderSessions}
        onReconnectSession={handleReconnectSession}
        onSessionStatusChange={handleSessionStatusChange}
        playbookTabs={[]}
        activePlaybookTabId={null}
        onSelectPlaybookTab={() => undefined}
        onClosePlaybookTab={() => undefined}
        expanded={true}
        onToggleExpand={() => { if (isMobile) void navigate("/connections"); }}
        dockPosition="bottom"
        onChangeDock={() => undefined}
        panelSize={viewportHeight}
        onPanelResize={() => undefined}
      />
    </div>
  );
}
