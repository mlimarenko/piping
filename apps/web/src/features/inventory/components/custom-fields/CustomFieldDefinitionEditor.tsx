import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import FormSection from "@/shared/ui-shared/FormSection";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Switch } from "@/shared/ui-primitives/switch";
import { Label } from "@/shared/ui-primitives/label";
import FormField from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import {
  FIELD_TYPES,
  UI_EDITABLE_OPTIONS,
  UI_VISIBLE_OPTIONS,
  isChoiceFieldType,
  isNumericFieldType,
  isObjectFieldType,
  supportsRegexValidation,
  type ChoiceSet,
  type ContentType,
  type CustomFieldDefinition,
  type CustomFieldDefinitionCreatePayload,
  type CustomFieldDefinitionUpdatePayload,
  type FieldType,
} from "@/features/inventory/custom-fields-api";

/** Flat editable draft for the definition form. */
interface DefinitionDraft {
  name: string;
  label: string;
  fieldType: FieldType;
  description: string;
  groupName: string;
  weight: string;
  required: boolean;
  enforceUnique: boolean;
  isSecret: boolean;
  uiVisible: string;
  uiEditable: string;
  choiceSetId: string;
  relatedObjectTypeId: string;
  validationMinimum: string;
  validationMaximum: string;
  validationRegex: string;
  objectTypes: string[];
}

const NAME_RE = /^[a-z][a-z0-9_]{0,49}$/;

function draftFromDefinition(def: CustomFieldDefinition): DefinitionDraft {
  return {
    name: def.name,
    label: def.label,
    fieldType: def.field_type as FieldType,
    description: def.description ?? "",
    groupName: def.group_name ?? "",
    weight: String(def.weight ?? 100),
    required: def.required,
    enforceUnique: def.enforce_unique,
    isSecret: def.is_secret,
    uiVisible: def.ui_visible,
    uiEditable: def.ui_editable,
    choiceSetId: def.choice_set_id ?? "",
    relatedObjectTypeId: def.related_object_type_id ?? "",
    validationMinimum: def.validation_minimum != null ? String(def.validation_minimum) : "",
    validationMaximum: def.validation_maximum != null ? String(def.validation_maximum) : "",
    validationRegex: def.validation_regex ?? "",
    objectTypes: [...(def.object_types ?? [])],
  };
}

const EMPTY_DRAFT: DefinitionDraft = {
  name: "",
  label: "",
  fieldType: "text",
  description: "",
  groupName: "",
  weight: "100",
  required: false,
  enforceUnique: false,
  isSecret: false,
  uiVisible: "always",
  uiEditable: "yes",
  choiceSetId: "",
  relatedObjectTypeId: "",
  validationMinimum: "",
  validationMaximum: "",
  validationRegex: "",
  objectTypes: [],
};

function numOrNull(raw: string): number | null {
  if (raw.trim() === "") return null;
  const n = Number(raw);
  return Number.isFinite(n) ? n : null;
}

function contentTypeLabel(type: ContentType): string {
  return `${type.label} · ${type.token}`;
}

function withMissingSelectedItems(
  items: Array<{ id: string; label: string }>,
  selectedIds: string[],
  unknownLabel: string,
): Array<{ id: string; label: string }> {
  const known = new Set(items.map((item) => item.id));
  const missing = selectedIds
    .filter((id) => !known.has(id))
    .map((id) => ({ id, label: `${unknownLabel} · ${id.slice(0, 8)}...` }));
  return missing.length === 0 ? items : [...items, ...missing];
}

export interface CustomFieldDefinitionEditorProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing?: CustomFieldDefinition;
  choiceSets: ChoiceSet[];
  contentTypes: ContentType[];
  contentTypesLoading: boolean;
  contentTypesError: boolean;
  saving: boolean;
  onCreate: (body: CustomFieldDefinitionCreatePayload) => void;
  onUpdate: (id: string, body: CustomFieldDefinitionUpdatePayload) => void;
}

