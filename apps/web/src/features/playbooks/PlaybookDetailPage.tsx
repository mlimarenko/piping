import type { ReactNode } from "react";
import { useParams, useSearchParams } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { FileCode2 } from "lucide-react";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import PageState from "@/shared/ui-shared/PageState";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import { Button } from "@/shared/ui-primitives/button";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { isUuid } from "@/shared/lib/ids";
import { ApiError } from "@/shared/api/client";
import { getPlaybook, getPlaybookRunGroup, getTargetSet, previewPlaybookTargetSet } from "./api";
import { PLAYBOOK_CHILD_SECTIONS, playbookChildSection, playbookDetailHref, playbookReturnHref } from "./playbookDetailRoutes";
import { playbookEditorEditHref } from "./playbookEditorRoutes";
import PlaybooksPage from "./PlaybooksPage";
import VersionHistory from "./VersionHistory";
import { TargetPreview } from "./PlaybookPanelUi";

export default function PlaybookDetailPage() {
  const { id = "" } = useParams();
  const [params] = useSearchParams();
  const { can } = usePermissions();
  const { t } = useTranslation();
  const section = playbookChildSection(params.get("section"));
  const readable = can("READ", "PLAYBOOK");
  const query = useQuery({
    queryKey: ["playbooks", "detail", id],
    queryFn: () => getPlaybook(id),
    enabled: isUuid(id) && readable,
  });
  const item = query.data;
  const targetId = item?.playbook.default_target_set_id;
  const canReadTargets = can("READ", "PLAYBOOK_TARGET_SET");
  const groupId = params.get("group");
  const groupQuery = useQuery({
    queryKey: ["playbooks", "run-group", groupId],
    queryFn: () => getPlaybookRunGroup(groupId!, 50_000),
    enabled: section === "runs" && Boolean(item) && isUuid(groupId) && readable && can("READ", "PLAYBOOK_RUN_GROUP"),
  });
  const targetQuery = useQuery({
    queryKey: ["playbooks", "target-set", targetId],
    queryFn: () => getTargetSet(targetId!),
    enabled: section === "targets" && Boolean(targetId) && canReadTargets && readable,
  });
  const previewQuery = useQuery({
    queryKey: ["playbooks", "target-set-preview", targetId],
    queryFn: () => previewPlaybookTargetSet(targetId!, 50),
    enabled: section === "targets" && Boolean(targetId) && canReadTargets && readable,
  });
  const labels = {
    versions: t("playbooks.page.actions.versionHistory"),
    assignments: t("navItems.assignments"),
    schedules: t("navItems.schedules"),
    runs: t("navItems.runs"),
    targets: t("playbooks.page.fields.defaultTargetSet"),
  };
  const canReadSection = section === "versions" ||
    (section === "targets" ? canReadTargets :
      section === "assignments" ? can("READ", "PLAYBOOK_ASSIGNMENT") :
      section === "schedules" ? can("READ", "PLAYBOOK_SCHEDULE") : can("READ", "PLAYBOOK_RUN_GROUP"));
  const renderState = (content: ReactNode) => <PageState title={t("playbooks.page.title")} icon={FileCode2} backHref={playbookReturnHref(params.get("returnTo"))}>{content}</PageState>;
  if (!readable) return renderState(<p role="alert">{t("accessDenied.title")}</p>);
  if (!isUuid(id)) return renderState(<p role="alert">{t("notFound.title")}</p>);
  if (query.isPending) return renderState(<p role="status">{t("loading")}</p>);
  if (query.isError || !item) {
    const status = query.error instanceof ApiError ? query.error.status : undefined;
    const message = status === 403 ? "accessDenied.title" : status === 404 || !query.isError ? "notFound.title" : "unknownError";
    return renderState(<div role="alert" className="flex flex-col items-start gap-4"><p>{t(message)}</p>{query.isError && status !== 403 && <Button onClick={() => void query.refetch()}>{t("retry")}</Button>}</div>);
  }
  return (
    <div className="flex min-w-0 flex-col gap-4">
      <EntityDetail
        icon={FileCode2}
        title={item.playbook.name}
        description={item.playbook.description ?? undefined}
        backHref={playbookReturnHref(params.get("returnTo"))}
        fields={[
          { label: "ID", value: item.playbook.id },
          { label: t("playbooks.page.fields.runtime"), value: item.playbook.runtime },
          { label: t("playbooks.page.fields.category"), value: item.playbook.category },
          { label: t("playbooks.page.fields.version"), value: item.current_version?.version },
          { label: t("status"), value: item.playbook.status },
        ]}
        actions={[{ key: "edit", label: t(item.playbook.source_kind === "in_app" ? "edit" : "playbooks.page.actions.viewScript"), allowed: item.playbook.source_kind !== "in_app" || can("UPDATE", "PLAYBOOK"), href: `${playbookEditorEditHref(id)}?${new URLSearchParams({ returnTo: playbookDetailHref(id, section, params.toString()) })}` }]}
      />
      <SectionLinks label={item.playbook.name} activeValue={section} items={PLAYBOOK_CHILD_SECTIONS.map((value) => ({ value, label: labels[value], to: playbookDetailHref(id, value, params.toString()) }))} />
      {!canReadSection ? <p role="alert">{t("accessDenied.title")}</p> : section === "versions" ? (
        <VersionHistory key={id} playbookId={id} currentVersionId={item.current_version?.id} readOnly onRestore={() => { /* Restoring is owned by the gated editor. */ }} restoring={false} />
      ) : section === "targets" ? (
        <section className="flex flex-col gap-4" aria-label={labels.targets}>
          {/* Only this persisted relation is available without inventing a playbook filter on target sets. */}
          <h2 className="text-sm font-medium">{labels.targets}: {targetQuery.data?.name ?? "—"}</h2>
          {!targetId ? <p>{t("playbooks.page.empty.targetSets")}</p> : targetQuery.isError || previewQuery.isError ? <p role="alert">{t("unknownError")}<Button onClick={() => { void targetQuery.refetch(); void previewQuery.refetch(); }}>{t("retry")}</Button></p> : previewQuery.data ? <TargetPreview preview={previewQuery.data} /> : <p role="status">{t("loading")}</p>}
        </section>
      ) : section === "runs" && groupId && (!isUuid(groupId) || groupQuery.isError || (groupQuery.data && groupQuery.data.run_group.playbook_id !== id)) ? <p role="alert">{t("notFound.title")}</p>
        : section === "runs" && groupId && groupQuery.isPending ? <p role="status">{t("loading")}</p>
        : <PlaybooksPage key={id} parent={item} initialTab={section} />}
    </div>
  );
}
