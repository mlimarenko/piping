import { useRef, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useApiMutation } from "@/shared/api/hooks";
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
import { createApplication, getApplication, listApplicationRoles, updateApplication } from "./dataflowCatalog-api";
import type { Application } from "./dataflowCatalog-api";

type ApplicationDraft = {
  name: string;
  role_id: string | null;
  description: string;
  comments: string;
  owner_id: string | null;
  tenant_id: string | null;
  tags: string[];
  custom_field_data: Record<string, unknown>;
};

const EMPTY: ApplicationDraft = { name: "", role_id: null, description: "", comments: "", owner_id: null, tenant_id: null, tags: [], custom_field_data: {} };

function applicationDraft(app: Application): ApplicationDraft {
  return {
    name: app.name ?? "",
    role_id: app.role_id ?? null,
    description: app.description ?? "",
    comments: app.comments ?? "",
    owner_id: app.owner_id ?? null,
    tenant_id: app.tenant_id ?? null,
    tags: app.tags ?? [],
    custom_field_data: customFieldDataRecord(app.custom_field_data),
  };
}

function ApplicationForm({ application }: Readonly<{ application?: Application }>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const allowed = can(application ? "UPDATE" : "CREATE", "INVENTORY");
  const initial = application ? applicationDraft(application) : EMPTY;
  const [draft, setDraft] = useState<ApplicationDraft>(initial);
  const [nameError, setNameError] = useState<string | null>(null);
  const [customError, setCustomError] = useState<string | null>(null);
  const customFields = useRef<EntityCustomFieldsHandle | null>(null);
  const roles = useQuery({ queryKey: ["inventory", "catalog", "application-roles"], queryFn: () => listApplicationRoles() });
  const mutation = useApiMutation({
    mutationFn: (body: ApplicationDraft) => application
      ? updateApplication(application.id, { name: body.name, role_id: body.role_id, description: body.description, comments: body.comments, owner_id: body.owner_id, tenant_id: body.tenant_id, tags: body.tags, custom_field_data: body.custom_field_data })
      : createApplication({ name: body.name, role_id: body.role_id, description: body.description, comments: body.comments, owner_id: body.owner_id, tenant_id: body.tenant_id, tags: body.tags, custom_field_data: body.custom_field_data }),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["inventory", "catalog", "applications"] });
      await navigate(DATAFLOW_CATALOG_ROUTES.applications);
    },
  });
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial) && !mutation.isSuccess;
  const save = () => {
    if (!allowed || mutation.isPending) return;
    if (!draft.name.trim()) { setNameError(t("required")); return; }
    setNameError(null);
    let custom_field_data = draft.custom_field_data;
    try {
      custom_field_data = customFields.current?.buildPayload() ?? draft.custom_field_data;
      setCustomError(null);
    } catch {
      setCustomError(t("customFields.entity.definitionsUnavailable"));
      return;
    }
    mutation.mutate({ ...draft, name: draft.name.trim(), custom_field_data });
  };
  const back = () => void navigate(DATAFLOW_CATALOG_ROUTES.applications);
  return <EntityEditorPage scope="routed" backLabel={t("dataflows.applicationsTitle")}
    title={application ? t("dataflows.editApplication", { name: application.name }) : t("dataflows.newApplication")}
    description={t("dataflows.applicationHint")} onBack={back} dirty={dirty} dirtyMessage={t("editor.unsavedChanges")}
    onSave={allowed ? save : undefined} saveLabel={t(application ? "saveChanges" : "create")} cancelLabel={t("cancel")}
    saving={mutation.isPending}
    error={mutation.isError ? <p role="alert" className="text-sm text-destructive">{inventoryErrorMessage(t, mutation.error)}</p> : undefined}>
    <form className="flex min-w-0 flex-col gap-4" noValidate onSubmit={event => { event.preventDefault(); save(); }}>
      <FormSection contentClassName="grid min-w-0 gap-4 @lg:grid-cols-2">
        <FormField label={t("dataflows.applicationName")} htmlFor="application-name" required error={nameError}>
          <Input id="application-name" value={draft.name} onChange={event => setDraft({ ...draft, name: event.target.value })} disabled={!allowed || mutation.isPending} />
        </FormField>
        <FormField label={t("dataflows.applicationRole")} htmlFor="application-role">
          <select id="application-role" aria-label={t("dataflows.applicationRole")} value={draft.role_id ?? ""} disabled={!allowed || mutation.isPending || roles.isPending}
            onChange={event => setDraft({ ...draft, role_id: event.target.value || null })}
            className="control-surface h-9 rounded-md px-2 text-sm">
            <option value="">{t("dataflows.noRole")}</option>
            {[...(roles.data ?? [])].sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id))
              .map(role => <option key={role.id} value={role.id}>{role.name}</option>)}
          </select>
          {roles.isError && <p role="alert" className="text-xs text-destructive">{t("dataTable.errorDescription")}</p>}
        </FormField>
        <FormField label={t("description")} htmlFor="application-description" className="@lg:col-span-2">
          <Textarea id="application-description" value={draft.description} rows={2} onChange={event => setDraft({ ...draft, description: event.target.value })} disabled={!allowed || mutation.isPending} />
        </FormField>
        <FormField label={t("inventory.resources.fields.comments")} htmlFor="application-comments" className="@lg:col-span-2">
          <Textarea id="application-comments" value={draft.comments} rows={2} onChange={event => setDraft({ ...draft, comments: event.target.value })} disabled={!allowed || mutation.isPending} />
        </FormField>
      </FormSection>
      <FormSection title={t("dataflows.metadataSection")}>
        <DataflowCatalogMetadataFields draft={{ owner_id: draft.owner_id, tenant_id: draft.tenant_id, tags: draft.tags }}
          onChange={metadata => setDraft({ ...draft, owner_id: metadata.owner_id ?? null, tenant_id: metadata.tenant_id ?? null, tags: metadata.tags ?? [] })}
          disabled={!allowed || mutation.isPending}
          entityKind="inventory.application" entityId={application?.id}
          creationScopeToken={application ? undefined : "inventory.application"}
          customFields={draft.custom_field_data}
          onRegisterCustomFields={handle => { customFields.current = handle; }} />
        {customError && <p role="alert" className="text-sm text-destructive">{customError}</p>}
      </FormSection>
    </form>
  </EntityEditorPage>;
}

export default function ApplicationEditorPage() {
  const { t } = useTranslation();
  const { id } = useParams<{ id?: string }>();
  const navigate = useNavigate();
  const application = useQuery({
    queryKey: ["inventory", "catalog", "applications", id],
    queryFn: () => getApplication(id as string),
    enabled: !!id,
  });
  const back = () => void navigate(DATAFLOW_CATALOG_ROUTES.applications);
  if (id && (application.isPending || application.isError || !application.data)) {
    return <EntityEditorPage scope="routed" title={t("dataflows.applicationsTitle")} onBack={back}
      backLabel={t("dataflows.applicationsTitle")} cancelLabel={t("cancel")}>
      {application.isPending ? <PageState kind="loading" title={t("loading")} />
        : <PageState kind="error" description={t("dataTable.errorDescription")}
          action={<Button variant="outline" onClick={() => void application.refetch()}>{t("retry")}</Button>} />}
    </EntityEditorPage>;
  }
  return <ApplicationForm key={id ?? "new"} application={application.data} />;
}
