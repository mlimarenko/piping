import { useRef, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
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
import DataflowEndpointPicker from "./DataflowEndpointPicker";
import { DATAFLOW_CATALOG_ROUTES } from "./DataflowCatalogNav";
import { DATAFLOW_ENDPOINT_TYPES, dataflowEndpointError, dataflowEndpointInput } from "./dataflowEndpoints";
import { inventoryErrorMessage } from "./inventoryErrors";
import { createAddressGroup, getAddressGroup, updateAddressGroup } from "./dataflowCatalog-api";
import type { AddressGroup } from "./dataflowCatalog-api";
import type { DataflowEndpoint } from "./dataflows-api";

const ADDRESS_GROUP_MEMBER_LIMIT = 64;
const ADDRESS_GROUP_MEMBER_TYPES = DATAFLOW_ENDPOINT_TYPES.filter(type => type !== "inventory.addressgroup");

type AddressGroupDraft = {
  name: string;
  description: string;
  comments: string;
  owner_id: string | null;
  tenant_id: string | null;
  tags: string[];
  members: DataflowEndpoint[];
  custom_field_data: Record<string, unknown>;
};

const EMPTY_MEMBER: DataflowEndpoint = { object_type: "dcim.device", object_id: null, address_mode: "default", ipaddresses: [] };

const EMPTY: AddressGroupDraft = {
  name: "", description: "", comments: "", owner_id: null, tenant_id: null, tags: [], members: [EMPTY_MEMBER], custom_field_data: {},
};

function addressGroupDraft(group: AddressGroup): AddressGroupDraft {
  return {
    name: group.name ?? "",
    description: group.description ?? "",
    comments: group.comments ?? "",
    owner_id: group.owner_id ?? null,
    tenant_id: group.tenant_id ?? null,
    tags: group.tags ?? [],
    members: (group.members ?? []).map(dataflowEndpointInput),
    custom_field_data: customFieldDataRecord(group.custom_field_data),
  };
}

function AddressGroupForm({ group }: Readonly<{ group?: AddressGroup }>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const allowed = can(group ? "UPDATE" : "CREATE", "INVENTORY");
  const initial = group ? addressGroupDraft(group) : EMPTY;
  const [draft, setDraft] = useState<AddressGroupDraft>(initial);
  const [nameError, setNameError] = useState<string | null>(null);
  const [memberErrors, setMemberErrors] = useState<Array<string | undefined>>([]);
  const [customError, setCustomError] = useState<string | null>(null);
  const customFields = useRef<EntityCustomFieldsHandle | null>(null);

  const memberMessage = (member: DataflowEndpoint): string | undefined => {
    if (member.object_type === "inventory.addressgroup") {
      return t("dataflows.addressGroupNestedError", { defaultValue: "An address group cannot contain another address group." });
    }
    const key = dataflowEndpointError(member);
    return key ? t(key) : undefined;
  };
  const setMember = (index: number, member: DataflowEndpoint) => {
    setMemberErrors(current => current.map((message, position) => position === index ? undefined : message));
    setDraft({ ...draft, members: draft.members.map((current, position) => position === index ? member : current) });
  };
  const addMember = () => {
    if (draft.members.length >= ADDRESS_GROUP_MEMBER_LIMIT) return;
    setDraft({ ...draft, members: [...draft.members, EMPTY_MEMBER] });
  };
  const removeMember = (index: number) => {
    if (draft.members.length <= 1) return;
    setMemberErrors(current => current.filter((_, position) => position !== index));
    setDraft({ ...draft, members: draft.members.filter((_, position) => position !== index) });
  };
  const mutation = useApiMutation({
    mutationFn: (body: AddressGroupDraft) => group
      ? updateAddressGroup(group.id, { name: body.name, description: body.description, comments: body.comments, owner_id: body.owner_id, tenant_id: body.tenant_id, tags: body.tags, members: body.members, custom_field_data: body.custom_field_data })
      : createAddressGroup({ name: body.name, description: body.description, comments: body.comments, owner_id: body.owner_id, tenant_id: body.tenant_id, tags: body.tags, members: body.members, custom_field_data: body.custom_field_data }),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["inventory", "catalog", "address-groups"] });
      await navigate(DATAFLOW_CATALOG_ROUTES.addressGroups);
    },
  });
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial) && !mutation.isSuccess;
  const save = () => {
    if (!allowed || mutation.isPending) return;
    let invalidName: string | null = null;
    if (!draft.name.trim()) invalidName = t("required");
    setNameError(invalidName);
    const errors = draft.members.map(memberMessage);
    const countInvalid = draft.members.length < 1 || draft.members.length > ADDRESS_GROUP_MEMBER_LIMIT;
    setMemberErrors(errors);
    if (invalidName || countInvalid || errors.some(Boolean)) return;
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
  const back = () => void navigate(DATAFLOW_CATALOG_ROUTES.addressGroups);
  return <EntityEditorPage scope="routed" backLabel={t("dataflows.addressGroupsTitle")}
    title={group ? t("dataflows.editAddressGroup", { name: group.name }) : t("dataflows.newAddressGroup")}
    description={t("dataflows.addressGroupHint")} onBack={back} dirty={dirty} dirtyMessage={t("editor.unsavedChanges")}
    onSave={allowed ? save : undefined} saveLabel={t(group ? "saveChanges" : "create")} cancelLabel={t("cancel")}
    saving={mutation.isPending}
    error={mutation.isError ? <p role="alert" className="text-sm text-destructive">{inventoryErrorMessage(t, mutation.error)}</p> : undefined}>
    <form className="flex min-w-0 flex-col gap-4" noValidate onSubmit={event => { event.preventDefault(); save(); }}>
      <FormSection contentClassName="grid min-w-0 gap-4 @lg:grid-cols-2">
        <FormField label={t("name")} htmlFor="address-group-name" required error={nameError}>
          <Input id="address-group-name" value={draft.name} onChange={event => setDraft({ ...draft, name: event.target.value })} disabled={!allowed || mutation.isPending} />
        </FormField>
        <FormField label={t("description")} htmlFor="address-group-description">
          <Textarea id="address-group-description" value={draft.description} rows={2} onChange={event => setDraft({ ...draft, description: event.target.value })} disabled={!allowed || mutation.isPending} />
        </FormField>
        <FormField label={t("inventory.resources.fields.comments")} htmlFor="address-group-comments" className="@lg:col-span-2">
          <Textarea id="address-group-comments" value={draft.comments} rows={2} onChange={event => setDraft({ ...draft, comments: event.target.value })} disabled={!allowed || mutation.isPending} />
        </FormField>
      </FormSection>
      <FormSection title={t("dataflows.metadataSection")}>
        <DataflowCatalogMetadataFields draft={{ owner_id: draft.owner_id, tenant_id: draft.tenant_id, tags: draft.tags }}
          onChange={metadata => setDraft({ ...draft, owner_id: metadata.owner_id ?? null, tenant_id: metadata.tenant_id ?? null, tags: metadata.tags ?? [] })}
          disabled={!allowed || mutation.isPending}
          entityKind="inventory.addressgroup" entityId={group?.id}
          creationScopeToken={group ? undefined : "inventory.addressgroup"}
          customFields={draft.custom_field_data}
          onRegisterCustomFields={handle => { customFields.current = handle; }} />
        {customError && <p role="alert" className="text-sm text-destructive">{customError}</p>}
      </FormSection>
      <FormSection title={t("dataflows.addressGroupMembers")} description={t("dataflows.memberLimit")}
        action={<Button type="button" variant="outline" size="sm" onClick={addMember} disabled={!allowed || mutation.isPending || draft.members.length >= ADDRESS_GROUP_MEMBER_LIMIT}>
          <Plus aria-hidden="true" className="size-4" />{t("dataflows.addMember")}
        </Button>}>
        <ul className="flex min-w-0 flex-col gap-4">
          {draft.members.map((member, index) => {
            const label = t("dataflows.addressGroupMember", { index: index + 1 });
            return <li key={index} className="flex min-w-0 flex-col gap-2 border-b border-border/60 pb-4 last:border-0 last:pb-0">
              <DataflowEndpointPicker label={label} value={member} onChange={next => setMember(index, next)} disabled={!allowed || mutation.isPending} allowedTypes={ADDRESS_GROUP_MEMBER_TYPES} />
              {memberErrors[index] && <p role="alert" className="text-xs text-destructive">{memberErrors[index]}</p>}
              <Button type="button" variant="ghost" size="sm" className="self-start text-destructive"
                aria-label={`${t("dataflows.removeEndpoint")}: ${label}`}
                onClick={() => removeMember(index)} disabled={!allowed || mutation.isPending || draft.members.length <= 1}>
                <Trash2 aria-hidden="true" className="size-4" />{t("dataflows.removeEndpoint")}
              </Button>
            </li>;
          })}
        </ul>
        <p className="mt-3 text-xs tabular-nums text-muted-foreground">{draft.members.length} / {ADDRESS_GROUP_MEMBER_LIMIT}</p>
      </FormSection>
    </form>
  </EntityEditorPage>;
}

export default function AddressGroupEditorPage() {
  const { t } = useTranslation();
  const { id } = useParams<{ id?: string }>();
  const navigate = useNavigate();
  const group = useQuery({
    queryKey: ["inventory", "catalog", "address-groups", id],
    queryFn: () => getAddressGroup(id as string),
    enabled: !!id,
  });
  const back = () => void navigate(DATAFLOW_CATALOG_ROUTES.addressGroups);
  if (id && (group.isPending || group.isError || !group.data)) {
    return <EntityEditorPage scope="routed" title={t("dataflows.addressGroupsTitle")} onBack={back}
      backLabel={t("dataflows.addressGroupsTitle")} cancelLabel={t("cancel")}>
      {group.isPending ? <PageState kind="loading" title={t("loading")} />
        : <PageState kind="error" description={t("dataTable.errorDescription")}
          action={<Button variant="outline" onClick={() => void group.refetch()}>{t("retry")}</Button>} />}
    </EntityEditorPage>;
  }
  return <AddressGroupForm key={id ?? "new"} group={group.data} />;
}
