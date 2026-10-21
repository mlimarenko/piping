import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Plus,
  Trash2,
  Variable as VarIcon,
  Layers,
  Settings2,
  AlertCircle,
  CheckCircle2,
} from "lucide-react";

import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Switch } from "@/shared/ui-primitives/switch";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { Badge } from "@/shared/ui-primitives/badge";
import FormSelect from "@/shared/ui-shared/FormSelect";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/shared/ui-primitives/collapsible";
import { ChevronDown } from "lucide-react";
import { cn } from "@/shared/lib/utils";

import {
  autoBindStepReferences,
  clearStepReferences,
  createEmptyTemplateVariable,
  createTemplateStep,
  formatTemplateValue,
  renameStepReferences,
  formatTemplateVariableDefault,
  parseConfiguredValue,
  parseTemplateVariableDefault,
  isTemplateSelection,
  validateTemplate,
  TEMPLATE_CONFLICT_POLICIES,
  TEMPLATE_REFERENCE_ENTITIES,
  TEMPLATE_RESOURCE_DEFINITION_MAP,
  TEMPLATE_RESOURCE_DEFINITIONS,
  TEMPLATE_VARIABLE_VALUE_TYPES,
  templateReferenceFor,
  hasTemplateVariableConsumers,
  type InventoryTemplate,
  type TemplateIssue,
  type TemplateStep,
  type TemplateVariable,
} from "./templateTypes";
import { FieldHelp, TokenMenu } from "./templateEditorShared";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import FormSection from "@/shared/ui-shared/FormSection";
import {
  useConflictPolicyHelp,
  useValueTypeHelp,
  type TokenOption,
} from "./templateEditorHelp";
import TemplatePreviewRail from "./TemplatePreviewRail";

type SectionId = "meta" | "variables" | "steps";

interface Props {
  /** The starting template; `null` for a fresh draft. */
  initial: InventoryTemplate | null;
  saving: boolean;
  canSave: boolean;
  onSave: (template: InventoryTemplate) => void;
  onCancel: () => void;
}

const emptyTemplate = (): InventoryTemplate => ({
  id: "",
  name: "",
  slug: "",
  status: "draft",
  description: "",
  variables: [],
  steps: [],
  updatedAt: "",
});

/**
 * Two-column guided authoring editor for inventory templates.
 *
 * Left: three anchored sections (Meta → Variables → Steps) with live issue
 * badges. Right: a preview rail showing the resolved payloads as you type.
 */
