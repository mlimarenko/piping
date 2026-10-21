import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { Pencil, Plus } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { listResources } from "@/features/inventory/api";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Switch } from "@/shared/ui-primitives/switch";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FormField from "@/shared/ui-shared/FormField";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import FormSelect from "@/shared/ui-shared/FormSelect";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter,
} from "@/shared/ui-primitives/dialog";
import {
  CONNCONF_SCOPE_TYPES,
  createAssignment,
  updateAssignment,
  type ConnConfAssignment,
  type ConnConfScopeType,
} from "./api";
import { SCOPE_ENTITY, SCOPE_I18N_KEY } from "./scopeCatalog";

/**
 * Add/edit an assignment in a focused modal so it never competes with the
 * record editor. On create the scope/target/weight are captured; on edit the
 * backend PUT accepts a scope retarget alongside weight/enabled, so every
 * control is editable and prefilled from `assignment`. All create/edit
 * mutations, scope validation and cache invalidation live here.
 */
const AssignmentDialog = ({
  open,
  onOpenChange,
  recordId,
  assignment,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  recordId: string;
  /** Present in edit mode; null/undefined for create. */
  assignment?: ConnConfAssignment | null;
}) => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const isEditing = !!assignment;

  const [scopeType, setScopeType] = useState<ConnConfScopeType>("region");
  const [scopeId, setScopeId] = useState("");
  const [weight, setWeight] = useState(0);
  const [enabled, setEnabled] = useState(true);

  // Reset the form to a clean draft (create) or the assignment (edit) each
  // time the dialog opens or its target changes, so a previous session never
  // bleeds through. Done during render via a sentinel (React's recommended
  // pattern) rather than in an effect, so the form never flashes stale values.
  const sentinel = open ? (assignment?.id ?? "__create__") : "__closed__";
  const [prevSentinel, setPrevSentinel] = useState(sentinel);
  if (prevSentinel !== sentinel) {
    setPrevSentinel(sentinel);
    if (assignment) {
      setScopeType(assignment.scope_type as ConnConfScopeType);
      setScopeId(assignment.scope_id ?? "");
      setWeight(assignment.weight);
      setEnabled(assignment.enabled);
    } else {
      setScopeType("region");
      setScopeId("");
      setWeight(0);
      setEnabled(true);
    }
  }

  // Eagerly load every scope entity so targets can be offered in the picker.
  // Each entity is a small lookup list, cached for a minute.
  const targetableScopes = CONNCONF_SCOPE_TYPES.filter((s) => s !== "global");
  const targetQueries = useQueries({
    queries: targetableScopes.map((scope) => ({
      queryKey: ["inventory", "connconf-scope", SCOPE_ENTITY[scope]],
      queryFn: () => listResources(SCOPE_ENTITY[scope], { limit: 1000, offset: 0 }),
      staleTime: 60_000,
      enabled: open,
    })),
  });
  const optionsByScope = useMemo(() => {
    const map = {} as Record<Exclude<ConnConfScopeType, "global">, { id: string; name: string }[]>;
    targetableScopes.forEach((scope, index) => {
      map[scope] = (targetQueries[index]?.data?.items ?? []).map((r) => ({
        id: r.id,
        name: r.name,
      }));
    });
    return map;
  }, [targetableScopes, targetQueries]);

  const invalidate = () =>
    qc.invalidateQueries({ queryKey: ["connection-config", "assignments", recordId] });

  const createMutation = useApiMutation({
    mutationFn: () =>
      createAssignment({
        record_id: recordId,
        scope_type: scopeType,
        scope_id: scopeType === "global" ? null : scopeId,
        weight,
      }),
    onSuccess: () => {
      void invalidate();
      onOpenChange(false);
      notify("success", t("connectionConfig.assignments.added"));
    },
    onError: (err: ApiError) =>
      notify("error", err.message || t("connectionConfig.assignments.addFailed")),
  });

  const editMutation = useApiMutation({
    mutationFn: (id: string) =>
      updateAssignment(id, {
        scope_type: scopeType,
        scope_id: scopeType === "global" ? null : scopeId,
        weight,
        enabled,
      }),
    onSuccess: () => {
      void invalidate();
      onOpenChange(false);
      notify("success", t("connectionConfig.assignments.updated"));
    },
    onError: (err: ApiError) =>
      notify("error", err.message || t("connectionConfig.assignments.addFailed")),
  });

  const canSave = scopeType === "global" || !!scopeId;
  const saving = createMutation.isPending || editMutation.isPending;

  const submit = () => {
    if (!canSave || saving) return;
    if (isEditing && assignment) editMutation.mutate(assignment.id);
    else createMutation.mutate();
  };

  return (
    <Dialog open={open} onOpenChange={next => { if (!saving) onOpenChange(next); }}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-[800px]">
        <DialogHeader>
          <DialogTitle className="text-xl">
            {isEditing
              ? t("connectionConfig.assignments.editHeading")
              : t("connectionConfig.assignments.addHeading")}
          </DialogTitle>
          <DialogDescription className="text-xs">
            {t("connectionConfig.assignments.whatIsHint")}
          </DialogDescription>
        </DialogHeader>

        <fieldset disabled={saving} inert={saving} className="flex min-w-0 flex-col gap-4" aria-busy={saving}>
          <FormSelect
            value={scopeType}
            onValueChange={(value) => {
              setScopeType(value);
              setScopeId("");
            }}
            label={t("connectionConfig.assignments.scopeType")}
            options={CONNCONF_SCOPE_TYPES.map((scope) => ({
              value: scope,
              label: t(`connectionConfig.scope.${SCOPE_I18N_KEY[scope]}`),
            }))}
          />

          <FormField
            label={t("connectionConfig.assignments.target")}
            htmlFor="connection-assignment-target"
            required={scopeType !== "global"}
          >
            {scopeType === "global" ? (
              <Input
                id="connection-assignment-target"
                className="h-9 text-sm"
                value={t("connectionConfig.scope.appliesAll")}
                disabled
              />
            ) : (
              <EntitySelect
                id="connection-assignment-target"
                aria-label={t("connectionConfig.assignments.target")}
                value={scopeId}
                onValueChange={setScopeId}
                triggerClassName="h-9 text-sm"
                placeholder={t("connectionConfig.assignments.pickTarget")}
                searchPlaceholder={t("search")}
                items={(optionsByScope[scopeType] ?? []).map((option) => ({
                  id: option.id,
                  label: option.name,
                }))}
              />
            )}
          </FormField>

          <div className="flex items-end gap-4">
            <FormField
              className="w-24"
              htmlFor="connection-assignment-weight"
              label={t("connectionConfig.assignments.weight")}
            >
              <Input
                id="connection-assignment-weight"
                type="number"
                aria-label={t("connectionConfig.assignments.weight")}
                className="h-9 text-sm text-center font-mono"
                value={weight}
                onChange={(event) => setWeight(Number(event.target.value) || 0)}
              />
            </FormField>
            {isEditing && (
              <div className="flex items-center gap-2 pb-1.5">
                <Switch
                  id="connection-assignment-enabled"
                  checked={enabled}
                  onCheckedChange={setEnabled}
                  aria-label={t("connectionConfig.assignments.enabled")}
                />
                <FieldLabel htmlFor="connection-assignment-enabled">
                  {t("connectionConfig.assignments.enabled")}
                </FieldLabel>
              </div>
            )}
          </div>
        </fieldset>

        <DialogFooter>
          <Button variant="outline" size="sm" disabled={saving} onClick={() => onOpenChange(false)}>
            {t("cancel")}
          </Button>
          <Button
            size="sm"
            className="gap-1"
            disabled={!canSave || saving}
            onClick={submit}
          >
            {isEditing ? <Pencil className="h-3 w-3" /> : <Plus className="h-3 w-3" />}
            {isEditing
              ? t("saveChanges")
              : t("connectionConfig.assignments.confirmAdd")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default AssignmentDialog;
