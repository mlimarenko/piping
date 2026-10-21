import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Check, ChevronLeft, ChevronRight, Eye, Play, Loader2, Variable as VarIcon } from "lucide-react";

import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import EntityPicker, { type EntityPickerOption } from "@/shared/forms/EntityPicker";
import EntityCreateEditor from "@/shared/forms/EntityCreateEditor";
import { relatedEntitiesForEntity } from "@/shared/forms/entityRelatedEntities";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";

import FieldLabel from "@/shared/ui-shared/FieldLabel";
import { templateVariablePresentation } from "./templateSchema";
import TemplatePreviewRows from "./TemplatePreviewRows";
import {
  buildTemplateVariableValues,
  formatTemplateValue,
  getTemplateVariableOptions,
  templateReferenceFor,
  type InventoryTemplate,
  type PreviewRow,
  type TemplateReference,
  type TemplateSelection,
  isTemplateSelection as isSelection,
  isTemplateReferenceId,
} from "./templateTypes";
import { createResource, listAllResources, previewTemplateRun, type TemplatePreviewStep } from "@/features/inventory/api";
import { inventoryResourceOption, type InventoryResourceOption } from "@/features/inventory/resourceOptions";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  template: InventoryTemplate | null;
  onRun: (variables: Record<string, unknown>) => Promise<unknown> | void;
}

type Stage = "variables" | "preview";
type PreviewRowLike = PreviewRow | TemplatePreviewStep;

