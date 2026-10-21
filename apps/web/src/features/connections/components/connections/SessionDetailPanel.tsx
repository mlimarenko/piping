import { Activity, Copy, Link2Off, Power } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import { notify } from "@/shared/lib/notifications";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type { ActiveSession } from "@/shared/api/types";

interface Props {
  session: ActiveSession | null;
  onDisconnect: (id: string) => void;
  onTerminate: (id: string) => void;
}

function SectionTitle({ children }: { children: React.ReactNode }) {
  return (
    <h3 className="text-sm font-semibold text-foreground">
      {children}
    </h3>
  );
}

function Row({ label, value, action }: { label: string; value: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-start gap-2 py-1">
      <dt className="w-28 shrink-0 text-xs text-muted-foreground">{label}</dt>
      <dd className="min-w-0 flex-1 break-words text-sm text-foreground">{value || "—"}</dd>
      {action && <div className="shrink-0">{action}</div>}
    </div>
  );
}

const SessionDetailPanel = ({ session, onDisconnect, onTerminate }: Props) => {
  const { t } = useTranslation();
  // View-but-not-edit: session lifecycle (disconnect/terminate) is enforced as
  // Update on CONNECTION_SESSION server-side. Hide the affordances the caller
  // can't use; read-only views stay visible.
  const { can } = usePermissions();
  const canManageSession = can("UPDATE", "CONNECTION_SESSION");

  const copy = (v: string, label: string) => {
    void navigator.clipboard.writeText(v);
    notify("info", t("activeConnections.details.copied", { label }));
  };

  if (!session) {
    return (
      <div className="flex min-h-48 w-full flex-col items-center justify-center gap-4 border-t border-border py-6 text-center">
        <Activity className="h-10 w-10 text-muted-foreground/40" />
        <p className="text-sm font-medium">{t("activeConnections.details.empty.title")}</p>
        <p className="mx-auto max-w-xs text-xs text-muted-foreground text-balance">{t("activeConnections.details.empty.description")}</p>
      </div>
    );
  }

  const connectedAtFormatted = (() => {
    try {
      return new Date(session.connectedAt).toLocaleString();
    } catch {
      return session.connectedAt;
    }
  })();

  const lastActivityFormatted = (() => {
    try {
      return new Date(session.lastActivity).toLocaleString();
    } catch {
      return session.lastActivity;
    }
  })();

  return (
    <section aria-label={t("activeConnections.details.title")} className="flex min-w-0 flex-col gap-4 border-t border-border pt-4 xl:border-l xl:border-t-0 xl:pl-4 xl:pt-0">
      {/* Header */}
      <div className="flex items-center gap-2 border-b border-border/30 pb-3">
        <Activity className="h-4 w-4 text-muted-foreground shrink-0" />
        <h2 className="text-sm font-semibold">{t("activeConnections.details.title")}</h2>
        <span className="ml-auto surface-chip px-2 py-0.5 rounded text-xs font-medium uppercase">
          {session.protocol}
        </span>
      </div>

      {/* Identity */}
      <section>
        <SectionTitle>{t("activeConnections.details.section.identity")}</SectionTitle>
        <dl className="space-y-0.5">
          <Row label={t("activeConnections.host")} value={session.hostName ?? session.hostId.slice(0, 8) + "…"} />
          <Row
            label={t("activeConnections.user")}
            value={session.userDisplayName ?? session.userEmail ?? session.userId.slice(0, 8) + "…"}
          />
          {session.userEmail && (
            <Row label={t("activeConnections.details.email")} value={session.userEmail} />
          )}
        </dl>
      </section>

      {/* Network */}
      {(session.clientIp ?? session.proxyNodeName) && (
        <section>
          <SectionTitle>{t("activeConnections.details.section.network")}</SectionTitle>
          <dl className="space-y-0.5">
            {session.clientIp && <Row label={t("activeConnections.peerIp")} value={session.clientIp} />}
            {session.proxyNodeName && <Row label={t("activeConnections.proxyNode")} value={session.proxyNodeName} />}
          </dl>
        </section>
      )}

      {/* Timing */}
      <section>
        <SectionTitle>{t("activeConnections.details.section.timing")}</SectionTitle>
        <dl className="space-y-0.5">
          <Row label={t("activeConnections.connectedAt")} value={connectedAtFormatted} />
          <Row label={t("activeConnections.lastActivity")} value={lastActivityFormatted} />
        </dl>
      </section>

      {/* Identifiers */}
      <section>
        <SectionTitle>{t("activeConnections.details.section.identifiers")}</SectionTitle>
        <dl className="space-y-0.5">
          <Row
            label={t("activeConnections.details.sessionId")}
            value={<span className="font-mono text-[11px]">{session.id.slice(0, 8)}…</span>}
            action={
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6"
                title={t("activeConnections.copySessionId")} aria-label={t("activeConnections.copySessionId")}
                onClick={() => copy(session.id, t("activeConnections.copySessionId"))}
              >
                <Copy className="h-3 w-3" />
              </Button>
            }
          />
          <Row
            label={t("activeConnections.details.hostId")}
            value={<span className="font-mono text-[11px]">{session.hostId.slice(0, 8)}…</span>}
            action={
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6"
                title={t("activeConnections.copyHostId")} aria-label={t("activeConnections.copyHostId")}
                onClick={() => copy(session.hostId, t("activeConnections.copyHostId"))}
              >
                <Copy className="h-3 w-3" />
              </Button>
            }
          />
          <Row
            label={t("activeConnections.details.userId")}
            value={<span className="font-mono text-[11px]">{session.userId.slice(0, 8)}…</span>}
            action={
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6"
                title={t("activeConnections.copyUserId")} aria-label={t("activeConnections.copyUserId")}
                onClick={() => copy(session.userId, t("activeConnections.copyUserId"))}
              >
                <Copy className="h-3 w-3" />
              </Button>
            }
          />
        </dl>
      </section>

      {/* Actions */}
      {canManageSession && (
        <section className="flex flex-col gap-2 border-t border-border pt-4">
          <SectionTitle>{t("activeConnections.details.section.actions")}</SectionTitle>
          <Button
            variant="outline"
            size="sm"
            className="w-full gap-1.5 text-sm h-9"
            onClick={() => onDisconnect(session.id)}
          >
            <Link2Off className="h-3.5 w-3.5" />
            {t("activeConnections.disconnect")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="w-full gap-1.5 text-sm h-9 text-destructive hover:text-destructive"
            onClick={() => onTerminate(session.id)}
          >
            <Power className="h-3.5 w-3.5" />
            {t("activeConnections.terminate")}
          </Button>
        </section>
      )}
    </section>
  );
};

export default SessionDetailPanel;
