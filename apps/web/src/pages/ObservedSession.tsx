import { useMemo } from "react";
import { Link, useParams } from "react-router";
import { ArrowLeft, MonitorOff, MonitorPlay } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import LiveSessionViewer from "@/features/connections/components/connections/LiveSessionViewer";
import { readObservationHandoff } from "@/features/connections/observation";
import { useDocumentTitle } from "@/shared/hooks/useDocumentTitle";
import { useBranding } from "@/app/BrandingContext";

/**
 * Standalone read-only observation window, opened from the active-connections
 * inspector. The handoff snapshot lives in local storage so a reload keeps the
 * same session without a second list query; the observe relay still rechecks
 * the current grants on every handshake.
 */
const ObservedSession = () => {
  const { t } = useTranslation();
  const { appName } = useBranding();
  const { sessionId } = useParams();
  const handoff = useMemo(
    () => (sessionId ? readObservationHandoff(sessionId) : null),
    [sessionId],
  );
  const activeHandoff = handoff?.sessionId === sessionId ? handoff : null;

  useDocumentTitle([
    appName,
    activeHandoff?.hostName,
    activeHandoff ? activeHandoff.protocol.toUpperCase() : null,
    activeHandoff ? t("activeConnections.readOnly") : t("activeConnections.title"),
  ]);

  if (!activeHandoff) {
    return (
      <div className="flex h-[var(--app-vh)] w-screen items-center justify-center bg-background p-4 text-center">
        <div className="flex max-w-sm flex-col items-center gap-3">
          <MonitorOff className="h-8 w-8 text-muted-foreground/50" />
          <p className="text-sm font-medium">{t("activeConnections.observeWindowMissing")}</p>
          <Link to="/connections/active">
            <Button variant="outline" size="sm" className="gap-1.5">
              <ArrowLeft className="h-3.5 w-3.5" />
              {t("activeConnections.backToActiveConnections")}
            </Button>
          </Link>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-[var(--app-vh)] w-screen flex-col gap-3 overflow-hidden bg-background p-3 text-sm">
      <header className="flex flex-wrap items-center gap-2 border-b border-border/40 pb-2">
        <MonitorPlay className="h-4 w-4 shrink-0 text-muted-foreground" />
        <h1 className="min-w-0 truncate text-sm font-semibold">{activeHandoff.hostName}</h1>
        <span className="surface-chip shrink-0 rounded px-2 py-0.5 text-xs font-medium uppercase">
          {activeHandoff.protocol}
        </span>
        {activeHandoff.userLabel && (
          <span className="min-w-0 truncate text-xs text-muted-foreground">
            {activeHandoff.userLabel}
          </span>
        )}
        <span className="surface-chip ml-auto shrink-0 rounded px-2 py-0.5 text-xs font-medium">
          {t("activeConnections.readOnly")}
        </span>
        <Link to="/connections/active" className="shrink-0">
          <Button variant="ghost" size="sm" className="gap-1.5">
            <ArrowLeft className="h-3.5 w-3.5" />
            {t("activeConnections.backToActiveConnections")}
          </Button>
        </Link>
      </header>
      <div className="min-h-0 flex-1 overflow-hidden rounded-lg border border-border/60">
        <LiveSessionViewer
          sessionId={activeHandoff.sessionId}
          protocol={activeHandoff.protocol}
          websocketUrl={activeHandoff.websocketUrl}
          token=""
          hostId={activeHandoff.hostId}
          isActive
          readOnly
        />
      </div>
    </div>
  );
};

export default ObservedSession;
