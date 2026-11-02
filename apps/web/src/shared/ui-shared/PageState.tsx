import type { ReactNode } from "react";
import { AlertTriangle, Inbox, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import EmptyState from "./EmptyState";

interface PageStateProps {
  kind: "loading" | "error" | "empty";
  title?: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  children?: ReactNode;
}

/** Feedback inside the page canvas; title/navigation remain owned by PageChrome. */
export default function PageState({ kind, title, description, action, children }: PageStateProps) {
  const { t } = useTranslation();
  if (kind === "empty") return <div data-slot="page-state" role="status">
    <EmptyState icon={Inbox} title={title ?? t("noResults")} description={description} actions={action}>{children}</EmptyState>
  </div>;
  const Icon = kind === "loading" ? Loader2 : AlertTriangle;
  return <div data-slot="page-state" role={kind === "error" ? "alert" : "status"}
    aria-busy={kind === "loading" || undefined}
    className="flex min-h-40 w-full flex-col items-center justify-center gap-4 px-4 py-8 text-center">
    <Icon className={kind === "loading" ? "size-6 animate-spin text-muted-foreground" : "size-6 text-muted-foreground"} aria-hidden />
    <div className="flex min-w-0 max-w-prose flex-col gap-2">
      <p className="text-sm font-medium text-foreground">{title ?? t(kind === "loading" ? "loading" : "dataTable.errorTitle")}</p>
      {description && <p className="text-sm leading-relaxed text-muted-foreground">{description}</p>}
      {children}
    </div>
    {action && <div className="flex flex-wrap items-center justify-center gap-2">{action}</div>}
  </div>;
}