const TemplateEditor = ({ initial, saving, canSave, onSave, onCancel }: Props) => {
  const { t } = useTranslation();
  const [template, setTemplate] = useState<InventoryTemplate>(
    () => (initial ? structuredClone(initial) : emptyTemplate()),
  );
  const [submitted, setSubmitted] = useState(false);
  // Representation is edit-session intent, not inferred from draft text: a
  // malformed intermediate edit must not switch an object default into a string.
  const [structuredDefaults, setStructuredDefaults] = useState<boolean[]>(
    () => initial?.variables.map((variable) => variable.value_type !== "json" && isTemplateSelection(variable.default_value)) ?? [],
  );
  // Per-item expand state, aligned by position to template.variables / .steps.
  // Existing items load collapsed (scannable list); freshly added ones open so
  // the author can fill them in. Mutators below keep these arrays in sync, and
  // an item with submitted issues is force-expanded regardless (see isXOpen).
  const [openVars, setOpenVars] = useState<boolean[]>(
    () => (initial ? initial.variables.map(() => false) : []),
  );
  const [openSteps, setOpenSteps] = useState<boolean[]>(
    () => (initial ? initial.steps.map(() => false) : []),
  );
  const conflictHelp = useConflictPolicyHelp();
  const valueTypeHelp = useValueTypeHelp();

  const scrollRef = useRef<HTMLDivElement>(null);
  const slugTouched = useRef(!!initial);

  const validation = useMemo(() => validateTemplate(template, structuredDefaults), [template, structuredDefaults]);
  const issues = validation.issues;

  // Only surface issues after a save attempt OR for the field the user just
  // left dirty — but keep section counters live so the author has a compass.
  const issuesBySection = useMemo(() => {
    const map: Record<SectionId, TemplateIssue[]> = { meta: [], variables: [], steps: [] };
    for (const issue of issues) map[issue.section].push(issue);
    return map;
  }, [issues]);

  const issueFor = (target: string): TemplateIssue | undefined =>
    submitted ? issues.find((i) => i.target === target) : undefined;

  // Count of *visible* (post-submit) issues whose target belongs to a given
  // item, so a collapsed summary row can flag that it hides errors. Targets are
  // namespaced like `var-<index>-...` / `step-<index>-...`.
  const issueCountFor = (prefix: string): number =>
    submitted ? issues.filter((i) => i.target.startsWith(prefix)).length : 0;
  const variableIssueCount = (index: number): number => issueCountFor(`var-${index}-`);
  const stepIssueCount = (index: number): number => issueCountFor(`step-${index}-`);

  // An item is shown expanded when the user opened it OR it has surfaced issues
  // (errors must never hide behind a collapsed row).
  const isVariableOpen = (index: number): boolean =>
    (openVars[index] ?? false) || variableIssueCount(index) > 0;
  const isStepOpen = (index: number): boolean =>
    (openSteps[index] ?? false) || stepIssueCount(index) > 0;

  const issueMessage = (issue: TemplateIssue): string =>
    t(`inventory.templates.editor.issues.${issue.code}`, issue.params ?? {});

  // Tooltip copy for a step payload field: prepend behavior notes for reference
  // (FK) fields and slug, which auto-resolve, then fall back to the field's own
  // description / placeholder so plain fields keep their existing hint.
  const fieldHelpText = (field: {
    key: string;
    references?: string;
    description?: string;
    placeholder?: string;
  }): string | undefined => {
    const notes: string[] = [];
    if (field.references) notes.push(t("inventory.templates.editor.help_referenceField"));
    if (field.key === "slug") notes.push(t("inventory.templates.editor.help_slugField"));
    if (field.description) notes.push(field.description);
    return notes.length > 0 ? notes.join(" ") : field.placeholder;
  };

  // ── Mutators ────────────────────────────────────────────────────────────

  const patch = (next: Partial<InventoryTemplate>) =>
    setTemplate((current) => ({ ...current, ...next }));

  const onNameChange = (name: string) => {
    if (slugTouched.current) {
      patch({ name });
    } else {
      const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "");
      patch({ name, slug });
    }
  };

  const nextUniqueStepKey = (base: string, ignoreIndex?: number): string => {
    const existing = new Set(
      template.steps.filter((_, i) => i !== ignoreIndex).map((s) => s.step_key),
    );
    if (!existing.has(base)) return base;
    let counter = 2;
    while (existing.has(`${base}_${counter}`)) counter += 1;
    return `${base}_${counter}`;
  };

  // Variables
  const addVariable = () => {
    patch({ variables: [...template.variables, createEmptyTemplateVariable()] });
    setStructuredDefaults((current) => [...template.variables.map((variable, index) => current[index]
      ?? (variable.value_type !== "json" && isTemplateSelection(variable.default_value))), false]);
    setOpenVars((prev) => [...prev, true]); // open the new one for editing
  };
  const updateVariable = (index: number, p: Partial<TemplateVariable>) => {
    const next = [...template.variables];
    const current = next[index];
    if (!current) return;
    next[index] = { ...current, ...p };
    patch({ variables: next });
  };
  const removeVariable = (index: number) => {
    patch({ variables: template.variables.filter((_, i) => i !== index) });
    setStructuredDefaults((current) => template.variables.map((variable, i) => current[i]
      ?? (variable.value_type !== "json" && isTemplateSelection(variable.default_value))).filter((_, i) => i !== index));
    setOpenVars((prev) => prev.filter((_, i) => i !== index));
  };
  const setVariableOpen = (index: number, open: boolean) =>
    setOpenVars((prev) => {
      const next = prev.length === template.variables.length ? [...prev] : template.variables.map((_, i) => prev[i] ?? false);
      next[index] = open;
      return next;
    });
  const updateVariableDefault = (index: number, raw: string) => {
    const valueType = template.variables[index]?.value_type ?? "string";
    const variable = template.variables[index];
    const parsed = parseTemplateVariableDefault(valueType, raw,
      variable ? templateReferenceFor(template, variable)?.resource : undefined, structuredDefaults[index] ?? false);
    if (valueType !== "json" && isTemplateSelection(parsed.value)) {
      setStructuredDefaults((current) => template.variables.map((item, i) => i === index || (current[i]
        ?? (item.value_type !== "json" && isTemplateSelection(item.default_value)))));
    }
    updateVariable(index, { default_value: parsed.value });
  };
  const updateVariableReference = (index: number, resource: string) => {
    const current = template.variables[index];
    if (!current) return;
    updateVariable(index, {
      validation_rules: { ...(current.validation_rules ?? {}), resource },
    });
  };

  // Reference kinds a `resource_ref` variable can point at, labelled with the
  // inventory entity the run wizard picks from.
  const referenceOptions = useMemo(
    () =>
      Object.entries(TEMPLATE_REFERENCE_ENTITIES).map(([resource, entity]) => ({
        value: resource,
        label: t(
          `inventory.resources.entities.${entity}.one`,
          { defaultValue: entity },
        ),
      })),
    [t],
  );

  // Steps
  const addStep = () => {
    const step = createTemplateStep();
    setTemplate((current) => {
      const withStep: InventoryTemplate = {
        ...current,
        steps: [
          ...current.steps,
          { ...step, step_key: nextUniqueStepKey(step.step_key), sort_order: current.steps.length },
        ],
      };
      const bound = autoBindStepReferences(withStep, withStep.steps.length - 1);
      return { ...withStep, steps: bound.steps, variables: bound.variables };
    });
    setOpenSteps((prev) => [...prev, true]); // open the new one for editing
  };
  // General-purpose patch for a single step's fields. The dedicated step-key
  // rename path lives in `renameStep` so reference integrity is maintained.
  const updateStep = (index: number, p: Partial<TemplateStep>) => {
    const next = [...template.steps];
    const current = next[index];
    if (!current) return;
    next[index] = { ...current, ...p };
    patch({ steps: next });
  };
  const renameStep = (index: number, rawKey: string) => {
    setTemplate((current) => {
      const target = current.steps[index];
      if (!target) return current;
      const oldKey = target.step_key;
      const renamed = current.steps.map((s, i) =>
        i === index ? { ...s, step_key: rawKey } : s,
      );
      // Cascade only when the key actually changed to a non-empty value.
      const cascaded =
        rawKey.trim() && rawKey !== oldKey
          ? renameStepReferences(renamed, oldKey, rawKey)
          : renamed;
      return { ...current, steps: cascaded };
    });
  };
  const removeStep = (index: number) => {
    const removedKey = template.steps[index]?.step_key;
    const next = template.steps
      .filter((_, i) => i !== index)
      .map((s) => ({
        ...s,
        depends_on_step_keys: s.depends_on_step_keys.filter((k) => k !== removedKey),
      }));
    // Blank any payload value that still points at the removed step so
    // validation re-flags the now-missing required field.
    patch({ steps: removedKey ? clearStepReferences(next, removedKey) : next });
    setOpenSteps((prev) => prev.filter((_, i) => i !== index));
  };
  const setStepOpen = (index: number, open: boolean) =>
    setOpenSteps((prev) => {
      const next = prev.length === template.steps.length ? [...prev] : template.steps.map((_, i) => prev[i] ?? false);
      next[index] = open;
      return next;
    });
  const replaceStepResource = (index: number, resourceType: TemplateStep["resource_type"]) => {
    const definition = TEMPLATE_RESOURCE_DEFINITION_MAP[resourceType];
    setTemplate((current) => {
      const reset: InventoryTemplate = {
        ...current,
        steps: current.steps.map((s, i) =>
          i === index
            ? {
                ...s,
                resource_type: resourceType,
                title: definition.defaultTitle,
                step_key: nextUniqueStepKey(definition.defaultStepKey, index),
                payload_template: {},
              }
            : s,
        ),
      };
      const bound = autoBindStepReferences(reset, index);
      return { ...reset, steps: bound.steps, variables: bound.variables };
    });
  };
  const toggleDependency = (index: number, depKey: string) => {
    const step = template.steps[index];
    if (!step) return;
    const has = step.depends_on_step_keys.includes(depKey);
    updateStep(index, {
      depends_on_step_keys: has
        ? step.depends_on_step_keys.filter((k) => k !== depKey)
        : [...step.depends_on_step_keys, depKey],
    });
  };
  const updateStepPayloadField = (
    index: number,
    key: string,
    raw: string,
    kind: "string" | "number" | "boolean",
  ) => {
    const step = template.steps[index];
    if (!step) return;
    const nextPayload = { ...step.payload_template };
    const parsed = parseConfiguredValue(raw, kind);
    if (parsed === undefined) delete nextPayload[key];
    else nextPayload[key] = parsed;
    updateStep(index, { payload_template: nextPayload });
  };
  const insertPayloadToken = (index: number, key: string, token: string) => {
    const step = template.steps[index];
    if (!step) return;
    const existing = formatTemplateValue(step.payload_template[key]);
    const nextRaw = existing ? `${existing}${token}` : token;
    updateStepPayloadField(index, key, nextRaw, "string");
  };

  // ── Token catalog for insert-assist menus ────────────────────────────────

  const variableTokens: TokenOption[] = useMemo(
    () =>
      template.variables
        .filter((v) => v.key.trim())
        .map((v) => ({
          token: `{{${v.key}}}`,
          label: v.label || v.key,
          hint: v.value_type,
        })),
    [template.variables],
  );

  const stepRefTokens = (currentIndex: number): TokenOption[] =>
    template.steps
      .filter((s, i) => i < currentIndex && s.step_key.trim())
      .map((s) => ({
        token: `{{steps.${s.step_key}.id}}`,
        label: t("inventory.templates.editor.tokenStepHint", { title: s.title || s.step_key }),
      }));

  // ── Submit ────────────────────────────────────────────────────────────────

  const handleSave = () => {
    if (!canSave) return;
    setSubmitted(true);
    const first = issues[0];
    if (first) {
      // Setting `submitted` force-expands any collapsed item carrying an issue
      // (see isVariableOpen / isStepOpen). Defer scroll+focus one frame so the
      // newly-mounted CollapsibleContent exists before we target its field.
      requestAnimationFrame(() => {
        const el = scrollRef.current?.querySelector<HTMLElement>(`[data-issue="${first.target}"]`);
        if (!el) return;
        el.scrollIntoView({ behavior: "smooth", block: "center" });
        // Focus the actual control, never the FieldHelp tooltip button rendered
        // inside the label (it precedes the input in DOM order). Native inputs +
        // Radix combobox/contenteditable or an explicitly labelled group; a row
        // without a control falls back to the wrapper itself.
        const focusEl =
          el.querySelector<HTMLElement>(
            "input, textarea, select, [role='combobox'], [contenteditable], [role='group']",
          ) ?? el;
        focusEl.focus({ preventScroll: true });
      });
      return;
    }
    onSave({ ...validation.normalized, updatedAt: t("inventory.templates.justNow") });
  };

  // Note: the submitted-issues banner is rendered as `submitted && issues.length > 0`.
  // Because `issues` is derived live on every render, fixing the last issue hides
  // the banner immediately without any reset effect.

  // ── Section header with live issue badge ──────────────────────────────────

  const SectionBadge = ({ section }: { section: SectionId }) => {
    const count = issuesBySection[section].length;
    if (count === 0) {
      return <CheckCircle2 className="h-3.5 w-3.5 text-status-positive/70" aria-hidden />;
    }
    return (
      <Badge variant="outline" className="h-5 gap-1 text-xs text-status-warning">
        <AlertCircle className="h-3 w-3" />
        {count}
      </Badge>
    );
  };

  // Small destructive count shown on a collapsed item's summary row so its
  // hidden validation errors stay visible at a glance.
  const IssuePip = ({ count }: { count: number }) => {
    if (count === 0) return null;
    return (
      <Badge
        variant="outline"
        className="h-5 shrink-0 gap-1 border-status-negative/40 px-1.5 text-xs text-status-negative"
      >
        <AlertCircle className="h-3 w-3" />
        {count}
      </Badge>
    );
  };

  const sections: Array<{ id: SectionId; label: string; icon: typeof VarIcon }> = [
    { id: "meta", label: t("inventory.templates.editor.sectionMeta"), icon: Settings2 },
    { id: "variables", label: t("inventory.templates.builder.variables"), icon: VarIcon },
    { id: "steps", label: t("inventory.templates.builder.steps"), icon: Layers },
  ];

  const goToSection = (id: SectionId) => {
    scrollRef.current
      ?.querySelector(`#section-${id}`)
      ?.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  // ── Render helpers ─────────────────────────────────────────────────────────

  const renderVariableDefault = (variable: TemplateVariable, index: number) => {
    const value = formatTemplateVariableDefault(variable);
    const id = `var-${index}-default-input`;
    const issue = issueFor(`var-${index}-default`);
    const fieldProps = {
      label: <>{t("inventory.templates.builder.variableDefault")}<FieldHelp text={t("inventory.templates.editor.help_variableDefault")} /></>,
      error: issue ? issueMessage(issue) : undefined,
    };
    if (variable.value_type === "boolean") {
      return (
        <FormSelect
          {...fieldProps}
          id={id}
          value={value}
          onValueChange={(next) => updateVariableDefault(index, next)}
          placeholder={t("inventory.resources.form.noneOption")}
          options={[
            { value: "true", label: t("yes") },
            { value: "false", label: t("no") },
          ]}
        />
      );
    }
    return (
      <FormField {...fieldProps} htmlFor={id}>
        <FormFieldControl>
          {variable.value_type === "json" ? (
        <Textarea
          id={id}
          value={value}
          onChange={(e) => updateVariableDefault(index, e.target.value)}
          placeholder='{"site":"edge-1"}'
          className="min-h-[80px] resize-y font-mono text-sm"
        />
          ) : (
      <Input
        id={id}
        type={variable.value_type === "integer" || variable.value_type === "number" ? "number" : "text"}
        value={value}
        onChange={(e) => updateVariableDefault(index, e.target.value)}
        aria-invalid={!!issueFor(`var-${index}-default`)}
        placeholder={t("inventory.templates.builder.variableDefault")}
        className="h-9 font-mono text-sm"
      />
          )}
        </FormFieldControl>
      </FormField>
    );
  };

  return (
    <EntityEditorPage scope="routed"
      width="standard"
      title={template.name.trim() || (initial ? t("inventory.templates.builder.editTitle") : t("inventory.templates.createTemplate"))}
      onBack={onCancel}
      backLabel={t("inventory.templates.editor.backToList")}
      onCancel={onCancel}
      cancelLabel={t("cancel")}
      onSave={canSave ? handleSave : undefined}
      saveLabel={initial ? t("saveChanges") : t("inventory.templates.createTemplate")}
      saving={saving}
      dirty={canSave && JSON.stringify(template) !== JSON.stringify(initial ?? emptyTemplate())}
      error={submitted && issues.length > 0 ? <p className="text-xs text-status-warning">{t("inventory.templates.editor.issueCount", { count: issues.length })}</p> : undefined}
    >

      {/* Two-column workspace */}
      <div className="grid min-w-0 grid-cols-1 gap-6 lg:grid-cols-[minmax(0,1fr)_20rem]">
        {/* Editor column */}
        <div className="flex min-w-0 flex-col gap-4">
          {/* Section anchors */}
          <nav
            className="flex shrink-0 items-center gap-1 overflow-x-auto border-b border-border px-3 py-2"
            aria-label={t("inventory.templates.editor.sections")}
          >
            {sections.map((s) => (
              <Button
                variant="ghost"
                key={s.id}
                type="button"
                onClick={() => goToSection(s.id)}
                className="shrink-0 gap-2 text-sm text-muted-foreground"
              >
                <s.icon className="h-3.5 w-3.5" />
                {s.label}
                <SectionBadge section={s.id} />
              </Button>
            ))}
          </nav>

           <div ref={scrollRef} className="min-w-0">
             <div className="flex min-w-0 flex-col gap-6">
            {/* Friendly intro: what templates are for. */}
             <p className="text-xs leading-relaxed text-muted-foreground text-balance">
              {t("inventory.templates.editor.intro")}
            </p>
            {/* ── META ─────────────────────────────────────────────────── */}
             <div id="section-meta" className="scroll-mt-4">
             <FormSection title={t("inventory.templates.editor.sectionMeta")} contentClassName="flex flex-col gap-4">

              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <div data-issue="meta-name">
                  <FormField
                    label={
                      <>
                        {t("inventory.templates.builder.nameLabel")}
                        <FieldHelp text={t("inventory.templates.editor.help_name")} />
                      </>
                    }
                    htmlFor="meta-name"
                    required
                    error={issueFor("meta-name") ? issueMessage(issueFor("meta-name")!) : undefined}
                  >
                    <FormFieldControl><Input
                      id="meta-name"
                      value={template.name}
                      onChange={(e) => onNameChange(e.target.value)}
                      placeholder={t("inventory.templates.builder.nameHint")}
                      className="h-9 text-sm"
                      aria-invalid={!!issueFor("meta-name")}
                    /></FormFieldControl>
                  </FormField>
                </div>

                <div data-issue="meta-slug">
                  <FormField
                    label={
                      <>
                        {t("inventory.templates.builder.slugLabel")}
                        <FieldHelp text={t("inventory.templates.editor.help_slug")} />
                      </>
                    }
                    htmlFor="meta-slug"
                    required
                    error={issueFor("meta-slug") ? issueMessage(issueFor("meta-slug")!) : undefined}
                  >
                    <FormFieldControl><Input
                      id="meta-slug"
                      value={template.slug}
                      onChange={(e) => {
                        slugTouched.current = true;
                        patch({ slug: e.target.value });
                      }}
                      placeholder={t("inventory.templates.builder.slugHint")}
                      className="h-9 font-mono text-sm"
                      aria-invalid={!!issueFor("meta-slug")}
                    /></FormFieldControl>
                  </FormField>
                </div>
              </div>

              <div className="grid grid-cols-1 gap-3 sm:grid-cols-[10rem_1fr]">
                <div className="space-y-1.5">
                  <FormSelect<InventoryTemplate["status"]>
                    id="meta-status"
                    label={t("inventory.templates.builder.statusLabel")}
                    help={t("inventory.templates.editor.help_status")}
                    value={template.status}
                    onValueChange={(status) => patch({ status })}
                    options={[
                      { value: "draft", label: t("inventory.templates.builder.statusDraft") },
                      { value: "active", label: t("inventory.templates.builder.statusActive") },
                    ]}
                  />
                </div>
                <FormField
                  label={
                    <>
                      {t("description")}
                      <FieldHelp text={t("inventory.templates.editor.help_description")} />
                    </>
                  }
                  htmlFor="meta-description"
                >
                  <FormFieldControl><Textarea
                    id="meta-description"
                    value={template.description}
                    onChange={(e) => patch({ description: e.target.value })}
                    placeholder={t("inventory.templates.builder.descriptionHint")}
                    className="min-h-[80px] resize-y text-sm"
                  /></FormFieldControl>
                </FormField>
              </div>

              <div className="rounded-md border border-border bg-accent/10 p-3 text-xs text-muted-foreground">
                <p className="flex items-center gap-1.5 font-medium text-foreground/80">
                  {t("inventory.templates.editor.placeholderTitle")}
                  <FieldHelp text={t("inventory.templates.editor.help_placeholder")} />
                </p>
                <p className="mt-1 font-mono text-xs text-foreground/70">
                  {"{{site_name}}"} · {"{{steps.site.id}}"}
                </p>
              </div>
             </FormSection></div>

            {/* ── VARIABLES ────────────────────────────────────────────── */}
             <div id="section-variables" className="scroll-mt-4">
             <FormSection divider title={t("inventory.templates.builder.variables")} contentClassName="flex flex-col gap-4" action={<>
               <FieldHelp text={t("inventory.templates.editor.help_variables")} />
               <Badge variant="outline" className="text-xs">{template.variables.length}</Badge>
             </>}>

              {template.variables.length === 0 && (
                <p className="rounded-md border border-dashed border-border px-3 py-6 text-center text-xs text-muted-foreground">
                  {t("inventory.templates.builder.noVariables")}
                </p>
              )}

              <div className="space-y-2">
                {template.variables.map((variable, index) => {
                  const keyIssue = issueFor(`var-${index}-key`);
                  const open = isVariableOpen(index);
                  const issueCount = variableIssueCount(index);
                  const typeLabel =
                    TEMPLATE_VARIABLE_VALUE_TYPES.find((o) => o.value === variable.value_type)?.label ??
                    variable.value_type;
                  const isReference = variable.value_type === "resource_ref";
                  const referenceResource = templateReferenceFor(template, variable)?.resource ?? null;
                  return (
                    <Collapsible
                      key={`var-${index}`}
                      open={open}
                      onOpenChange={(next) => setVariableOpen(index, next)}
                      className={cn(
                        "border-b pb-4 transition-colors",
                        issueCount > 0 ? "border-status-negative/40" : "border-border",
                      )}
                    >
                      {/* Collapsed summary row — the whole row toggles the card. */}
                      <CollapsibleTrigger
                        className="group flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors hover:bg-accent/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring data-[state=open]:rounded-b-none"
                      >
                        <ChevronDown
                          className="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-180"
                          aria-hidden
                        />
                        <code className="shrink-0 font-mono text-xs text-foreground">
                          {`{{${variable.key || "var"}}}`}
                        </code>
                        {variable.label && (
                          <span className="truncate text-xs text-muted-foreground">
                            {variable.label}
                          </span>
                        )}
                        <Badge variant="outline" className="ml-auto h-5 shrink-0 text-xs">
                          {typeLabel}
                        </Badge>
                        {variable.required && (
                          <span
                            className="h-1.5 w-1.5 shrink-0 rounded-full bg-primary"
                            title={t("inventory.templates.builder.variableRequired")}
                            aria-label={t("inventory.templates.builder.variableRequired")}
                          />
                        )}
                        <IssuePip count={issueCount} />
                      </CollapsibleTrigger>

                      <CollapsibleContent className="pt-4">
                      <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-12">
                        <div className="sm:col-span-4" data-issue={`var-${index}-key`}>
                          <FormField
                            label={
                              <>
                                {t("inventory.templates.builder.variableKey")}
                                <FieldHelp text={t("inventory.templates.editor.help_variableKey")} />
                              </>
                            }
                            htmlFor={`var-${index}-key-input`}
                            error={keyIssue ? issueMessage(keyIssue) : undefined}
                          >
                            <FormFieldControl><Input
                              id={`var-${index}-key-input`}
                              value={variable.key}
                              onChange={(e) => updateVariable(index, { key: e.target.value })}
                              placeholder="site_name"
                              className="h-9 font-mono text-sm"
                              aria-invalid={!!keyIssue}
                            /></FormFieldControl>
                            <code className="text-xs text-muted-foreground">{`{{${variable.key || "var"}}}`}</code>
                          </FormField>
                        </div>
                        <div className="sm:col-span-4">
                          <FormField
                            label={t("inventory.templates.builder.variableLabel")}
                            htmlFor={`var-${index}-label-input`}
                          >
                            <FormFieldControl><Input
                              id={`var-${index}-label-input`}
                              value={variable.label}
                              onChange={(e) => updateVariable(index, { label: e.target.value })}
                              placeholder={t("inventory.templates.builder.variableLabel")}
                              className="h-9 text-sm"
                            /></FormFieldControl>
                          </FormField>
                        </div>
                        <div className="space-y-1.5 sm:col-span-4">
                          <FormSelect<TemplateVariable["value_type"]>
                            id={`var-${index}-type`}
                            label={t("inventory.templates.builder.variableType")}
                            help={valueTypeHelp[variable.value_type]}
                            value={variable.value_type}
                            onValueChange={(value_type) => updateVariable(index, { value_type })}
                            options={TEMPLATE_VARIABLE_VALUE_TYPES.map((opt) => ({
                              value: opt.value,
                              label: opt.label,
                            }))}
                          />
                        </div>
                      </div>

                      {isReference && (
                        <div className="mt-2.5 grid grid-cols-1 gap-2.5 sm:grid-cols-12">
                          <div className="space-y-1.5 sm:col-span-6">
                            <FormSelect
                              id={`var-${index}-reference`}
                              label={t("inventory.templates.builder.variableReference")}
                              help={t("inventory.templates.editor.help_variableReference")}
                              value={referenceResource ?? ""}
                              disabled={hasTemplateVariableConsumers(template, variable)}
                              onValueChange={(resource) => updateVariableReference(index, resource)}
                              options={referenceOptions}
                              placeholder={t("inventory.templates.builder.variableReferencePlaceholder")}
                            />
                          </div>
                        </div>
                      )}

                      <div className="mt-2.5 grid grid-cols-1 gap-2.5 sm:grid-cols-12">
                        <div className="sm:col-span-5" data-issue={`var-${index}-default`}>
                          {renderVariableDefault(variable, index)}
                        </div>
                        <div className="sm:col-span-5">
                          <FormField
                            label={t("inventory.templates.builder.variableHelp")}
                            htmlFor={`var-${index}-help-input`}
                          >
                            <FormFieldControl><Input
                              id={`var-${index}-help-input`}
                              value={variable.description ?? ""}
                              onChange={(e) => updateVariable(index, { description: e.target.value })}
                              placeholder={t("inventory.templates.builder.variableHelp")}
                              className="h-9 text-sm"
                            /></FormFieldControl>
                          </FormField>
                        </div>
                        <div className="flex items-end justify-between gap-2 sm:col-span-2">
                          <FormField
                            label={
                              <>
                                {t("inventory.templates.builder.variableRequired")}
                                <FieldHelp text={t("inventory.templates.editor.help_required")} />
                              </>
                            }
                          >
                            <div className="flex h-9 items-center">
                              <FormFieldControl><Switch
                                checked={!!variable.required}
                                onCheckedChange={(c) => updateVariable(index, { required: c })}
                                aria-label={t("inventory.templates.builder.variableRequired")}
                              /></FormFieldControl>
                            </div>
                          </FormField>
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            className="text-muted-foreground hover:text-destructive"
                            onClick={() => removeVariable(index)}
                            aria-label={t("inventory.templates.builder.deleteVariable")}
                          >
                            <Trash2 className="h-4 w-4" />
                          </Button>
                        </div>
                      </div>
                      </CollapsibleContent>
                    </Collapsible>
                  );
                })}
              </div>

              <Button
                variant="outline"
                size="sm"
                className="h-9 w-full gap-1.5 text-sm"
                onClick={addVariable}
              >
                <Plus className="h-3.5 w-3.5" />
                {t("inventory.templates.builder.addVariable")}
              </Button>
             </FormSection></div>

            {/* ── STEPS ────────────────────────────────────────────────── */}
             <div id="section-steps" className="scroll-mt-4" data-issue="steps-add" tabIndex={-1}>
             <FormSection divider title={t("inventory.templates.builder.steps")} contentClassName="flex flex-col gap-4" action={<>
               <FieldHelp text={t("inventory.templates.editor.help_steps")} />
               <Badge variant="outline" className="text-xs">{template.steps.length}</Badge>
             </>}>

              {template.steps.length === 0 && (
                <p className="rounded-md border border-dashed border-border px-3 py-6 text-center text-xs text-muted-foreground">
                  {t("inventory.templates.builder.noSteps")}
                </p>
              )}

              <div className="space-y-2">
                {template.steps.map((step, index) => {
                  const definition = TEMPLATE_RESOURCE_DEFINITION_MAP[step.resource_type];
                  const keyIssue = issueFor(`step-${index}-key`);
                  const depsIssue = issueFor(`step-${index}-deps`);
                  const otherSteps = template.steps.filter((_, i) => i !== index);
                  const refTokens = stepRefTokens(index);
                  const open = isStepOpen(index);
                  const issueCount = stepIssueCount(index);
                  const fieldCount = definition.payloadFields.length;
                  const depCount = step.depends_on_step_keys.length;
                  return (
                    <Collapsible
                      key={`step-${index}`}
                      open={open}
                      onOpenChange={(next) => setStepOpen(index, next)}
                      className={cn(
                        "border-b pb-4 transition-colors",
                        issueCount > 0 ? "border-status-negative/40" : "border-border",
                      )}
                    >
                      {/* Collapsed summary row + always-available delete control. */}
                      <div className="flex items-center gap-1.5 pr-2">
                        <CollapsibleTrigger className="group flex min-w-0 flex-1 items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors hover:bg-accent/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring data-[state=open]:rounded-b-none">
                          <ChevronDown
                            className="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-180"
                            aria-hidden
                          />
                          <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-primary/10 text-xs font-semibold text-primary tabular-nums">
                            {index + 1}
                          </span>
                          <span className="truncate text-sm font-medium text-foreground">
                            {step.title || definition.defaultTitle}
                          </span>
                          <Badge variant="outline" className="ml-auto h-5 shrink-0 text-xs">
                            {t(`inventory.templates.resourceTypes.${definition.resourceType}`, { defaultValue: definition.label })}
                          </Badge>
                          <span className="hidden shrink-0 text-xs text-muted-foreground sm:inline">
                            {depCount > 0
                              ? t("inventory.templates.editor.stepSummaryWithDeps", {
                                  fields: fieldCount,
                                  deps: depCount,
                                })
                              : t("inventory.templates.editor.stepSummaryFields", { fields: fieldCount })}
                          </span>
                          <IssuePip count={issueCount} />
                        </CollapsibleTrigger>
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          className="shrink-0 text-muted-foreground hover:text-destructive"
                          onClick={() => removeStep(index)}
                          aria-label={t("inventory.templates.builder.deleteStep")}
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>

                      <CollapsibleContent className="pt-4">
                      <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-12">
                        <div className="space-y-1.5 sm:col-span-4">
                          <FormSelect<TemplateStep["resource_type"]>
                            id={`step-${index}-resource`}
                            label={t("inventory.templates.builder.stepResource")}
                            help={definition.description}
                            value={step.resource_type}
                            onValueChange={(resourceType) => replaceStepResource(index, resourceType)}
                            options={TEMPLATE_RESOURCE_DEFINITIONS.map((res) => ({
                              value: res.resourceType,
                              label: t(`inventory.templates.resourceTypes.${res.resourceType}`, { defaultValue: res.label }),
                            }))}
                          />
                        </div>
                        <div className="sm:col-span-4" data-issue={`step-${index}-key`}>
                          <FormField
                            label={
                              <>
                                {t("inventory.templates.builder.stepKey")}
                                <FieldHelp text={t("inventory.templates.editor.help_stepKey")} />
                              </>
                            }
                            htmlFor={`step-${index}-key-input`}
                            error={keyIssue ? issueMessage(keyIssue) : undefined}
                          >
                            <FormFieldControl><Input
                              id={`step-${index}-key-input`}
                              value={step.step_key}
                              onChange={(e) => renameStep(index, e.target.value)}
                              placeholder={definition.defaultStepKey}
                              className="h-9 font-mono text-sm"
                              aria-invalid={!!keyIssue}
                            /></FormFieldControl>
                          </FormField>
                        </div>
                        <div className="sm:col-span-4">
                          <FormField
                            label={t("inventory.templates.builder.stepName")}
                            htmlFor={`step-${index}-name-input`}
                          >
                            <FormFieldControl><Input
                              id={`step-${index}-name-input`}
                              value={step.title}
                              onChange={(e) => updateStep(index, { title: e.target.value })}
                              placeholder={definition.defaultTitle}
                              className="h-9 text-sm"
                            /></FormFieldControl>
                          </FormField>
                        </div>
                      </div>

                      {/* Conflict policy + dependency picker */}
                      <div className="mt-2.5 grid grid-cols-1 gap-2.5 sm:grid-cols-2">
                        <div className="space-y-1.5">
                          <FormSelect<TemplateStep["on_conflict_policy"]>
                            id={`step-${index}-conflict`}
                            label={t("inventory.templates.builder.conflictPolicy")}
                            help={conflictHelp[step.on_conflict_policy]}
                            value={step.on_conflict_policy}
                            onValueChange={(on_conflict_policy) =>
                              updateStep(index, { on_conflict_policy })
                            }
                            options={TEMPLATE_CONFLICT_POLICIES.map((policy) => ({
                              value: policy.value,
                              label: policy.label,
                            }))}
                          />
                          <p className="text-xs text-muted-foreground">
                            {conflictHelp[step.on_conflict_policy]}
                          </p>
                        </div>

                        <div data-issue={`step-${index}-deps`} tabIndex={-1}>
                          <FormField
                            label={
                              <>
                                {t("inventory.templates.builder.dependsOn")}
                                <FieldHelp text={t("inventory.templates.editor.help_dependsOn")} />
                              </>
                            }
                            error={depsIssue ? issueMessage(depsIssue) : undefined}
                          >
                            <FormFieldControl>
                            <div role="group" tabIndex={-1} aria-label={t("inventory.templates.builder.dependsOn")}>
                            {otherSteps.length === 0 ? (
                              <p className="flex h-9 items-center text-sm text-muted-foreground">
                                {t("inventory.templates.editor.noOtherSteps")}
                              </p>
                            ) : (
                              <div className="flex flex-wrap gap-x-3 gap-y-1.5 rounded-md border border-border bg-background p-2">
                                {otherSteps.map((other) => {
                                  const checked = step.depends_on_step_keys.includes(other.step_key);
                                  const cbId = `step-${index}-dep-${other.step_key}`;
                                  return (
                                    <label
                                      key={cbId}
                                      htmlFor={cbId}
                                      className="flex cursor-pointer items-center gap-1.5 text-xs"
                                    >
                                      <Checkbox
                                        id={cbId}
                                        checked={checked}
                                        onCheckedChange={() => toggleDependency(index, other.step_key)}
                                      />
                                      <code className="text-foreground/80">{other.step_key}</code>
                                    </label>
                                  );
                                })}
                              </div>
                            )}
                            </div>
                            </FormFieldControl>
                          </FormField>
                        </div>
                      </div>

                      {/* Payload fields */}
                      <Collapsible defaultOpen className="mt-3">
                        <CollapsibleTrigger className="flex min-h-8 w-full items-center gap-1.5 rounded-md py-1 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground [&[data-state=open]>svg]:rotate-180">
                          <ChevronDown className="h-3.5 w-3.5 transition-transform" />
                          {t("inventory.templates.builder.stepFields")}
                          <Badge variant="outline" className="ml-1 h-4 text-xs">
                            {definition.payloadFields.length}
                          </Badge>
                        </CollapsibleTrigger>
                        <CollapsibleContent className="pt-2">
                          <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 xl:grid-cols-3">
                            {definition.payloadFields.map((field) => {
                              const fieldId = `step-${index}-field-${field.key}`;
                              const fieldIssue = issueFor(fieldId);
                              const supportsTokens = field.valueKind === "string";
                              return (
                                <div key={field.key} data-issue={fieldId}>
                                  <FormField
                                    label={
                                      <>
                                        {field.label}
                                        <FieldHelp text={fieldHelpText(field)} />
                                      </>
                                    }
                                    htmlFor={`${fieldId}-input`}
                                    required={field.required}
                                    error={fieldIssue ? issueMessage(fieldIssue) : undefined}
                                  >
                                    <div className="flex items-center gap-1">
                                      <FormFieldControl><Input
                                        id={`${fieldId}-input`}
                                        value={formatTemplateValue(step.payload_template[field.key])}
                                        onChange={(e) =>
                                          updateStepPayloadField(
                                            index,
                                            field.key,
                                            e.target.value,
                                            field.valueKind,
                                          )
                                        }
                                        placeholder={field.placeholder}
                                        className="h-9 flex-1 font-mono text-sm"
                                        aria-invalid={!!fieldIssue}
                                      /></FormFieldControl>
                                      {supportsTokens && (
                                        <TokenMenu
                                          variableTokens={variableTokens}
                                          stepTokens={refTokens}
                                          onInsert={(token) =>
                                            insertPayloadToken(index, field.key, token)
                                          }
                                          ariaLabel={t("inventory.templates.editor.insertTokenInto", {
                                            field: field.label,
                                          })}
                                        />
                                      )}
                                    </div>
                                  </FormField>
                                </div>
                              );
                            })}
                          </div>
                        </CollapsibleContent>
                      </Collapsible>

                      {/* Variables used by this step */}
                      <StepVariableUsage step={step} className="mt-2.5" />
                      </CollapsibleContent>
                    </Collapsible>
                  );
                })}
              </div>

              <Button
                variant="outline"
                size="sm"
                className="h-9 w-full gap-1.5 text-sm"
                onClick={addStep}
              >
                <Plus className="h-3.5 w-3.5" />
                {t("inventory.templates.builder.addStep")}
              </Button>
             </FormSection></div>
            </div>
          </div>
        </div>

        {/* Preview rail */}
        <div className="hidden min-w-0 lg:block">
          <TemplatePreviewRail template={template} templateId={initial?.id ?? null} />
        </div>
      </div>
    </EntityEditorPage>
  );
};

