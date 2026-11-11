import { useRef, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useApiMutation } from "@/shared/api/hooks";
import { ApiError } from "@/shared/api/client";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import FormField from "@/shared/ui-shared/FormField";
import FormSection from "@/shared/ui-shared/FormSection";
import PageState from "@/shared/ui-shared/PageState";
import DataflowCatalogMetadataFields from "./DataflowCatalogMetadataFields";
import type { EntityCustomFieldsHandle } from "./components/custom-fields/EntityCustomFieldsSection";
import { customFieldDataRecord } from "./components/custom-fields/entityCustomFieldDraft";
import { DATAFLOW_CATALOG_ROUTES } from "./DataflowCatalogNav";
import { inventoryErrorMessage } from "./inventoryErrors";
import { inventoryResourceOptionsQuery } from "./resourceOptions";
import { createDataflowGroup, dataflowGroupDescendants, getDataflowGroup, listApplications, listDataflowGroups, updateDataflowGroup } from "./dataflowCatalog-api";
import type { DataflowGroup } from "./dataflowCatalog-api";
import type { DataflowStatus } from "./dataflows-api";

type GroupDraft = {
  parent_id: string | null;
  application_id: string | null;
  name: string;
  status: DataflowStatus;
  description: string;
  comments: string;
  owner_id: string | null;
  tenant_id: string | null;
  tags: string[];
  custom_field_data: Record<string, unknown>;
};

const EMPTY: GroupDraft = {
  parent_id: null, application_id: null, name: "", status: "active", description: "", comments: "",
  owner_id: null, tenant_id: null, tags: [], custom_field_data: {},
};

function groupDraft(group: DataflowGroup): GroupDraft {
  return {
    parent_id: group.parent_id ?? null,
    application_id: group.application_id ?? null,
    name: group.name ?? "",
    status: group.status ?? "active",
    description: group.description ?? "",
    comments: group.comments ?? "",
    owner_id: group.owner_id ?? null,
    tenant_id: group.tenant_id ?? null,
    tags: group.tags ?? [],
    custom_field_data: customFieldDataRecord(group.custom_field_data),
  };
}

/** A group may not become its own descendant's child. */

