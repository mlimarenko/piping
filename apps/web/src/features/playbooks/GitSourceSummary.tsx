import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { Button } from "@/shared/ui-primitives/button";
import { getProjectRevision, type Project, type RevisionSelection } from "./api";
import { formatDateTime } from "./playbookPageFormat";
import { projectRevisionQueryKey } from "./projectFilesModel";
import { projectSectionHref } from "./projectRoutes";

export function GitSourceSummary({ project, selection, disabled = false }: Readonly<{
  project: Project;
  selection: RevisionSelection;
  disabled?: boolean;
}>) {
  const { t } = useTranslation();
  const revisionId = selection.kind === "fixed" ? selection.revision_id : project.current_revision_id ?? "";
  const currentId = project.current_revision_id ?? "";
  const selected = useQuery({
    queryKey: projectRevisionQueryKey(project.id, revisionId),
    queryFn: () => getProjectRevision(project.id, revisionId),
    enabled: Boolean(revisionId),
    staleTime: Infinity,
    retry: false,
  });
  const current = useQuery({
    queryKey: projectRevisionQueryKey(project.id, currentId),
    queryFn: () => getProjectRevision(project.id, currentId),
    enabled: Boolean(currentId),
    staleTime: Infinity,
    retry: false,
  });
  const revision = selected.data?.project_id === project.id && selected.data.revision_id === revisionId ? selected.data : undefined;
  const latest = current.data?.project_id === project.id && current.data.revision_id === currentId ? current.data : undefined;
  const selectedUnavailable = Boolean(revisionId) && !selected.isLoading && !revision?.source_commit_oid;

  return <section aria-label={t("playbooks.gitSource.title")} className="flex min-w-0 flex-col gap-2 text-sm">
    <dl className="grid min-w-0 gap-2 sm:grid-cols-2">
      <div className="min-w-0"><dt className="text-muted-foreground">{t("playbooks.gitSource.mode")}</dt><dd>{t(selection.kind === "fixed" ? "playbooks.gitSource.pin" : "playbooks.gitSource.follow")}</dd></div>
      {project.source.kind === "git" && <div className="min-w-0"><dt className="text-muted-foreground">{t("playbooks.gitSource.reference")}</dt><dd className="break-all font-mono">{project.source.reference}</dd></div>}
      <div className="min-w-0 sm:col-span-2"><dt className="text-muted-foreground">{t("playbooks.gitSource.commit")}</dt><dd className="break-all font-mono">{revision?.source_commit_oid ?? t(selected.isLoading ? "playbooks.gitSource.loading" : "playbooks.gitSource.unavailable")}</dd></div>
      {revisionId && <div className="min-w-0 sm:col-span-2"><dt className="text-muted-foreground">{t("playbooks.launchDraft.revision")}</dt><dd className="break-all font-mono">{revisionId}</dd></div>}
      <div className="min-w-0 sm:col-span-2"><dt className="text-muted-foreground">{t("playbooks.gitSource.lastSync")}</dt><dd>{latest?.source_commit_oid ? <time dateTime={latest.created_at}>{formatDateTime(latest.created_at)}</time> : t(current.isLoading ? "playbooks.gitSource.loading" : currentId ? "playbooks.gitSource.unavailable" : "playbooks.gitSource.notSynced")}</dd></div>
    </dl>
    <p className="text-muted-foreground">{t(selection.kind === "fixed" ? "playbooks.gitSource.pinHint" : "playbooks.gitSource.followHint")}</p>
    {selectedUnavailable && <p role="alert" className="text-status-warning">{t("playbooks.gitSource.selectedUnavailable")}</p>}
    {!currentId && <p role="status" className="text-status-warning">{t("playbooks.gitSource.syncRequired")}</p>}
    {!disabled && <div className="flex flex-wrap gap-2">
      <Button asChild variant="outline" size="sm"><Link to={projectSectionHref(project.id, "source")}>{t("playbooks.gitSource.openProject")}</Link></Button>
      {(selected.isError || current.isError) && <Button type="button" variant="outline" size="sm" onClick={() => { if (selected.isError) void selected.refetch(); if (current.isError && currentId !== revisionId) void current.refetch(); }}>{t("retry")}</Button>}
    </div>}
  </section>;
}