const stringRule = (rules: Record<string, unknown>, keys: string[]): string | null => {
  for (const key of keys) {
    const value = rules[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return null;
};

const stringArrayRule = (rules: Record<string, unknown>, keys: string[]): string[] => {
  for (const key of keys) {
    const value = rules[key];
    if (Array.isArray(value)) {
      return value
        .filter((item): item is string => typeof item === "string")
        .map((item) => item.trim())
        .filter(Boolean);
    }
  }
  return [];
};

const resourceRefDefaultFor = (
  variable: InventoryTemplate["variables"][number],
): { id: string | null; names: string[] } | null => {
  const rules = variable.validation_rules;
  if (!rules || typeof rules !== "object" || Array.isArray(rules)) return null;
  const record = rules;
  const id = stringRule(record, ["default_id", "defaultId", "default_value_id", "defaultValueId"]);
  const names = [
    ...stringArrayRule(record, ["default_names", "defaultNames", "default_labels", "defaultLabels"]),
    stringRule(record, ["default_name", "defaultName", "default_label", "defaultLabel"]),
  ].filter((name): name is string => Boolean(name));
  return id || names.length > 0 ? { id, names } : null;
};

const resolveResourceRefDefault = (
  options: InventoryResourceOption[],
  defaultConfig: { id: string | null; names: string[] },
): string | null => {
  // An explicit identity never falls through to a different object's name.
  if (defaultConfig.id) return defaultConfig.id;
  const matches = options.filter((option) =>
    defaultConfig.names.some((name) => option.name === name || option.slug === name),
  );
  return matches.length === 1 ? matches[0]?.id ?? null : null;
};

const textValue = (value: unknown): string => typeof value === "string" ? value : "";

const uniqueEntities = (entities: Array<InventoryEntityKey | null | undefined>): InventoryEntityKey[] =>
  Array.from(new Set(entities.filter((entity): entity is InventoryEntityKey => Boolean(entity))));

const TemplateRunDialog = ({ open, onOpenChange, template, onRun }: Props) => {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [stage, setStage] = useState<Stage>("variables");
  const [rawValues, setRawValues] = useState<Record<string, unknown>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [serverPreview, setServerPreview] = useState<TemplatePreviewStep[] | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState(false);
  const submittingRef = useRef(false);
  const [createTarget, setCreateTarget] = useState<{
    entity: InventoryEntityKey;
    variableKey: string;
  } | null>(null);
  const [createRelatedEntities, setCreateRelatedEntities] = useState<InventoryEntityKey[]>([]);
  // Incremented each time a new preview request is dispatched; stale
  // resolutions (superseded or from a previous template) are discarded.
  const previewReqIdRef = useRef(0);
  const appliedResourceDefaultsRef = useRef<Set<string>>(new Set());
  const createOriginRef = useRef<string | null>(null);
  const referenceInputRefs = useRef(new Map<string, HTMLDivElement>());

  // Seed the run-wizard inputs when the dialog opens (open-event sync; the
  // dialog stays mounted across open/close so Radix can animate the exit).
  useEffect(() => {
    if (!open || !template) return;
    const initialValues: Record<string, unknown> = {};
    for (const variable of template.variables) {
      initialValues[variable.key] = variable.value_type !== "json" && isSelection(variable.default_value)
        ? variable.default_value : formatTemplateValue(variable.default_value);
    }
    // eslint-disable-next-line react-hooks/set-state-in-effect -- dialog stays mounted for the Radix exit animation, so each open re-seeds the wizard in an effect; a key remount would kill the close animation
    setRawValues(initialValues);
    setErrors({});
    setStage("variables");
    setServerPreview(null);
    setPreviewError(false);
    setSubmitError(false);
    setCreateTarget(null);
    setCreateRelatedEntities([]);
    appliedResourceDefaultsRef.current = new Set();
    // Invalidate any in-flight preview for the previous template.
    previewReqIdRef.current += 1;
  }, [open, template]);

  const parsedVariables = useMemo(() => {
    if (!template) return { values: {}, errors: {} };
    return buildTemplateVariableValues(template.variables, rawValues);
  }, [template, rawValues]);

  const referenceVariables = useMemo(
    () =>
      (template?.variables ?? [])
        .map((variable) => ({
          variable,
          reference: template ? templateReferenceFor(template, variable) : null,
        }))
        .filter(
          (item): item is { variable: InventoryTemplate["variables"][number]; reference: TemplateReference } =>
            item.reference !== null,
        ),
    [template],
  );
  const referenceByVariable = useMemo(
    () => new Map(referenceVariables.map(({ variable, reference }) => [variable.key, reference])),
    [referenceVariables],
  );
  const optionEntities = useMemo(
    () =>
      uniqueEntities([
        ...referenceVariables.map(({ reference }) => reference.entity),
        createTarget?.entity,
        ...createRelatedEntities,
      ]),
    [createRelatedEntities, createTarget?.entity, referenceVariables],
  );
  const optionQueries = useQueries({
    queries: optionEntities.map((entity) => ({
      queryKey: ["inventory", "template-run-options", entity],
      queryFn: () => listAllResources(entity),
      retry: false,
      enabled: open,
      staleTime: 30_000,
    })),
  });
  const optionStateByEntity = useMemo(() => {
    const state = new Map<InventoryEntityKey, {
      options: InventoryResourceOption[];
      loading: boolean;
      /** Options loaded: an unmatched stored value can be judged and resolved. */
      ready: boolean;
      error: boolean;
      retry: () => void;
    }>();
    optionEntities.forEach((entity, index) => {
      const query = optionQueries[index];
      state.set(entity, {
        options: (query?.data ?? []).map((resource) => inventoryResourceOption(entity, resource)),
        loading: query?.isFetching ?? false,
        ready: query?.isSuccess ?? false,
        error: query?.isError ?? false,
        retry: () => { void query?.refetch(); },
      });
    });
    return state;
  }, [optionEntities, optionQueries]);

  // Only ID-bound textual defaults are resolved, by a unique exact name/slug.
  // Name-bound strings remain manual input; picks retain identity even if lists change.
  useEffect(() => {
    if (!open || !template) return;
    const pendingDefaults = referenceVariables.filter(({ variable, reference }) =>
      !appliedResourceDefaultsRef.current.has(variable.key)
      && optionStateByEntity.get(reference.entity)?.ready,
    );
    if (!pendingDefaults.length) return;
    for (const { variable } of pendingDefaults) appliedResourceDefaultsRef.current.add(variable.key);
    setRawValues((current) => {
      const nextValues: Record<string, unknown> = {};
      for (const { variable, reference } of pendingDefaults) {
        const currentValue = current[variable.key];
        if (isSelection(currentValue)) continue;
        if (reference.value === "name" && textValue(currentValue)) continue;
        const state = optionStateByEntity.get(reference.entity);
        if (!state?.ready) continue;
        const config = resourceRefDefaultFor(variable);
        const value = textValue(currentValue);
        const id = isTemplateReferenceId(value) ? value : resolveResourceRefDefault(state.options, config ?? {
          id: null,
          names: value ? [value] : [],
        });
        nextValues[variable.key] = id ? { resource: reference.resource, id } satisfies TemplateSelection : "";
      }
      return Object.keys(nextValues).length ? { ...current, ...nextValues } : current;
    });
  }, [open, optionStateByEntity, referenceVariables, template]);

  if (!template) return null;

  const validationMessage = (errorCode: string) => {
    switch (errorCode) {
      case "invalid_integer":
      case "invalid_number":
        return t("inventory.templates.builder.mustBeNumber");
      case "invalid_boolean":
        return t("inventory.templates.builder.mustBeBoolean");
      case "invalid_json":
        return t("inventory.templates.builder.mustBeJson");
      case "required":
        return t("required");
      case "invalid_reference":
        return t("inventory.templates.run.invalidReference");
      default:
        return t("unknownError");
    }
  };

  const validate = () => {
    const nextErrors = Object.fromEntries(
      Object.entries(parsedVariables.errors).map(([key, code]) => [key, validationMessage(code)]),
    );
    setErrors(nextErrors);
    const first = Object.keys(nextErrors)[0];
    if (first) document.getElementById(`run-field-${first}`)?.querySelector<HTMLElement>('input, textarea, [role="combobox"]')?.focus();
    return Object.keys(nextErrors).length === 0;
  };

  const goPreview = () => {
    if (!validate()) return;
    setStage("preview");
    // Only a successful server preview authorizes execution.
    const reqId = ++previewReqIdRef.current;
    setPreviewLoading(true);
    setServerPreview(null);
    setPreviewError(false);
    setSubmitError(false);
    previewTemplateRun(template.id, parsedVariables.values)
      .then((result) => {
        if (reqId !== previewReqIdRef.current) return;
        setServerPreview(result.steps);
      })
      .catch(() => {
        if (reqId !== previewReqIdRef.current) return;
        setServerPreview(null);
        setPreviewError(true);
      })
      .finally(() => {
        if (reqId !== previewReqIdRef.current) return;
        setPreviewLoading(false);
      });
  };

  const handleRun = async () => {
    if (submittingRef.current || stage !== "preview" || previewLoading || previewError || !serverPreview?.length) return;
    if (!validate()) {
      setStage("variables");
      return;
    }
    submittingRef.current = true;
    setSubmitting(true);
    setSubmitError(false);
    try {
      await onRun(parsedVariables.values);
      onOpenChange(false);
    } catch {
      setSubmitError(true);
    } finally {
      submittingRef.current = false;
      setSubmitting(false);
    }
  };

  const preview: PreviewRowLike[] = serverPreview ?? [];
  const entityLabel = (entity: InventoryEntityKey): string =>
    t(`inventory.resources.entities.${entity}.one`, { defaultValue: entity });
  const loadEntityOptions = (entity: InventoryEntityKey): EntityPickerOption[] =>
    optionStateByEntity.get(entity)?.options ?? [];
  const isEntityOptionsLoading = (entity: InventoryEntityKey): boolean =>
    optionStateByEntity.get(entity)?.loading ?? false;
  const beginCreateResource = (entity: InventoryEntityKey, variableKey: string) => {
    createOriginRef.current = variableKey;
    setCreateTarget({ entity, variableKey });
    setCreateRelatedEntities(relatedEntitiesForEntity(entity));
  };
  const createEntity = async (
    entity: InventoryEntityKey,
    resource: Resource,
  ): Promise<Resource | null> => {
    try {
      const { id: _omit, ...payload } = resource;
      const created = await createResource(entity, payload);
      queryClient.setQueryData<Resource[]>(["inventory", "template-run-options", entity], (current = []) => [
        ...current.filter((item) => item.id !== created.id), created,
      ]);
      void queryClient.invalidateQueries({ queryKey: ["inventory", "template-run-options", entity] });
      return created;
    } catch {
      notify("error", t("inventory.templates.run.createFailed"));
      return null;
    }
  };
  const handleRootCreated = (created: Resource) => {
    if (!createTarget) return;
    const reference = referenceByVariable.get(createTarget.variableKey);
    if (!reference) return;
    const value: TemplateSelection = { resource: reference.resource, id: created.id };
    appliedResourceDefaultsRef.current.add(createTarget.variableKey);
    setRawValues((current) => ({ ...current, [createTarget.variableKey]: value }));
    setCreateTarget(null);
    setCreateRelatedEntities([]);
  };

  const renderReferenceInput = (
    variable: InventoryTemplate["variables"][number],
    reference: TemplateReference,
  ) => {
    const label = templateVariablePresentation(template, variable, t).label || entityLabel(reference.entity);
    const state = optionStateByEntity.get(reference.entity);
    const options = loadEntityOptions(reference.entity);
    const value = rawValues[variable.key];
    const selectedId = isSelection(value) ? value.id : "";
    const retained = selectedId && !options.some((option) => option.id === selectedId)
      ? [{ id: selectedId, name: selectedId }] : [];
    return (
      <div className="grid gap-2" role="group" aria-label={label} aria-describedby={errors[variable.key] ? `run-error-${variable.key}` : undefined} ref={(node) => {
        if (node) referenceInputRefs.current.set(variable.key, node);
        else referenceInputRefs.current.delete(variable.key);
      }}>
        <EntityPicker
          id={`run-ref-${variable.key}`}
          aria-invalid={Boolean(errors[variable.key])}
          aria-describedby={[templateVariablePresentation(template, variable, t).help ? `run-help-${variable.key}` : "", errors[variable.key] ? `run-error-${variable.key}` : ""].filter(Boolean).join(" ") || undefined}
          entity={reference.entity}
          value={selectedId}
          onChange={(id) => {
            appliedResourceDefaultsRef.current.add(variable.key);
            setRawValues((current) => ({ ...current, [variable.key]: { resource: reference.resource, id } satisfies TemplateSelection }));
          }}
          options={[...retained, ...options]}
          loading={isEntityOptionsLoading(reference.entity)}
          placeholder={t("inventory.resources.form.relationPlaceholder", { label })}
          label={label}
          onRequestCreate={(entity) => beginCreateResource(entity, variable.key)}
        />
        {state?.loading && <p role="status" className="text-xs text-muted-foreground">{t("inventory.templates.run.optionsLoading")}</p>}
        {state?.error && <div className="flex flex-wrap items-center gap-2"><p role="alert" className="text-xs text-status-negative">{t("inventory.templates.run.optionsError")}</p><Button size="sm" variant="outline" onClick={state.retry}>{t("inventory.templates.run.retry")}</Button></div>}
        {state?.ready && !state.options.length && <p role="status" className="text-xs text-muted-foreground">{t("inventory.templates.run.optionsEmpty")}</p>}
        {reference.value === "name" && (
          <Input
            id={`run-var-${variable.key}`}
            aria-label={label}
            aria-invalid={Boolean(errors[variable.key])}
            aria-describedby={[templateVariablePresentation(template, variable, t).help ? `run-help-${variable.key}` : "", errors[variable.key] ? `run-error-${variable.key}` : ""].filter(Boolean).join(" ") || undefined}
            value={textValue(value)}
            placeholder={t("inventory.templates.run.manualReference")}
            onChange={(event) => {
              appliedResourceDefaultsRef.current.add(variable.key);
              setRawValues((current) => ({ ...current, [variable.key]: event.target.value }));
            }}
          />
        )}
      </div>
    );
  };

  const renderInput = (variable: InventoryTemplate["variables"][number]) => {
    const options = getTemplateVariableOptions(variable);
    const value = textValue(rawValues[variable.key]);
    const id = `run-var-${variable.key}`;
    const reference = referenceByVariable.get(variable.key);
    const accessibility = {
      "aria-label": templateVariablePresentation(template, variable, t).label,
      "aria-invalid": Boolean(errors[variable.key]),
      "aria-describedby": [templateVariablePresentation(template, variable, t).help ? `run-help-${variable.key}` : "", errors[variable.key] ? `run-error-${variable.key}` : ""].filter(Boolean).join(" ") || undefined,
    };

    if (reference) {
      return renderReferenceInput(variable, reference);
    }

    if (options.length > 0) {
      return (
        <FormSelect
          {...accessibility}
          id={id}
          value={value}
          onValueChange={(next) => setRawValues((c) => ({ ...c, [variable.key]: next }))}
          placeholder={t("inventory.resources.form.noneOption")}
          options={options.map((option) => ({ value: option, label: option }))}
        />
      );
    }
    if (variable.value_type === "boolean") {
      return (
        <FormSelect
          {...accessibility}
          id={id}
          value={value}
          onValueChange={(next) => setRawValues((c) => ({ ...c, [variable.key]: next }))}
          placeholder={t("inventory.resources.form.noneOption")}
          options={[
            { value: "true", label: t("yes") },
            { value: "false", label: t("no") },
          ]}
        />
      );
    }
    if (variable.value_type === "json") {
      return (
        <Textarea
          {...accessibility}
          id={id}
          value={value}
          onChange={(e) => setRawValues((c) => ({ ...c, [variable.key]: e.target.value }))}
          placeholder='{"site":"edge-1"}'
          className="min-h-[120px] resize-y font-mono text-sm"
        />
      );
    }
    return (
      <Input
        {...accessibility}
        id={id}
        type={variable.value_type === "integer" || variable.value_type === "number" ? "number" : "text"}
        value={value}
        onChange={(e) => setRawValues((c) => ({ ...c, [variable.key]: e.target.value }))}
        className="h-9 text-sm"
      />
    );
  };

  return (
    <>
      {/* A page editor must not sit behind a modal's inert boundary/focus trap.
          Suspend only the Dialog subtree; wizard values and stage stay owned here. */}
      {!createTarget && <Dialog open={open} onOpenChange={(next) => { if (!submittingRef.current) onOpenChange(next); }}>
        <DialogContent className="surface sm:max-w-2xl max-h-[85dvh] overflow-hidden flex flex-col gap-0 p-0 sm:p-0"
          onCloseAutoFocus={(event) => {
            if (createOriginRef.current) event.preventDefault();
          }}
          onOpenAutoFocus={(event) => {
            const origin = createOriginRef.current;
            if (!origin) return;
            const picker = referenceInputRefs.current.get(origin)?.querySelector<HTMLElement>('[role="combobox"]');
            if (picker) { event.preventDefault(); picker.focus(); }
            createOriginRef.current = null;
          }}>
        <DialogHeader className="shrink-0 border-b border-border px-6 py-4">
          <DialogTitle className="flex items-center gap-2">
            <Play className="h-4 w-4 text-status-positive" />
            {t("inventory.templates.run.title")}: "{template.name}"
          </DialogTitle>
          <DialogDescription>{template.description || t("inventory.templates.description")}</DialogDescription>
        </DialogHeader>

        {/* Stepper */}
        <div className="flex shrink-0 items-center gap-2 px-6 pt-3 text-xs">
          <div className={cn("flex items-center gap-1.5 rounded-md px-2 py-1", stage === "variables" ? "surface-chip text-foreground" : "text-muted-foreground")}>
            <VarIcon className="h-3 w-3" /> 1. {t("inventory.templates.run.variablesHeading")}
          </div>
          <ChevronRight className="h-3 w-3 text-muted-foreground" />
          <div className={cn("flex items-center gap-1.5 rounded-md px-2 py-1", stage === "preview" ? "surface-chip text-foreground" : "text-muted-foreground")}>
            <Eye className="h-3 w-3" /> 2. {t("inventory.templates.run.previewHeading")}
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-6 py-3">
          {stage === "variables" && (
            <div className="flex flex-col gap-4">
              <p className="text-xs leading-relaxed text-muted-foreground text-balance">
                {t("inventory.templates.run.hint")}
              </p>
              {template.variables.length === 0 && (
                <div className="surface-inset rounded-md p-4 text-center text-xs text-muted-foreground">
                  {t("inventory.templates.run.noVariablesNeeded")}
                </div>
              )}
              {(["creates", "references", "parameters"] as const).map((group) => {
                const variables = template.variables.filter((variable) => templateVariablePresentation(template, variable, t).group === group);
                return variables.length ? <section key={group} className="flex flex-col gap-3" aria-label={t(`inventory.templates.run.${group}`)}>
                  <h3 className="text-sm font-semibold">{t(`inventory.templates.run.${group}`)}</h3>
                  {variables.map((variable) => {
                    const presentation = templateVariablePresentation(template, variable, t);
                    return <div key={variable.key} id={`run-field-${variable.key}`} className="flex flex-col gap-1.5">
                      <FieldLabel htmlFor={`${referenceByVariable.has(variable.key) ? "run-ref" : "run-var"}-${variable.key}`} required={variable.required}>{presentation.label}</FieldLabel>
                      {renderInput(variable)}
                      {presentation.help && <p id={`run-help-${variable.key}`} className="text-xs text-muted-foreground">{presentation.help}</p>}
                      {errors[variable.key] && <p id={`run-error-${variable.key}`} role="alert" className="text-xs text-status-negative">{errors[variable.key]}</p>}
                    </div>;
                  })}
                </section> : null;
              })}
            </div>
          )}

          {stage === "preview" && (
            <div className="flex flex-col gap-2">
              <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                {previewLoading && <Loader2 className="h-3 w-3 animate-spin" />}
                <span>
                    <strong className="text-foreground">{preview.length}</strong>{" "}
                  {t("inventory.templates.run.willCreate")}
                </span>
                {serverPreview && !previewError && (
                  <Badge variant="outline" className="shrink-0 whitespace-nowrap text-xs text-status-positive">
                    {t("inventory.templates.run.serverResolved")}
                  </Badge>
                )}
              </div>
              {previewError && (
                <div className="flex items-start gap-2 rounded-md border border-status-warning/30 bg-status-warning/5 p-2 text-xs text-status-warning">
                  <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                  <div className="flex flex-col gap-2"><p className="font-medium" role="alert">{t("inventory.templates.run.previewError")}</p><Button size="sm" variant="outline" onClick={goPreview}>{t("inventory.templates.run.retry")}</Button></div>
                </div>
              )}

              {previewLoading && <p role="status" className="surface-inset rounded-md p-4 text-center text-xs text-muted-foreground">{t("loading")}</p>}
              {!previewLoading && !previewError && preview.length === 0 && <p role="status" className="surface-inset rounded-md p-4 text-center text-xs text-muted-foreground">{t("inventory.templates.run.previewEmpty")}</p>}
              <TemplatePreviewRows rows={preview} optionsByEntity={optionStateByEntity} />
              {submitError && <p role="alert" className="text-sm text-status-negative">{t("inventory.templates.run.submitError")}</p>}
              {submitting && <p role="status" className="text-sm text-muted-foreground">{t("inventory.templates.run.submitting")}</p>}
            </div>
          )}
        </div>

        <DialogFooter className="shrink-0 gap-2 border-t border-border px-6 py-4">
          {stage === "preview" && (
            <Button variant="outline" size="sm" disabled={submitting} onClick={() => { previewReqIdRef.current += 1; setPreviewLoading(false); setServerPreview(null); setStage("variables"); }} className="gap-1">
              <ChevronLeft className="h-3 w-3" /> {t("back")}
            </Button>
          )}
          <Button variant="outline" size="sm" disabled={submitting} onClick={() => onOpenChange(false)}>{t("cancel")}</Button>
          {stage === "variables" ? (
            <Button size="sm" onClick={goPreview} className="gap-1">
              {t("inventory.templates.run.previewHeading")} <ChevronRight className="h-3 w-3" />
            </Button>
          ) : (
            <Button size="sm" onClick={() => { void handleRun(); }} disabled={submitting || previewLoading || previewError || !serverPreview?.length} className="gap-1">
              <Check className="h-3 w-3" /> {t("inventory.templates.run.runNow")} ({preview.length})
            </Button>
          )}
        </DialogFooter>
        </DialogContent>
      </Dialog>}
      {createTarget && (
        <EntityCreateEditor
          open={Boolean(createTarget)}
          onOpenChange={(nextOpen) => {
            if (!nextOpen) {
              setCreateTarget(null);
              setCreateRelatedEntities([]);
            }
          }}
          rootEntity={createTarget.entity}
          createEntity={createEntity}
          loadOptions={loadEntityOptions}
          onVisibleRelatedEntitiesChange={setCreateRelatedEntities}
          isOptionsLoading={isEntityOptionsLoading}
          onRootCreated={handleRootCreated}
        />
      )}
    </>
  );
};

export default TemplateRunDialog;
