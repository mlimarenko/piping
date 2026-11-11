import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import EntityPicker from "@/shared/forms/EntityPicker";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import EntityCustomFieldsSection, { type EntityCustomFieldsHandle } from "./components/custom-fields/EntityCustomFieldsSection";
import { listContentTypes, type CustomFieldEntityKind } from "./custom-fields-api";
import { inventoryResourceOptions, inventoryResourceOptionsQuery } from "./resourceOptions";

/** Form state shared by catalog editors. The wire payload remains schema-derived. */
export type CatalogMetadataDraft = {
  owner_id?: string | null;
  tenant_id?: string | null;
  tags?: string[];
};

function ResourceRelation({ entity, label, value, onChange, disabled }: Readonly<{
  entity: "Owners" | "Tenants";
  label: string;
  value: string | null | undefined;
  onChange: (value: string | null) => void;
  disabled: boolean;
}>) {
  const { t } = useTranslation();
  const options = useQuery({ ...inventoryResourceOptionsQuery(entity), enabled: !disabled });
  return <div className="flex min-w-0 flex-col gap-1">
    <EntityPicker entity={entity} label={label} value={value ?? ""} onChange={next => onChange(next || null)}
      options={inventoryResourceOptions(entity, options.data?.items ?? [])}
      loading={options.isPending} disabled={disabled || options.isError} />
    {value && !disabled && <Button type="button" variant="ghost" size="sm" className="self-start text-xs" onClick={() => onChange(null)}>{t("clear")}</Button>}
  </div>;
}

/** Native owner, tenant, tag and custom-field metadata; no parallel metadata subsystem. */
export default function DataflowCatalogMetadataFields({ draft, onChange, disabled = false, tagsDisabled = false, entityKind, entityId, creationScopeToken, customFields, onRegisterCustomFields }: Readonly<{
  draft: CatalogMetadataDraft;
  onChange: (change: CatalogMetadataDraft) => void;
  disabled?: boolean;
  tagsDisabled?: boolean;
  entityKind?: CustomFieldEntityKind;
  entityId?: string;
  creationScopeToken?: string;
  customFields?: Record<string, unknown>;
  onRegisterCustomFields?: (handle: EntityCustomFieldsHandle | null) => void;
}>) {
  const { t } = useTranslation();
  const tags = useQuery({ ...inventoryResourceOptionsQuery("Tags"), enabled: !tagsDisabled && !disabled });
  const contentTypes = useQuery({ queryKey: ["inventory-content-types"], queryFn: listContentTypes, enabled: !!entityKind });
  const customFieldsSupported = !!entityKind && !!contentTypes.data?.some(type =>
    type.token === entityKind && type.supported_for_custom_fields && type.capabilities?.custom_fields !== false);
  return <>
    <div className="grid min-w-0 gap-4 @lg:grid-cols-2">
      <ResourceRelation entity="Owners" label={t("inventory.resources.fields.owner")} value={draft.owner_id} onChange={owner_id => onChange({ ...draft, owner_id })} disabled={disabled} />
      <ResourceRelation entity="Tenants" label={t("inventory.resources.fields.tenant")} value={draft.tenant_id} onChange={tenant_id => onChange({ ...draft, tenant_id })} disabled={disabled} />
      {!tagsDisabled && <div className="min-w-0 @lg:col-span-2">
        <MultiSelect aria-label={t("inventory.resources.fields.tags")} items={(tags.data?.items ?? []).map(row => ({ id: row.id, label: row.name, searchText: row.slug }))}
          value={draft.tags ?? []} onValueChange={tags => onChange({ ...draft, tags })} disabled={disabled || tags.isPending || tags.isError} />
      </div>}
    </div>
    {entityKind && customFieldsSupported && <div className="min-w-0 border-t border-border pt-4">
      <EntityCustomFieldsSection entityKind={entityKind} entityId={entityId} customFields={customFields ?? {}}
        creationScopeToken={creationScopeToken} onRegister={onRegisterCustomFields ?? (() => {})} />
    </div>}
  </>;
}
