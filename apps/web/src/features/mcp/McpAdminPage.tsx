import { Link } from "react-router";
import { useTranslation } from "react-i18next";
import {
  Activity,
  BookOpen,
  Copy,
  KeyRound,
  ScrollText,
} from "lucide-react";
import PageChrome from "@/shared/ui-shared/PageChrome";
import SurfaceCard, { SurfaceCardContent } from "@/shared/ui-shared/SurfaceCard";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { notify } from "@/shared/lib/notifications";

const START_STEPS = ["step1", "step2", "step3"] as const;

const DOCS_URL = "https://piping.space/docs/mcp";
const CREATE_TOKEN_TO = "/user/settings?create=token";

const McpAdminPage = () => {
  const { t } = useTranslation();
  const endpoint =
    typeof window === "undefined" ? "/api/v1/mcp" : `${window.location.origin}/api/v1/mcp`;
  const clientConfig = JSON.stringify(
    {
      mcpServers: {
        piping: {
          url: endpoint,
          headers: { Authorization: "Bearer <your-api-token>" },
        },
      },
    },
    null,
    2,
  );

  const copy = async (value: string) => {
    if (typeof navigator === "undefined" || !navigator.clipboard) return;
    try {
      await navigator.clipboard.writeText(value);
      notify("success", t("mcpAdmin.copied"));
    } catch {
      notify("error", t("error"));
    }
  };

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <PageChrome
        title={t("mcpAdmin.title")}
        icon={Activity}
        description={t("mcpAdmin.description")}
        badge={t("mcpAdmin.badge")}
        actions={
          <>
            <Button asChild variant="outline" size="sm" className="gap-1.5">
              <a href={DOCS_URL} target="_blank" rel="noopener noreferrer">
                <BookOpen className="h-3.5 w-3.5" />
                {t("mcpAdmin.actions.openDocs")}
              </a>
            </Button>
            <Button asChild variant="outline" size="sm" className="gap-1.5">
              <Link to="/audit-logs">
                <ScrollText className="h-3.5 w-3.5" />
                {t("mcpAdmin.actions.openAudit")}
              </Link>
            </Button>
            <Button asChild className="gap-1.5">
              <Link to={CREATE_TOKEN_TO}>
                <KeyRound className="h-3.5 w-3.5" />
                {t("mcpAdmin.actions.createToken")}
              </Link>
            </Button>
          </>
        }
      />

      {/* How to connect — primary onboarding panel */}
      <SurfaceCard>
        <SurfaceCardContent>
        <div className="grid min-w-0 grid-cols-1 gap-5 lg:grid-cols-[minmax(0,1fr)_minmax(20rem,22rem)]">
          <div className="min-w-0 space-y-3.5">
            <h2 className="text-sm font-semibold text-foreground">{t("mcpAdmin.start.title")}</h2>
            <ol className="space-y-2.5">
              {START_STEPS.map((key, index) => (
                <li key={key} className="flex gap-3">
                  <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-accent text-xs font-semibold text-primary">
                    {index + 1}
                  </span>
                  <p className="min-w-0 break-words text-sm leading-relaxed text-muted-foreground">
                    {t(`mcpAdmin.start.${key}`)}
                  </p>
                </li>
              ))}
            </ol>
          </div>

          <div className="min-w-0 rounded-lg border border-border/35 bg-card p-3">
            <div className="flex items-center justify-between gap-2">
              <span className="text-sm font-medium text-foreground">{t("mcpAdmin.endpoint.title")}</span>
              <Badge variant="secondary" className="rounded-md text-xs">
                2025-06-18
              </Badge>
            </div>
            <div className="mt-2 flex min-w-0 items-center gap-2 rounded-md border border-border/35 bg-card p-2">
              <code className="min-w-0 flex-1 truncate font-mono text-sm text-foreground">{endpoint}</code>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                title={t("copy")}
                aria-label={t("copy")}
                onClick={() => copy(endpoint)}
              >
                <Copy className="h-3.5 w-3.5" />
              </Button>
            </div>
            <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
              {t("mcpAdmin.endpoint.description")}
            </p>

            <div className="mt-3 border-t border-border/30 pt-3">
              <div className="flex items-center justify-between gap-2">
                <span className="text-sm font-medium text-foreground">
                  {t("mcpAdmin.config.title")}
                </span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  title={t("copy")}
                  aria-label={t("copy")}
                  onClick={() => copy(clientConfig)}
                >
                  <Copy className="h-3.5 w-3.5" />
                </Button>
              </div>
              <pre className="mt-2 max-h-56 overflow-auto rounded-md border border-border/35 bg-card p-2.5 font-mono text-xs leading-relaxed text-muted-foreground">
                {clientConfig}
              </pre>
              <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
                {t("mcpAdmin.config.note")}
              </p>
            </div>
          </div>
        </div>
        </SurfaceCardContent>
      </SurfaceCard>
    </div>
  );
};

export default McpAdminPage;
