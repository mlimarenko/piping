import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import FormSection from "@/shared/ui-shared/FormSection";
import { RadioGroup, RadioGroupItem } from "@/shared/ui-primitives/radio-group";
import ScrollableDialog from "@/shared/ui-primitives/scrollable-dialog";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import type { EntityPickerOption } from "@/shared/forms/EntityPicker";
import type { EntityFormValues } from "@/shared/forms/entityForm";
import { inventoryEntityHasStatus, type InventoryEntityKey } from "@/shared/api/types";
import {
  fieldsByTab,
  statusOptions,
  type FieldConfig,
  type Resource,
} from "@/features/inventory/components/inventory/inventoryTypes";

/** A bulk-edit patch: each entry sets a value or clears the column with null. */
export type BulkEditPatch = Record<string, string | null>;

type FieldMode = "keep" | "set" | "clear";

interface BulkEditDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  entity: InventoryEntityKey;
  /** The rows currently selected — drives the "mixed values" hints. */
  selectedRows: Resource[];
  /** Selectable options for related pickers. */
  loadOptions: (entity: InventoryEntityKey) => EntityPickerOption[];
  /** Reports related entities visible in active "set" controls. */
  onVisibleRelatedEntitiesChange?: (entities: InventoryEntityKey[]) => void;
  /** Loading state for a related entity's option list. */
  isOptionsLoading?: (entity: InventoryEntityKey) => boolean;
  /** Applies the assembled patch to every selected row. */
  onApply: (patch: BulkEditPatch) => void;
  /** Opens the recursive create flow for a related entity. */
  onCreateRelated: (entity: InventoryEntityKey) => void;
}

/** Synthetic FieldConfig for the entity-level status select, so it flows
 * through the same field-grid machinery as the configured fields. */
const STATUS_FIELD: FieldConfig = {
  key: "status",
  label: "status",
  type: "select",
  options: statusOptions.map((s) => s.value),
};

/** The state key the form/payload uses for a field (FK column for related). */
const stateKeyOf = (f: FieldConfig): string => f.idKey ?? f.key;

/** Numeric columns opt into clearing through their nullable form empty value. */
const isClearable = (f: FieldConfig): boolean =>
  f.type === "textarea" || (f.type === "related" && !f.required) ||
  (f.type === "number" && f.emptyValue === "");