function GroupForm({ group }: Readonly<{ group?: DataflowGroup }>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const allowed = can(group ? "UPDATE" : "CREATE", "INVENTORY");
  const initial = group ? groupDraft(group) : EMPTY;
  const [draft, setDraft] = useState<GroupDraft>(initial);
  const [errors, setErrors] = useState<{ name?: string; parent?: string; custom?: string }>({});
  const customFields = useRef<EntityCustomFieldsHandle | null>(null);
  const groups = useQuery({ queryKey: ["inventory", "catalog", "dataflow-groups"], queryFn: () => listDataflowGroups() });
  const applications = useQuery({ queryKey: ["inventory", "catalog", "applications"], queryFn: () => listApplications() });
  const tags = useQuery(inventoryResourceOptionsQuery("Tags"));
  const descendants = group ? dataflowGroupDescendants(groups.data ?? [], group.id) : new Set<string>();
  const parentOptions = (groups.data ?? [])
    .filter(candidate => candidate.id !== group?.id && !descendants.has(candidate.id))
    .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id));
  const applicationName = (id?: string | null) => id ? applications.data?.find(app => app.id === id)?.name ?? null : null;
  const siblingName = (id?: string | null) => id ? groups.data?.find(row => row.id === id)?.name ?? t("dataflows.unresolved") : null;
  const tagNames = (ids?: string[] | null) => (ids ?? [])
    .map(id => tags.data?.items.find(tag => tag.id === id)?.name)
    .filter((name): name is string => !!name);

  const mutation = useApiMutation({
    mutationFn: (body: GroupDraft) => group
      ? updateDataflowGroup(group.id, { parent_id: body.parent_id, application_id: body.application_id, name: body.name, status: body.status, description: body.description, comments: body.comments, owner_id: body.owner_id, tenant_id: body.tenant_id, tags: body.tags, custom_field_data: body.custom_field_data })
      : createDataflowGroup({ parent_id: body.parent_id, application_id: body.application_id, name: body.name, status: body.status, description: body.description, comments: body.comments, owner_id: body.owner_id, tenant_id: body.tenant_id, tags: body.tags, custom_field_data: body.custom_field_data }),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["inventory", "catalog", "dataflow-groups"] });
      await qc.invalidateQueries({ queryKey: ["inventory", "dataflows"] });
      await navigate(DATAFLOW_CATALOG_ROUTES.groups);
    },
  });
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial) && !mutation.isSuccess;
  const save = () => {
    if (!allowed || mutation.isPending) return;
    const next: { name?: string; parent?: string; custom?: string } = {};
    if (!draft.name.trim()) next.name = t("required");
    const parent = draft.parent_id ? groups.data?.find(candidate => candidate.id === draft.parent_id) : undefined;
    if (parent && descendants.has(parent.id)) next.parent = t("dataflows.cycleError");
    else if (parent && (parent.application_id ?? null) !== (draft.application_id ?? null)) next.parent = t("dataflows.applicationMismatch");
    let custom_field_data = draft.custom_field_data;
    try {
      custom_field_data = customFields.current?.buildPayload() ?? draft.custom_field_data;
    } catch {
      next.custom = t("customFields.entity.definitionsUnavailable");
    }
    setErrors(next);
    if (Object.keys(next).length) return;
    mutation.mutate({ ...draft, name: draft.name.trim(), custom_field_data });
  };
  const back = () => void navigate(DATAFLOW_CATALOG_ROUTES.groups);
  const writeError = mutation.isError
    ? (mutation.error instanceof ApiError && mutation.error.status === 409 ? t("dataflows.groupAppConflict") : inventoryErrorMessage(t, mutation.error))
    : null;
  return <EntityEditorPage scope="routed" backLabel={t("dataflows.flowGroupsTitle")}
    title={group ? t("dataflows.editGroup", { name: group.name }) : t("dataflows.newGroup")}
    description={t("dataflows.groupHint")} onBack={back} dirty={dirty} dirtyMessage={t("editor.unsavedChanges")}
    onSave={allowed ? save : undefined} saveLabel={t(group ? "saveChanges" : "create")} cancelLabel={t("cancel")}
    saving={mutation.isPending} error={writeError ? <p role="alert" className="text-sm text-destructive">{writeError}</p> : undefined}>
    <form className="flex min-w-0 flex-col gap-4" noValidate onSubmit={event => { event.preventDefault(); save(); }}>
      <FormSection contentClassName="grid min-w-0 gap-4 @lg:grid-cols-2">
        <FormField label={t("dataflows.groupName")} htmlFor="group-name" required error={errors.name}>
          <Input id="group-name" value={draft.name} onChange={event => setDraft({ ...draft, name: event.target.value })} disabled={!allowed || mutation.isPending} />
        </FormField>
        <FormField label={t("dataflows.groupApplication")} htmlFor="group-application">
          <select id="group-application" aria-label={t("dataflows.groupApplication")} value={draft.application_id ?? ""} disabled={!allowed || mutation.isPending || applications.isPending}
            onChange={event => { setErrors(current => ({ ...current, parent: undefined })); setDraft({ ...draft, application_id: event.target.value || null }); }}
            className="control-surface h-9 rounded-md px-2 text-sm">
            <option value="">{t("dataflows.noApplication")}</option>
            {[...(applications.data ?? [])].sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id))
              .map(app => <option key={app.id} value={app.id}>{app.name}</option>)}
          </select>
        </FormField>
        <FormField label={t("dataflows.groupParent")} htmlFor="group-parent" error={errors.parent} className="@lg:col-span-2">
          <select id="group-parent" aria-label={t("dataflows.groupParent")} value={draft.parent_id ?? ""} disabled={!allowed || mutation.isPending || groups.isPending}
            onChange={event => { setErrors(current => ({ ...current, parent: undefined })); setDraft({ ...draft, parent_id: event.target.value || null }); }}
            className="control-surface h-9 rounded-md px-2 text-sm">
            <option value="">{t("dataflows.noGroup")}</option>
            {parentOptions.map(candidate => {
              const application = applicationName(candidate.application_id);
              return <option key={candidate.id} value={candidate.id}>{application ? `${candidate.name} · ${application}` : candidate.name}</option>;
            })}
          </select>
        </FormField>
        <FormField label={t("dataflows.groupStatus")} htmlFor="group-status">
          <select id="group-status" aria-label={t("dataflows.groupStatus")} value={draft.status} disabled={!allowed || mutation.isPending}
            onChange={event => setDraft({ ...draft, status: event.target.value as DataflowStatus })}
            className="control-surface h-9 rounded-md px-2 text-sm">
            {(["active", "draft", "disabled"] as const).map(status => <option key={status} value={status}>{t(`dataflows.statuses.${status}`)}</option>)}
          </select>
        </FormField>
        <FormField label={t("description")} htmlFor="group-description">
          <Textarea id="group-description" value={draft.description} rows={2} onChange={event => setDraft({ ...draft, description: event.target.value })} disabled={!allowed || mutation.isPending} />
        </FormField>
        <FormField label={t("inventory.resources.fields.comments")} htmlFor="group-comments" className="@lg:col-span-2">
          <Textarea id="group-comments" value={draft.comments} rows={2} onChange={event => setDraft({ ...draft, comments: event.target.value })} disabled={!allowed || mutation.isPending} />
        </FormField>
      </FormSection>
      <FormSection title={t("dataflows.metadataSection")}>
        <DataflowCatalogMetadataFields draft={{ owner_id: draft.owner_id, tenant_id: draft.tenant_id, tags: draft.tags }}
          onChange={metadata => setDraft({ ...draft, owner_id: metadata.owner_id ?? null, tenant_id: metadata.tenant_id ?? null, tags: metadata.tags ?? [] })}
          disabled={!allowed || mutation.isPending}
          entityKind="inventory.dataflowgroup" entityId={group?.id}
          creationScopeToken={group ? undefined : "inventory.dataflowgroup"}
          customFields={draft.custom_field_data}
          onRegisterCustomFields={handle => { customFields.current = handle; }} />
        {errors.custom && <p role="alert" className="text-sm text-destructive">{errors.custom}</p>}
      </FormSection>
      {group && <FormSection title={t("dataflows.effectiveStatus")} description={t("dataflows.groupHint")}>
        <dl role="group" aria-label={t("dataflows.effectiveStatus")} className="grid min-w-0 gap-3 text-sm @lg:grid-cols-2">
          <div className="flex flex-col gap-1"><dt className="text-xs text-muted-foreground">{t("dataflows.effectiveStatus")}</dt>
            <dd className="font-medium">{t(`dataflows.statuses.${group.effective_status}`)}</dd></div>
          <div className="flex flex-col gap-1"><dt className="text-xs text-muted-foreground">{t("dataflows.blockingAncestor", { defaultValue: "Blocked by" })}</dt>
            <dd className="font-medium">{group.blocked_by_group_id ? siblingName(group.blocked_by_group_id) : "—"}</dd></div>
          <div className="flex min-w-0 flex-col gap-1 @lg:col-span-2"><dt className="text-xs text-muted-foreground">{t("dataflows.inheritedTags")}</dt>
            <dd className="flex flex-wrap gap-1">{tagNames(group.inherited_tags).length
              ? tagNames(group.inherited_tags).map(name => <span key={name} className="rounded border border-border px-1.5 py-0.5 text-xs">{name}</span>)
              : <span className="text-muted-foreground">{t("dataflows.noTags")}</span>}</dd></div>
        </dl>
      </FormSection>}
    </form>
  </EntityEditorPage>;
}

export default function DataflowGroupEditorPage() {
  const { t } = useTranslation();
  const { id } = useParams<{ id?: string }>();
  const navigate = useNavigate();
  const group = useQuery({
    queryKey: ["inventory", "catalog", "dataflow-groups", id],
    queryFn: () => getDataflowGroup(id as string),
    enabled: !!id,
  });
  const back = () => void navigate(DATAFLOW_CATALOG_ROUTES.groups);
  if (id && (group.isPending || group.isError || !group.data)) {
    return <EntityEditorPage scope="routed" title={t("dataflows.flowGroupsTitle")} onBack={back}
      backLabel={t("dataflows.flowGroupsTitle")} cancelLabel={t("cancel")}>
      {group.isPending ? <PageState kind="loading" title={t("loading")} />
        : <PageState kind="error" description={t("dataTable.errorDescription")}
          action={<Button variant="outline" onClick={() => void group.refetch()}>{t("retry")}</Button>} />}
    </EntityEditorPage>;
  }
  return <GroupForm key={id ?? "new"} group={group.data} />;
}
