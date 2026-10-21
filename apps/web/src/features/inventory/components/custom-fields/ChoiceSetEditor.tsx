import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2 } from "lucide-react";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Switch } from "@/shared/ui-primitives/switch";
import { Label } from "@/shared/ui-primitives/label";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import FormSection from "@/shared/ui-shared/FormSection";
import type {
  Choice,
  ChoiceSet,
  ChoiceSetCreatePayload,
  ChoiceSetUpdatePayload,
} from "@/features/inventory/custom-fields-api";

interface ChoiceSetDraft {
  name: string;
  description: string;
  orderAlphabetically: boolean;
  choices: Choice[];
}

function draftFromChoiceSet(cs: ChoiceSet): ChoiceSetDraft {
  return {
    name: cs.name,
    description: cs.description ?? "",
    orderAlphabetically: cs.order_alphabetically,
    choices: cs.extra_choices.map((c) => ({ value: c.value, label: c.label })),
  };
}

const EMPTY_DRAFT: ChoiceSetDraft = {
  name: "",
  description: "",
  orderAlphabetically: false,
  choices: [{ value: "", label: "" }],
};

export interface ChoiceSetEditorProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing?: ChoiceSet;
  saving: boolean;
  onCreate: (body: ChoiceSetCreatePayload) => void;
  onUpdate: (id: string, body: ChoiceSetUpdatePayload) => void;
}

const ChoiceSetEditor = ({ open, onOpenChange, editing, saving, onCreate, onUpdate }: ChoiceSetEditorProps) => {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<ChoiceSetDraft>(() =>
    editing ? draftFromChoiceSet(editing) : { ...EMPTY_DRAFT, choices: [{ value: "", label: "" }] },
  );
  const [errors, setErrors] = useState<Record<string, string>>({});

  const setChoice = (index: number, key: keyof Choice, value: string) => {
    setDraft((prev) => ({
      ...prev,
      choices: prev.choices.map((c, i) => (i === index ? { ...c, [key]: value } : c)),
    }));
  };

  const addChoice = () => setDraft((prev) => ({ ...prev, choices: [...prev.choices, { value: "", label: "" }] }));
  const removeChoice = (index: number) =>
    setDraft((prev) => ({ ...prev, choices: prev.choices.filter((_, i) => i !== index) }));

  const handleSubmit = () => {
    const errs: Record<string, string> = {};
    if (!draft.name.trim()) errs.name = t("customFields.choiceSet.errors.nameRequired");

    // Keep only fully-specified rows; a value is mandatory, label defaults to value.
    const choices: Choice[] = draft.choices
      .map((c) => ({ value: c.value.trim(), label: c.label.trim() }))
      .filter((c) => c.value.length > 0)
      .map((c) => ({ value: c.value, label: c.label || c.value }));

    if (choices.length === 0) errs.choices = t("customFields.choiceSet.errors.choicesRequired");
    const values = choices.map((c) => c.value);
    if (new Set(values).size !== values.length) errs.choices = t("customFields.choiceSet.errors.duplicateValue");

    setErrors(errs);
    if (Object.keys(errs).length > 0) return;

    const shared = {
      name: draft.name.trim(),
      description: draft.description.trim(),
      order_alphabetically: draft.orderAlphabetically,
      extra_choices: choices,
    };
    if (editing) onUpdate(editing.id, shared satisfies ChoiceSetUpdatePayload);
    else onCreate(shared satisfies ChoiceSetCreatePayload);
    onOpenChange(false);
  };

  return (
    <EntityEditorHost
      open={open}
      width="compact"
      onOpenChange={onOpenChange}
      title={editing ? t("customFields.choiceSet.editTitle") : t("customFields.choiceSet.createTitle")}
      description={t("customFields.choiceSet.dialogDescription")}
      footer={
        <>
          <Button data-editor-cancel variant="outline" onClick={() => onOpenChange(false)}>{t("cancel")}</Button>
          <Button onClick={handleSubmit} disabled={saving}>
            {editing ? t("saveChanges") : t("create")}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <FormField label={t("name")} htmlFor="choice-set-name" required error={errors.name}>
          <FormFieldControl><Input
            id="choice-set-name"
            className="h-9 text-sm"
            value={draft.name}
            onChange={(e) => { setDraft((p) => ({ ...p, name: e.target.value })); if (errors.name) setErrors((p) => ({ ...p, name: "" })); }}
            placeholder={t("customFields.choiceSet.namePlaceholder")}
            maxLength={100}
            autoFocus
            aria-invalid={!!errors.name}
          /></FormFieldControl>
        </FormField>

        <FormField label={t("description")} htmlFor="choice-set-description">
          <FormFieldControl><Input
            id="choice-set-description"
            className="h-9 text-sm"
            value={draft.description}
            onChange={(e) => setDraft((p) => ({ ...p, description: e.target.value }))}
            placeholder={t("customFields.choiceSet.descriptionPlaceholder")}
            maxLength={255}
          /></FormFieldControl>
        </FormField>

        <div className="flex items-center justify-between gap-4">
          <Label htmlFor="cs-order-alpha" className="text-sm cursor-pointer">
            {t("customFields.choiceSet.orderAlphabetically")}
          </Label>
          <Switch
            id="cs-order-alpha"
            checked={draft.orderAlphabetically}
            onCheckedChange={(v) => setDraft((p) => ({ ...p, orderAlphabetically: v }))}
          />
        </div>

        {/* Choices: (value, label) rows */}
        <FormSection title={t("customFields.choiceSet.choices")} required divider contentClassName="flex flex-col gap-2" action={
            <Button type="button" variant="ghost" size="sm" className="h-9 text-sm gap-1" onClick={addChoice}>
              <Plus className="h-3 w-3" /> {t("customFields.choiceSet.addChoice")}
            </Button>
          }>
          <div className="grid grid-cols-[1fr_1fr_auto] gap-2 px-1">
            <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
              {t("customFields.choiceSet.value")}
            </span>
            <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
              {t("customFields.choiceSet.label")}
            </span>
            <span className="w-7" />
          </div>
          <FormField error={errors.choices}>
          <FormFieldControl>
          <div role="group" aria-label={t("customFields.choiceSet.choices")} className="space-y-1.5">
            {draft.choices.map((choice, index) => (
              <div key={index} className="grid grid-cols-[1fr_1fr_auto] gap-2 items-center">
                <Input
                  className="h-9 text-sm font-mono"
                  value={choice.value}
                  onChange={(e) => setChoice(index, "value", e.target.value)}
                  placeholder="prod"
                  spellCheck={false}
                  aria-label={t("customFields.choiceSet.value")}
                />
                <Input
                  className="h-9 text-sm"
                  value={choice.label}
                  onChange={(e) => setChoice(index, "label", e.target.value)}
                  placeholder={t("customFields.choiceSet.labelPlaceholder")}
                  aria-label={t("customFields.choiceSet.label")}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  className="text-muted-foreground hover:text-destructive shrink-0"
                  aria-label={t("remove")}
                  onClick={() => removeChoice(index)}
                  disabled={draft.choices.length === 1}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            ))}
          </div>
          </FormFieldControl>
          </FormField>
        </FormSection>
      </div>
    </EntityEditorHost>
  );
};

export default ChoiceSetEditor;