const BulkEditDialog = ({
  open,
  onOpenChange,
  entity,
  selectedRows,
  loadOptions,
  onVisibleRelatedEntitiesChange,
  isOptionsLoading,
  onApply,
  onCreateRelated,
}: BulkEditDialogProps) => {
  const { t } = useTranslation();

  // Bulk-editable whitelist: status (if any) + every non-free-text field
  // (related / select / textarea). Plain text fields are excluded — bulk-typing
  // identical free text across rows is rarely intended. `objectRef` targets are
  // excluded too: their two-part (type + entity) selection has no single-value
  // bulk-patch representation, and mass-assigning one polymorphic target across
  // heterogeneous rows is never the intent.
  const fields = useMemo<FieldConfig[]>(() => {
    const base = (fieldsByTab[entity] || []).filter(
      (f) =>
        f.type !== "text" &&
        f.type !== "icon" &&
        f.type !== "objectRef" &&
        // Identity collections use full replacement, not a scalar bulk patch.
        f.type !== "identity-multiple" &&
        // `nested-children` is a host-rendered inline editor with no single-value
        // bulk-patch representation — never a bulk-editable field.
        f.type !== "nested-children",
    );
    return inventoryEntityHasStatus(entity) ? [STATUS_FIELD, ...base] : base;
  }, [entity]);

  const [modes, setModes] = useState<Record<string, FieldMode>>({});
  const [draft, setDraft] = useState<EntityFormValues>({});

  const reset = () => {
    setModes({});
    setDraft({});
  };

  const handleOpenChange = (v: boolean) => {
    if (!v) reset();
    onOpenChange(v);
  };

  const modeOf = useCallback((key: string): FieldMode => modes[key] ?? "keep", [modes]);

  // True when the selected rows do not all share the same value for this field
  // (FK fields compare on their idKey column).
  const hasMixedValues = (f: FieldConfig): boolean => {
    if (selectedRows.length < 2) return false;
    const key = stateKeyOf(f);
    // `Resource` is a local UI type; comparing dynamic per-field keys across rows
    // is structural index access on the local type, not a backend-contract bypass.
    const first = (selectedRows[0] as unknown as Record<string, unknown>)[key];
    return selectedRows.some(
      (r) => (r as unknown as Record<string, unknown>)[key] !== first,
    );
  };

  const setFieldMode = (key: string, mode: FieldMode) =>
    setModes((prev) => ({ ...prev, [key]: mode }));

  const setDraftField = (key: string, value: string) =>
    setDraft((prev) => ({ ...prev, [key]: value }));

  // Count of fields actively contributing a change.
  const changeCount = fields.filter((f) => {
    const m = modeOf(stateKeyOf(f));
    if (m === "clear") return true;
    if (m === "set") return (draft[stateKeyOf(f)] ?? "").trim() !== "";
    return false;
  }).length;

  const visibleRelatedEntities = useMemo(
    () =>
      Array.from(
        new Set(
          fields
            .filter((f) => modeOf(stateKeyOf(f)) === "set" && f.type === "related" && f.relatedTab)
            .map((f) => f.relatedTab as InventoryEntityKey),
        ),
      ),
    [fields, modeOf],
  );

  useEffect(() => {
    onVisibleRelatedEntitiesChange?.(visibleRelatedEntities);
    return () => onVisibleRelatedEntitiesChange?.([]);
  }, [onVisibleRelatedEntitiesChange, visibleRelatedEntities]);

  const handleApply = () => {
    const patch: BulkEditPatch = {};
    for (const f of fields) {
      const key = stateKeyOf(f);
      const mode = modeOf(key);
      if (mode === "clear") {
        patch[key] = null;
      } else if (mode === "set") {
        const value = (draft[key] ?? "").trim();
        if (value) patch[key] = value;
      }
    }
    if (Object.keys(patch).length === 0) return;
    onApply(patch);
    reset();
  };

  return (
    <ScrollableDialog
      open={open}
      onOpenChange={handleOpenChange}
      title={t("inventory.resources.bulkEditTitle", { count: selectedRows.length })}
      footer={
        <>
          <Button variant="outline" size="sm" onClick={() => handleOpenChange(false)}>
            {t("cancel")}
          </Button>
          <Button size="sm" onClick={handleApply} disabled={changeCount === 0}>
            {t("inventory.resources.bulkEditApply", { count: selectedRows.length })}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {fields.map((f) => {
          const key = stateKeyOf(f);
          const mode = modeOf(key);
          const label =
            f.key === "status" ? t("status") : t(f.label, { defaultValue: f.label });
          const clearable = isClearable(f);
          return (
            <FormSection key={key} title={label} divider contentClassName="flex flex-col gap-2">
              <RadioGroup
                className="flex flex-wrap gap-4"
                aria-label={label}
                value={mode}
                onValueChange={(v) => setFieldMode(key, v as FieldMode)}
              >
                <label className="flex items-center gap-1.5 text-sm">
                  <RadioGroupItem value="keep" /> {t("inventory.resources.fieldMode.keep")}
                </label>
                <label className="flex items-center gap-1.5 text-sm">
                  <RadioGroupItem value="set" /> {t("inventory.resources.fieldMode.set")}
                </label>
                {clearable && (
                  <label className="flex items-center gap-1.5 text-sm">
                    <RadioGroupItem value="clear" /> {t("inventory.resources.fieldMode.clear")}
                  </label>
                )}
              </RadioGroup>

              {mode === "keep" && hasMixedValues(f) && (
                <p className="text-xs text-muted-foreground">
                  {t("inventory.resources.mixedValues")}
                </p>
              )}

              {mode === "set" && (
                <EntityFormBody
                  entity={entity}
                  draft={draft}
                  onField={setDraftField}
                  loadOptions={loadOptions}
                  isOptionsLoading={isOptionsLoading}
                  onRequestCreate={(related) => onCreateRelated(related)}
                  visibleKeys={new Set([key])}
                />
              )}
            </FormSection>
          );
        })}
      </div>
    </ScrollableDialog>
  );
};

export default BulkEditDialog;