// ── Variable usage chip strip ──────────────────────────────────────────────

const TOKEN_RE = /\{\{\s*([^}]+?)\s*\}\}/g;

function collectStepTokens(step: TemplateStep): string[] {
  const found = new Set<string>();
  const walk = (value: unknown) => {
    if (typeof value === "string") {
      let match: RegExpExecArray | null;
      TOKEN_RE.lastIndex = 0;
      while ((match = TOKEN_RE.exec(value)) !== null) {
        if (match[1]) found.add(match[1]);
      }
    } else if (Array.isArray(value)) {
      value.forEach(walk);
    } else if (value && typeof value === "object") {
      Object.values(value).forEach(walk);
    }
  };
  walk(step.payload_template);
  return [...found];
}

const StepVariableUsage = ({ step, className }: { step: TemplateStep; className?: string }) => {
  const { t } = useTranslation();
  const tokens = useMemo(() => collectStepTokens(step), [step]);
  if (tokens.length === 0) return null;
  return (
    <div className={cn("flex flex-wrap items-center gap-1.5", className)}>
      <span className="text-xs uppercase tracking-wide text-muted-foreground">
        {t("inventory.templates.editor.usesTokens")}
      </span>
      {tokens.map((token) => (
        <code
          key={token}
          className="rounded bg-primary/8 px-1.5 py-0.5 text-xs text-primary"
        >
          {`{{${token}}}`}
        </code>
      ))}
    </div>
  );
};

export default TemplateEditor;
