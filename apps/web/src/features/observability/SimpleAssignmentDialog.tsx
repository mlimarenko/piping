import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Switch } from "@/shared/ui-primitives/switch";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FormField from "@/shared/ui-shared/FormField";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import ScopeFields from "./ScopeFields";
import { type MonitoringScopeType } from "./scope";
import type { ScopeEntities } from "./useScopeEntities";

export interface AssignmentDraft {
  targetId: string;
  scopeType: MonitoringScopeType;
  scopeId: string;
  weight: string;
  enabled: boolean;
}

export interface AssignmentTargetOption {
  id: string;
  name: string;
}

interface SimpleAssignmentDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Whether we're editing (scope already chosen) or creating. */
  isEditing: boolean;
  title: string;
  description: string;
  /** Label for the target select, e.g. "Data source" or "Metric set". */
  targetLabel: string;
  targetOptions: AssignmentTargetOption[];
  initial: AssignmentDraft;
  entities: ScopeEntities;
  pending: boolean;
  onSubmit: (draft: AssignmentDraft) => void;
}

/**
 * Shared create/edit dialog for the two simple assignment kinds (query-source
 * and metric-set). Both bind a target to a scope with a weight + enabled flag.
 */
const SimpleAssignmentDialog = ({
  open,
  onOpenChange,
  isEditing,
  title,
  description,
  targetLabel,
  targetOptions,
  initial,
  entities,
  pending,
  onSubmit,
}: SimpleAssignmentDialogProps) => {
  const { t } = useTranslation();
  const targetFieldId = useId();
  const weightFieldId = useId();
  const [draft, setDraft] = useState<AssignmentDraft>(initial);

  // `initial` is rebuilt every render by the parent, so we key the render-phase
  // reseed on its content signature (not identity) plus open state — reopening
  // or switching the edited assignment rebuilds the draft without an effect.
  const formKey = `${open ? "open" : "closed"}:${initial.targetId}:${initial.scopeType}:${initial.scopeId}:${initial.weight}:${initial.enabled}`;
  const [prevFormKey, setPrevFormKey] = useState(formKey);
  if (prevFormKey !== formKey) {
    setPrevFormKey(formKey);
    setDraft(initial);
  }

  const set = <K extends keyof AssignmentDraft>(key: K, value: AssignmentDraft[K]) =>
    { if (!pending) setDraft((prev) => ({ ...prev, [key]: value })); };

  const canSubmit =
    !!draft.targetId && (draft.scopeType === "global" || !!draft.scopeId) && !pending;

  return (
    <EntityEditorHost
      open={open}
      onOpenChange={(next) => { if (!pending) onOpenChange(next); }}
      dirty={JSON.stringify(draft) !== JSON.stringify(initial)}
      title={title}
      description={description}
      footer={
        <>
          <Button
            data-editor-cancel
            variant="outline"
            disabled={pending}
            onClick={() => onOpenChange(false)}
          >
            {t("cancel", { defaultValue: "Cancel" })}
          </Button>
          <Button disabled={!canSubmit} onClick={() => { if (canSubmit) onSubmit(draft); }}>
            {isEditing ? t("save", { defaultValue: "Save" }) : t("create", { defaultValue: "Create" })}
          </Button>
        </>
      }
    >
      <fieldset disabled={pending} className="grid min-w-0 gap-4">
        <FormField htmlFor={targetFieldId} label={targetLabel} required>
          <EntitySelect
            id={targetFieldId}
            value={draft.targetId}
            onValueChange={(v) => set("targetId", v)}
            disabled={isEditing || pending}
            placeholder={t("observability.assign.pickTarget", { defaultValue: "Choose…" })}
            searchPlaceholder={t("search")}
            items={targetOptions.map((option) => ({ id: option.id, label: option.name }))}
          />
        </FormField>

        <ScopeFields
          disabled={pending}
          scopeType={draft.scopeType}
          scopeId={draft.scopeId}
          onScopeTypeChange={(v) => set("scopeType", v)}
          onScopeIdChange={(v) => set("scopeId", v)}
          entities={entities}
        />

        <div className="grid gap-4 sm:grid-cols-2">
          <FormField htmlFor={weightFieldId} label={t("observability.assign.weight", { defaultValue: "Weight" })}>
            <Input
              id={weightFieldId}
              type="number"
              className="text-center font-mono"
              value={draft.weight}
              onChange={(e) => set("weight", e.target.value)}
            />
          </FormField>
          <label className="flex items-center justify-between gap-4 py-2">
            <span className="text-sm font-medium">
              {t("observability.assign.enabled", { defaultValue: "Enabled" })}
            </span>
            <Switch checked={draft.enabled} onCheckedChange={(v) => set("enabled", v)} />
          </label>
        </div>
      </fieldset>
    </EntityEditorHost>
  );
};

export default SimpleAssignmentDialog;
