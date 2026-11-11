import { useCallback, useEffect, useMemo, type ComponentProps, type ReactNode } from "react";
import { useQuery, type UseQueryOptions } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import FormField from "@/shared/ui-shared/FormField";
import FormSection from "@/shared/ui-shared/FormSection";
import FormSelect from "@/shared/ui-shared/FormSelect";
import EntityPicker, { type EntityPickerOption } from "@/shared/forms/EntityPicker";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import { TagObjectTypesField } from "@/features/inventory/components/inventory/ContentTypeScopeField";
import { customFieldContentTypeTokenFor } from "@/features/inventory/tableConfig";
import { tagAllowsAttachment } from "@/features/inventory/resourceOptions";
import IdentitySelection from "@/features/access-control/components/access/IdentitySelection";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { Button } from "@/shared/ui-primitives/button";
import ColorField from "@/shared/forms/ColorField";
import RearPortMappingsField from "@/shared/forms/RearPortMappingsField";
import ManagedSecretField from "@/shared/forms/ManagedSecretField";
import { revealFhrpAuthKey } from "@/features/inventory/fhrp-auth-key";
import IconField from "@/shared/forms/IconField";
import MapPicker from "@/shared/forms/MapPicker";
import {
  entityHasName,
  entityRequiresName,
  objectRefTypeKey,
  type EntityFormValues,
} from "@/shared/forms/entityForm";
import { relatedEntitiesForEntity } from "@/shared/forms/entityRelatedEntities";
import { inventoryEntityHasStatus, type InventoryEntityKey } from "@/shared/api/types";
import {
  fieldsByTab,
  type FieldConfig,
  statusOptionsForEntity,
} from "@/features/inventory/components/inventory/inventoryTypes";

const EMPTY_FIELDS: FieldConfig[] = [];

/** The two field keys that, when both present and visible, collapse into a
 *  single {@link MapPicker} (manual lat/lng inputs + a "pick on map" affordance)
 *  instead of two bare number inputs. */
const LAT_KEY = "latitude";
const LNG_KEY = "longitude";

/** Pure field-grid for one entity. Owns no dialog chrome and no submit button —
 * the host supplies those. Related fields render an {@link EntityPicker} whose
 * "+" delegates to `onRequestCreate`. */
interface EntityFormBodyProps {
  editingId?: string;
  entity: InventoryEntityKey;
  draft: EntityFormValues;
  errors?: Record<string, string>;
  onField: (key: string, value: string) => void;
  /** Returns selectable options for a related entity. */
  loadOptions: (entity: InventoryEntityKey) => EntityPickerOption[];
  /** Reports related entities currently visible in this form body. */
  onVisibleRelatedEntitiesChange?: (entities: InventoryEntityKey[]) => void;
  /** Loading state for a related entity's option list. */
  isOptionsLoading?: (entity: InventoryEntityKey) => boolean;
  /** Invoked by a picker's "+" with the related entity to create. */
  onRequestCreate: (entity: InventoryEntityKey, returnToFieldKey: string) => void;
  /**
   * When provided, only fields whose state key is in this set are rendered
   * (plus `name`/`status` when their key is included). Omit for the full grid.
   */
  visibleKeys?: Set<string>;
  /** State keys whose inputs render disabled (non-editable). */
  disabledKeys?: Set<string>;
  /** Renders a `nested-children` marker field in place (at its position in the
   *  field order) — the inline nested-editor host. Omit and such markers are
   *  skipped (the default for nested child-row forms). */
  renderNested?: (relationshipsKey?: InventoryEntityKey) => ReactNode;
}

function RelatedEntityField({ required, emptyValue, ...pickerProps }: Readonly<ComponentProps<typeof EntityPicker> & Pick<FieldConfig, "required" | "emptyValue">>) {
  const { t } = useTranslation();
  const canClear = !required && emptyValue !== undefined && pickerProps.value && !pickerProps.disabled;
  return <div className="flex flex-col gap-1">
    <EntityPicker {...pickerProps} />
    {canClear && <Button type="button" variant="ghost" size="sm" className="self-start text-xs"
      aria-label={`${t("clear")} ${pickerProps.label}`} onClick={() => pickerProps.onChange("")}>
      {t("clear")}
    </Button>}
  </div>;
}