const CustomFieldDefinitionEditor = ({
  open,
  onOpenChange,
  editing,
  choiceSets,
  contentTypes,
  contentTypesLoading,
  contentTypesError,
  saving,
  onCreate,
  onUpdate,
}: CustomFieldDefinitionEditorProps) => {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<DefinitionDraft>(() =>
    editing ? draftFromDefinition(editing) : { ...EMPTY_DRAFT },
  );
  const [errors, setErrors] = useState<Record<string, string>>({});

  const set = <K extends keyof DefinitionDraft>(key: K, value: DefinitionDraft[K]) => {
    setDraft((prev) => ({ ...prev, [key]: value }));
    if (errors[key]) setErrors((prev) => ({ ...prev, [key]: "" }));
  };

  const isChoice = isChoiceFieldType(draft.fieldType);
  const isObject = isObjectFieldType(draft.fieldType);
  const isNumeric = isNumericFieldType(draft.fieldType);
  const hasRegex = supportsRegexValidation(draft.fieldType);
  const enforceUniqueAllowed = draft.fieldType !== "boolean";

  const choiceSetItems = useMemo(
    () => choiceSets.map((cs) => ({ id: cs.id, label: cs.name })),
    [choiceSets],
  );

  const relatedContentTypeItems = useMemo(
    () => contentTypes.map((ct) => ({ id: ct.id, label: contentTypeLabel(ct) })),
    [contentTypes],
  );
  const scopeContentTypeItems = useMemo(
    () =>
      withMissingSelectedItems(
        contentTypes
          .filter((ct) => ct.supported_for_custom_fields)
          .map((ct) => ({ id: ct.id, label: contentTypeLabel(ct) })),
        draft.objectTypes,
        t("customFields.def.unknownContentType"),
      ),
    [contentTypes, draft.objectTypes, t],
  );
  const selectableRelatedContentTypeItems = useMemo(
    () =>
      withMissingSelectedItems(
        relatedContentTypeItems,
        draft.relatedObjectTypeId ? [draft.relatedObjectTypeId] : [],
        t("customFields.def.unknownContentType"),
      ),
    [draft.relatedObjectTypeId, relatedContentTypeItems, t],
  );

  const validate = (): boolean => {
    const errs: Record<string, string> = {};
    if (!editing) {
      if (!draft.name.trim()) errs.name = t("customFields.def.errors.nameRequired");
      else if (!NAME_RE.test(draft.name.trim())) errs.name = t("customFields.def.errors.nameFormat");
    }
    if (!draft.label.trim()) errs.label = t("customFields.def.errors.labelRequired");
    if (isChoice && !draft.choiceSetId) errs.choiceSetId = t("customFields.def.errors.choiceSetRequired");
    if (isObject && !draft.relatedObjectTypeId) {
      errs.relatedObjectTypeId = t("customFields.def.errors.relatedObjectTypeRequired");
    }
    setErrors(errs);
    return Object.keys(errs).length === 0;
  };

  const handleSubmit = () => {
    if (!validate()) return;

    const shared = {
      label: draft.label.trim(),
      field_type: draft.fieldType,
      description: draft.description.trim(),
      group_name: draft.groupName.trim(),
      weight: Number(draft.weight) || 0,
      required: draft.required,
      enforce_unique: enforceUniqueAllowed ? draft.enforceUnique : false,
      is_secret: draft.isSecret,
      ui_visible: draft.uiVisible,
      ui_editable: draft.uiEditable,
      choice_set_id: isChoice && draft.choiceSetId ? draft.choiceSetId : null,
      related_object_type_id: isObject && draft.relatedObjectTypeId ? draft.relatedObjectTypeId : null,
      validation_minimum: isNumeric ? numOrNull(draft.validationMinimum) : null,
      validation_maximum: isNumeric ? numOrNull(draft.validationMaximum) : null,
      validation_regex: hasRegex ? draft.validationRegex.trim() : "",
      object_types: draft.objectTypes,
    };

    if (editing) {
      onUpdate(editing.id, shared satisfies CustomFieldDefinitionUpdatePayload);
    } else {
      onCreate({ ...shared, name: draft.name.trim() } satisfies CustomFieldDefinitionCreatePayload);
    }
    onOpenChange(false);
  };

  return (
    <EntityEditorHost
      open={open}
      onOpenChange={onOpenChange}
      title={editing ? t("customFields.def.editTitle") : t("customFields.def.createTitle")}
      description={t("customFields.def.dialogDescription")}
      footer={
        <>
          <Button data-editor-cancel variant="outline" onClick={() => onOpenChange(false)}>
            {t("cancel")}
          </Button>
          <Button onClick={handleSubmit} disabled={saving}>
            {editing ? t("saveChanges") : t("create")}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {/* Identity */}
        <FormSection title={t("customFields.def.sections.definition")} contentClassName="flex flex-col gap-4">
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <FormField
            label={t("customFields.def.name")}
            required
            help={t("customFields.def.nameHint")}
            error={errors.name}
            className="space-y-1"
          >
            <Input
              className="h-9 text-sm font-mono"
              value={draft.name}
              onChange={(e) => set("name", e.target.value)}
              placeholder="cf_owner"
              disabled={!!editing}
              maxLength={50}
              autoFocus={!editing}
            />
          </FormField>
          <FormField
            label={t("customFields.def.label")}
            required
            error={errors.label}
            className="space-y-1"
          >
            <Input
              className="h-9 text-sm"
              value={draft.label}
              onChange={(e) => set("label", e.target.value)}
              placeholder={t("customFields.def.labelPlaceholder")}
              maxLength={100}
            />
          </FormField>
        </div>

        {/* Type + group */}
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <FormSelect
            label={t("customFields.def.fieldType")}
            value={draft.fieldType}
            onValueChange={(v) => set("fieldType", v)}
            options={FIELD_TYPES.map((ft) => ({
              value: ft,
              label: t(`customFields.fieldTypes.${ft}`),
            }))}
            className="space-y-1"
          />
          <FormField label={t("customFields.def.group")} className="space-y-1">
            <Input
              className="h-9 text-sm"
              value={draft.groupName}
              onChange={(e) => set("groupName", e.target.value)}
              placeholder={t("customFields.def.groupPlaceholder")}
              maxLength={50}
            />
          </FormField>
        </div>

        {/* Description */}
        <FormField label={t("description")} className="space-y-1">
          <Textarea
            className="text-sm min-h-[72px] resize-y"
            value={draft.description}
            onChange={(e) => set("description", e.target.value)}
            placeholder={t("customFields.def.descriptionPlaceholder")}
            maxLength={500}
          />
        </FormField>

        {/* Choice set (select/multiselect only) */}
        {isChoice && (
          <FormField
            label={t("customFields.def.choiceSet")}
            required
            error={errors.choiceSetId}
            className="space-y-1"
          >
            <EntitySelect
              items={choiceSetItems}
              value={draft.choiceSetId}
              onValueChange={(id) => set("choiceSetId", id)}
              placeholder={t("customFields.def.choiceSetPlaceholder")}
              aria-label={t("customFields.def.choiceSet")}
            />
            {choiceSets.length === 0 && (
              <p className="text-xs text-status-warning">{t("customFields.def.noChoiceSets")}</p>
            )}
          </FormField>
        )}

        {/* Related object type (object/multiobject only) */}
        {isObject && (
          <FormField
            label={t("customFields.def.relatedObjectType")}
            help={t("customFields.def.relatedObjectTypeHint")}
            error={errors.relatedObjectTypeId}
            className="space-y-1"
          >
            <EntitySelect
              items={selectableRelatedContentTypeItems}
              value={draft.relatedObjectTypeId}
              onValueChange={(id) => set("relatedObjectTypeId", id)}
              placeholder={contentTypesLoading ? t("customFields.def.contentTypesLoading") : t("customFields.def.relatedObjectTypePlaceholder")}
              triggerClassName="h-9 text-sm"
              disabled={contentTypesLoading || contentTypesError}
              aria-label={t("customFields.def.relatedObjectType")}
            />
            {contentTypesError && (
              <p className="text-xs text-status-warning">{t("customFields.def.contentTypesError")}</p>
            )}
          </FormField>
        )}

        {/* Numeric validation bounds */}
        {isNumeric && (
          <div className="grid grid-cols-2 gap-3">
            <FormField label={t("customFields.def.validationMin")} className="space-y-1">
              <Input
                type="number"
                inputMode={draft.fieldType === "decimal" ? "decimal" : "numeric"}
                step={draft.fieldType === "decimal" ? "any" : "1"}
                className="h-9 text-sm font-mono"
                value={draft.validationMinimum}
                onChange={(e) => set("validationMinimum", e.target.value)}
                aria-label={t("customFields.def.validationMin")}
              />
            </FormField>
            <FormField label={t("customFields.def.validationMax")} className="space-y-1">
              <Input
                type="number"
                inputMode={draft.fieldType === "decimal" ? "decimal" : "numeric"}
                step={draft.fieldType === "decimal" ? "any" : "1"}
                className="h-9 text-sm font-mono"
                value={draft.validationMaximum}
                onChange={(e) => set("validationMaximum", e.target.value)}
                aria-label={t("customFields.def.validationMax")}
              />
            </FormField>
          </div>
        )}

        {/* Regex validation (text/url only) */}
        {hasRegex && (
          <FormField label={t("customFields.def.validationRegex")} className="space-y-1">
            <Input
              className="h-9 text-sm font-mono"
              value={draft.validationRegex}
              onChange={(e) => set("validationRegex", e.target.value)}
              placeholder="^[A-Z]{3}-\d+$"
              spellCheck={false}
            />
          </FormField>
        )}

        </FormSection>
        <FormSection title={t("customFields.def.sections.behavior")} divider contentClassName="flex flex-col gap-4">
        {/* UI visibility / editability + weight */}
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
          <FormSelect
            label={t("customFields.def.uiVisible")}
            value={draft.uiVisible}
            onValueChange={(v) => set("uiVisible", v)}
            options={UI_VISIBLE_OPTIONS.map((o) => ({
              value: o,
              label: t(`customFields.uiVisible.${o}`),
            }))}
            className="space-y-1"
          />
          <FormSelect
            label={t("customFields.def.uiEditable")}
            value={draft.uiEditable}
            onValueChange={(v) => set("uiEditable", v)}
            options={UI_EDITABLE_OPTIONS.map((o) => ({
              value: o,
              label: t(`customFields.uiEditable.${o}`),
            }))}
            className="space-y-1"
          />
          <FormField label={t("customFields.def.weight")} className="space-y-1">
            <Input
              type="number"
              inputMode="numeric"
              className="h-9 text-sm font-mono"
              value={draft.weight}
              onChange={(e) => set("weight", e.target.value)}
              aria-label={t("customFields.def.weight")}
            />
          </FormField>
        </div>

        {/* Flags */}
        <div className="flex flex-col gap-4">
          <div className="flex items-center justify-between">
            <Label htmlFor="cf-required" className="text-sm cursor-pointer">
              {t("customFields.def.required")}
            </Label>
            <Switch id="cf-required" checked={draft.required} onCheckedChange={(v) => set("required", v)} />
          </div>
          <div className="flex items-center justify-between">
            <Label htmlFor="cf-secret" className="text-sm cursor-pointer">
              {t("customFields.def.isSecret")}
            </Label>
            <Switch id="cf-secret" checked={draft.isSecret} onCheckedChange={(v) => set("isSecret", v)} />
          </div>
          <div className="flex items-center justify-between">
            <Label
              htmlFor="cf-unique"
              className={`text-sm cursor-pointer ${!enforceUniqueAllowed ? "opacity-50" : ""}`}
            >
              {t("customFields.def.enforceUnique")}
            </Label>
            <Switch
              id="cf-unique"
              checked={enforceUniqueAllowed && draft.enforceUnique}
              onCheckedChange={(v) => set("enforceUnique", v)}
              disabled={!enforceUniqueAllowed}
            />
          </div>
        </div>

        {/* Scope: content types */}
        </FormSection>
        <FormSection title={t("customFields.def.scope")} description={t("customFields.def.scopeHint")} divider contentClassName="flex flex-col gap-2">
          <MultiSelect
            items={scopeContentTypeItems}
            value={draft.objectTypes}
            onValueChange={(ids) => set("objectTypes", ids)}
            placeholder={contentTypesLoading ? t("customFields.def.contentTypesLoading") : t("customFields.def.contentTypePlaceholder")}
            searchPlaceholder={t("customFields.def.contentTypeSearchPlaceholder")}
            emptyText={t("customFields.def.contentTypeEmpty")}
            disabled={contentTypesLoading || contentTypesError}
            aria-label={t("customFields.def.scope")}
          />
          {contentTypesError && (
            <p className="text-xs text-status-warning">{t("customFields.def.contentTypesError")}</p>
          )}
          {draft.objectTypes.length === 0 && (
            <p className="text-xs text-status-warning pt-1">{t("customFields.def.scopeEmpty")}</p>
          )}
        </FormSection>
      </div>
    </EntityEditorHost>
  );
};

export default CustomFieldDefinitionEditor;