function NestedEntityField({ renderNested, relationshipsKey }: Readonly<Pick<EntityFormBodyProps, "renderNested"> & Pick<FieldConfig, "relationshipsKey">>) {
  const nested = renderNested?.(relationshipsKey);
  return nested ? <div className="min-w-0 @lg:col-span-2">{nested}</div> : null;
}

function RemoteObjectRefPicker({ optionsQuery, ...props }: Readonly<Omit<ComponentProps<typeof EntityPicker>, "options" | "entity"> & { optionsQuery: UseQueryOptions<EntityPickerOption[]> }>) {
  const { t } = useTranslation();
  const query = useQuery(optionsQuery);
  return <div className="flex flex-col gap-2">
    <EntityPicker {...props} options={query.data ?? []} loading={query.isPending} disabled={props.disabled || query.isError} />
    {query.isError && <div role="alert" className="text-sm text-destructive">
      {t("unknownError")}
      <Button type="button" variant="ghost" size="sm" onClick={() => void query.refetch()}>{t("retry")}</Button>
    </div>}
  </div>;
}

const EntityFormBody = ({
  entity,
  editingId,
  draft,
  errors = {},
  onField,
  loadOptions,
  onVisibleRelatedEntitiesChange,
  isOptionsLoading,
  onRequestCreate,
  visibleKeys,
  disabledKeys,
  renderNested,
}: Readonly<EntityFormBodyProps>) => {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const allFields = fieldsByTab[entity] ?? EMPTY_FIELDS;
  const isVisible = useCallback(
    (key: string) => !visibleKeys || visibleKeys.has(key),
    [visibleKeys],
  );
  const isDisabled = (key: string) => !!disabledKeys?.has(key) || !!fieldsByTab[entity]?.some(f => (f.idKey ?? f.key) === key && f.readOnly);
  const fields = useMemo(
    // `nested-children` markers render inline via `renderNested` at their position
    // in the field order; when no host slot is supplied (e.g. nested child-row
    // forms) they are skipped so they never produce an empty row.
    () =>
      allFields.filter(
        (f) =>
          (f.type !== "nested-children" || !!renderNested) &&
          isVisible((f.idKey ?? f.key) as string),
      ),
    [allFields, isVisible, renderNested],
  );
  const optionsFor = (f: FieldConfig, related: InventoryEntityKey): EntityPickerOption[] => {
    const options = loadOptions(related)
      .filter(option => related !== "Tags" || f.key !== "tags" || tagAllowsAttachment(option, customFieldContentTypeTokenFor(entity)))
      .filter(option => !f.optionsInterfaceTypes || f.optionsInterfaceTypes.includes(option.interfaceType ?? ""))
      .filter(option => !f.optionsSubdeviceRole || option.subdeviceRole === f.optionsSubdeviceRole);
    if (f.optionsDeviceKey) {
      const deviceId = draft[f.optionsDeviceKey];
      return deviceId ? options.filter(option => option.deviceId === deviceId) : [];
    }
    if (f.optionsScope) {
      const scopes = Object.entries(f.optionsScope) as [keyof EntityPickerOption, string][];
      if (!scopes.some(([, key]) => draft[key])) return [];
      return options.filter(option => scopes.every(([key, draftKey]) => (option[key] ?? "") === (draft[draftKey] ?? "")));
    }
    if (f.optionsSiteReference) {
      const ref = f.optionsSiteReference;
      const site = loadOptions(ref.relatedTab).find(option => option.id === draft[ref.draftKey])?.siteId;
      return site ? options.filter(option => option.siteId === site) : [];
    }
    return options;
  };
  // Changing a device invalidates every selection scoped to the previous device.
  const changeField = (key: string, value: string) => {
    onField(key, value);
    if (value === draft[key]) return;
    fields.forEach(f => {
      if (f.optionsDeviceKey === key || Object.values(f.optionsScope ?? {}).includes(key) || f.optionsSiteReference?.draftKey === key) onField(f.idKey ?? f.key, "");
    });
  };
  const hasStatus = inventoryEntityHasStatus(entity) && isVisible("status");
  const hasName = entityHasName(entity) && isVisible("name");
  // Merge the lat/lng pair into a single MapPicker only when BOTH fields exist
  // for this entity and both pass the visibility filter — otherwise each renders
  // as its own number input (keeps every non-Sites entity and partial-visibility
  // cases untouched).
  const mergeCoords = useMemo(
    () =>
      fields.some((f) => (f.idKey ?? f.key) === LAT_KEY) &&
      fields.some((f) => (f.idKey ?? f.key) === LNG_KEY),
    [fields],
  );
  const visibleRelatedEntities = useMemo(
    () => relatedEntitiesForEntity(entity, visibleKeys),
    [entity, visibleKeys],
  );

  useEffect(() => {
    onVisibleRelatedEntitiesChange?.(visibleRelatedEntities);
    return () => onVisibleRelatedEntitiesChange?.([]);
  }, [onVisibleRelatedEntitiesChange, visibleRelatedEntities]);

  const textInputPropsFor = (key: string): Pick<
    ComponentProps<typeof Input>,
    "autoComplete" | "inputMode" | "pattern" | "spellCheck" | "type"
  > => {
    const normalized = key.toLowerCase();
    if (normalized.includes("email")) return { autoComplete: "email", type: "email" };
    if (normalized.includes("phone")) return { autoComplete: "tel", inputMode: "tel", type: "tel" };
    if (normalized === "link" || normalized.includes("url")) return { autoComplete: "url", inputMode: "url", type: "url" };
    if (normalized.includes("date")) return { type: "date" };
    if (normalized.includes("ip") || normalized.includes("prefix")) return { inputMode: "text", spellCheck: false };
    if (normalized.includes("mac")) return { inputMode: "text", pattern: "[0-9A-Fa-f:.-]*", spellCheck: false };
    return { type: "text" };
  };

  return (
    <FormSection contentClassName="grid min-w-0 grid-cols-1 gap-4 @lg:grid-cols-2">
      {hasName && (
        <FormField
          label={t("name")}
          required={entityRequiresName(entity)}
          error={errors.name}
        >
          <Input
            aria-label={t("name")}
            value={draft.name || ""}
            onChange={(e) => onField("name", e.target.value)}
            placeholder={t("inventory.resources.form.namePlaceholder")}
            className="h-9 text-sm"
            maxLength={100}
            autoFocus
            disabled={isDisabled("name")}
          />
        </FormField>
      )}

      {hasStatus && (
        <FormSelect
          label={t("status")}
          value={draft.status || ""}
          onValueChange={(v) => onField("status", v)}
          options={statusOptionsForEntity(entity).map((s) => ({ value: s.value, label: t(s.label) }))}
          placeholder={t("inventory.resources.form.statusPlaceholder")}
          disabled={isDisabled("status")}
        />
      )}

      {fields.map((f) => {
        const stateKey = (f.idKey ?? f.key) as string;

        // Inline nested-editor host slot, rendered at this marker's position in
        // the field order (e.g. Device Interfaces right under "Primary IP").
        if (f.type === "nested-children") {
          return <NestedEntityField key={stateKey} renderNested={renderNested} relationshipsKey={f.relationshipsKey} />;
        }

        const label = t(f.label, { defaultValue: f.label });
        const placeholder = f.placeholder
          ? t(f.placeholder, { defaultValue: f.placeholder })
          : undefined;

        // Coordinate pair: render the combined MapPicker on the latitude row and
        // drop the longitude row (the picker owns both inputs). Falls through to
        // the normal number inputs when the pair is not merged for this entity.
        if (mergeCoords && stateKey === LNG_KEY) return null;
        if (mergeCoords && stateKey === LAT_KEY) {
          return (
            <MapPicker
              key={stateKey}
              lat={draft[LAT_KEY] || ""}
              lng={draft[LNG_KEY] || ""}
              onChange={(lat, lng) => {
                onField(LAT_KEY, lat);
                onField(LNG_KEY, lng);
              }}
              disabled={isDisabled(LAT_KEY) || isDisabled(LNG_KEY)}
            />
          );
        }

        return (
          <FormField
            key={stateKey}
            label={label}
            required={f.required}
            error={errors[stateKey]}
            className={f.type === "textarea" || f.type === "identity-multiple" ? "@lg:col-span-2" : undefined}
          >
            {f.type === "managed-secret" && <ManagedSecretField key={editingId ?? "create"}
              label={label} consumerType="inventory_fhrp_group" consumerId={editingId} fieldKey="auth_key"
              canReveal={can("READ", "INVENTORY") && can("SECRET", "INVENTORY")}
              intent={draft[`${stateKey}__intent`] ?? ""} value={draft[stateKey] ?? ""}
              disabled={isDisabled(stateKey)} onIntent={next => onField(`${stateKey}__intent`, next)}
              onValue={next => onField(stateKey, next)} reveal={revealFhrpAuthKey} />}
            {f.type === "related" && f.relatedTab && (
              <RelatedEntityField
                required={f.required}
                emptyValue={f.emptyValue}
                entity={f.relatedTab}
                value={draft[stateKey] || ""}
                onChange={(v) => changeField(stateKey, v)}
                options={optionsFor(f, f.relatedTab)}
                loading={isOptionsLoading?.(f.relatedTab) ?? false}
                disabled={isDisabled(stateKey) || (!!f.optionsDeviceKey && !draft[f.optionsDeviceKey])}
                placeholder={placeholder}
                label={label}
                onRequestCreate={can("CREATE", "INVENTORY") ? () => onRequestCreate(f.relatedTab as InventoryEntityKey, stateKey) : undefined}
              />
            )}

            {f.type === "identity-multiple" && f.identityKind && (
              <IdentitySelection kind={f.identityKind} label={label}
                value={draft[stateKey] ? draft[stateKey].split(",") : []}
                onChange={(ids) => onField(stateKey, ids.join(","))} disabled={isDisabled(stateKey)} />
            )}

            {f.type === "related-multiple" && f.relatedTab && <div className="flex flex-col gap-2">
              <MultiSelect aria-label={label} description={t(`inventory.resources.hints.entities.${f.relatedTab}`)} items={optionsFor(f, f.relatedTab).map(option => ({ id: option.id, label: option.name, color: option.color, searchText: option.searchText }))}
                value={draft[stateKey] ? draft[stateKey].split(",") : []} onValueChange={ids => onField(stateKey, ids.join(","))}
                disabled={isDisabled(stateKey) || (isOptionsLoading?.(f.relatedTab) ?? false)} />
              {placeholder && <p className="text-xs text-muted-foreground">{placeholder}</p>}
            </div>}

            {f.type === "content-type-scope" && <TagObjectTypesField label={label}
              value={draft[stateKey] ? draft[stateKey].split(",") : []}
              onValueChange={tokens => onField(stateKey, tokens.join(","))} disabled={isDisabled(stateKey)} />}

            {f.type === "objectRef" && f.objectTypes && (() => {
              const typeKey = objectRefTypeKey(stateKey);
              const token = draft[typeKey] || "";
              const chosen = f.objectTypes.find((o) => o.token === token);
              return (
                <div className="flex flex-col gap-2">
                  <FormSelect
                    value={token}
                    onValueChange={(v) => {
                      onField(typeKey, v);
                      // Changing the target type invalidates the picked entity.
                      onField(stateKey, "");
                    }}
                    options={f.objectTypes.map((o) => ({
                      value: o.token,
                      label: t(o.label, { defaultValue: o.token }),
                    }))}
                    placeholder={t("inventory.resources.form.objectTypePlaceholder")}
                    disabled={isDisabled(stateKey)}
                    aria-label={t("inventory.resources.form.objectTypePlaceholder")}
                  />
                  {chosen?.optionsQuery && <RemoteObjectRefPicker optionsQuery={chosen.optionsQuery}
                    value={draft[stateKey] || ""} onChange={value => onField(stateKey, value)}
                    disabled={isDisabled(stateKey)} placeholder={placeholder} label={label} />}
                  {chosen?.relatedTab && (
                    <EntityPicker
                      entity={chosen.relatedTab}
                      value={draft[stateKey] || ""}
                      onChange={(v) => onField(stateKey, v)}
                      options={optionsFor(f, chosen.relatedTab)}
                      loading={isOptionsLoading?.(chosen.relatedTab) ?? false}
                      disabled={isDisabled(stateKey)}
                      placeholder={placeholder}
                      label={label}
                       onRequestCreate={can("CREATE", "INVENTORY") ? () => onRequestCreate(chosen.relatedTab, stateKey) : undefined}
                    />
                  )}
                  {!f.required && f.emptyValue === null && (token || draft[stateKey]) && (
                    <Button type="button" variant="ghost" size="sm" className="self-start text-xs"
                      disabled={isDisabled(stateKey)} aria-label={`${t("clear")} ${label}`}
                      onClick={() => { onField(typeKey, ""); onField(stateKey, ""); }}>
                      {t("clear")}
                    </Button>
                  )}
                </div>
              );
            })()}

            {f.type === "select" && f.options && (
              <div className="flex items-center gap-2">
              <FormSelect
                className="min-w-0 flex-1"
                value={draft[stateKey] || ""}
                onValueChange={(v) => onField(stateKey, v)}
                options={f.options.map((o) => ({ value: o, label: o }))}
                placeholder={placeholder}
                disabled={isDisabled(stateKey)}
                aria-label={label}
              />
              {f.emptyValue !== undefined && !f.required && draft[stateKey] && (
                <Button type="button" variant="ghost" size="sm" disabled={isDisabled(stateKey)}
                  aria-label={`${t("clear")} ${label}`} onClick={() => onField(stateKey, "")}>
                  {t("clear")}
                </Button>
              )}
              </div>
            )}

            {f.type === "rear-port-mappings" && (
              <RearPortMappingsField
                label={label}
                value={draft[stateKey] ?? ""}
                onChange={(next) => onField(stateKey, next)}
                disabled={isDisabled(stateKey)}
                readOnly={f.readOnly}
                frontPositions={draft.positions ?? "1"}
                options={f.readOnly ? [] : optionsFor({ ...f, type: "related", relatedTab: "Rear Port Templates" }, "Rear Port Templates")}
                loading={isOptionsLoading?.("Rear Port Templates") ?? false}
              />
            )}

            {(f.type === "text" || f.type === "port-mappings" || f.type === "password") && (
              <Input
                autoComplete={f.type === "password" ? "new-password" : undefined}
                aria-label={label}
                {...textInputPropsFor(stateKey)}
                type={f.type === "password" ? "password" : "text"}
                value={draft[stateKey] || ""}
                onChange={(e) => onField(stateKey, e.target.value)}
                placeholder={placeholder}
                className="h-9 text-sm"
                maxLength={f.type === "port-mappings" ? undefined : f.maxLength || 255}
                aria-invalid={!!errors[stateKey]}
                disabled={isDisabled(stateKey)}
              />
            )}

            {f.type === "number" && (
              <Input
                type="number"
                inputMode={f.numberMode ?? "numeric"}
                step={f.step ?? (f.numberMode === "decimal" ? "any" : "1")}
                min={f.min}
                max={f.max}
                value={draft[stateKey] || ""}
                onChange={(e) => onField(stateKey, e.target.value)}
                placeholder={placeholder}
                className="h-9 text-sm"
                maxLength={f.maxLength || 255}
                disabled={isDisabled(stateKey)}
                aria-label={label}
              />
            )}

            {f.type === "color" && (
              <ColorField
                id={stateKey}
                value={draft[stateKey] || ""}
                onChange={(v) => onField(stateKey, v)}
                disabled={isDisabled(stateKey)}
                ariaLabel={label}
              />
            )}

            {f.type === "icon" && (
              <IconField
                iconName={draft[stateKey] || ""}
                iconDataUrl={draft[f.iconDataKey ?? "iconDataUrl"] || ""}
                onIconNameChange={(v) => onField(stateKey, v)}
                onIconDataUrlChange={(v) => onField(f.iconDataKey ?? "iconDataUrl", v)}
                disabled={isDisabled(stateKey)}
              />
            )}

            {f.type === "textarea" && (
              <Textarea
                aria-label={label}
                value={draft[stateKey] || ""}
                onChange={(e) => onField(stateKey, e.target.value)}
                placeholder={placeholder}
                className="text-xs min-h-[60px] resize-none"
                maxLength={f.maxLength || 255}
                disabled={isDisabled(stateKey)}
              />
            )}
          </FormField>
        );
      })}
    </FormSection>
  );
};

export default EntityFormBody;
