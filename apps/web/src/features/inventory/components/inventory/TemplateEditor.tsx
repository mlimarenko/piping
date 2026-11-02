import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Plus,
  Trash2,
  Variable as VarIcon,
  Layers,
  Settings2,
  AlertCircle,
  Eye,
} from "lucide-react";

import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Switch } from "@/shared/ui-primitives/switch";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { Badge } from "@/shared/ui-primitives/badge";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
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
import TemplateEditorPreview from "./TemplateEditorPreview";
import TemplateSortableList, { type TemplateSortableListLabels } from "./TemplateSortableList";
import { moveTemplateStep } from "./templateEditorOrder";
import { templatePayloadFieldLabel } from "./templateSchema";
import {
  applyAutoBoundReferences,
  createTemplateEditorDraft,
  emptyTemplate,
  moveEditorRow,
  stepRow,
  templateFromEditorDraft,
  variableRow,
  type TemplateEditorDraft,
} from "./templateEditorState";

type SectionId = "meta" | "variables" | "steps";
type EditorTab = SectionId | "preview";

interface Props {
  /** The starting template; `null` for a fresh draft. */
  initial: InventoryTemplate | null;
  saving: boolean;
  canSave: boolean;
  onSave: (template: InventoryTemplate) => void;
  onCancel: () => void;
}

const TemplateEditor = ({ initial, saving, canSave, onSave, onCancel }: Props) => {
  const { t } = useTranslation();
  const [draft, setDraft] = useState(() => createTemplateEditorDraft(initial ?? emptyTemplate()));
  const [baseline] = useState(() => templateFromEditorDraft(draft));
  const template = useMemo(() => templateFromEditorDraft(draft), [draft]);
  const structuredDefaults = useMemo(() => draft.variables.map(row => row.structuredDefault), [draft.variables]);
  const [activeTab, setActiveTab] = useState<EditorTab>("meta");
  const [submitted, setSubmitted] = useState(false);
  const [orderError, setOrderError] = useState<string | null>(null);
  const editable = canSave && !saving;
  const edit = (update: (current: TemplateEditorDraft) => TemplateEditorDraft) => {
    if (editable) { setOrderError(null); setDraft(update); }
  };
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
    (draft.variables[index]?.open ?? false) || variableIssueCount(index) > 0;
  const isStepOpen = (index: number): boolean =>
    (draft.steps[index]?.open ?? false) || stepIssueCount(index) > 0;

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

  const patch = (next: Partial<TemplateEditorDraft["details"]>) =>
    edit(current => ({ ...current, details: { ...current.details, ...next } }));

  const onNameChange = (name: string) => {
    if (slugTouched.current) {
      patch({ name });
    } else {
      const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "");
      patch({ name, slug });
    }
  };

  const nextUniqueStepKey = (base: string, steps: TemplateStep[], ignoreIndex?: number): string => {
    const existing = new Set(
      steps.filter((_, i) => i !== ignoreIndex).map((s) => s.step_key),
    );
    if (!existing.has(base)) return base;
    let counter = 2;
    while (existing.has(`${base}_${counter}`)) counter += 1;
    return `${base}_${counter}`;
  };

  // Variables
  const addVariable = () => {
    if (!editable) return;
    const row = variableRow(createEmptyTemplateVariable(), true);
    edit(current => ({ ...current, variables: [...current.variables, row] }));
    requestAnimationFrame(() => focusEditorRow(row.editorId));
  };
  const updateVariable = (index: number, p: Partial<TemplateVariable>) => {
    edit(current => ({ ...current, variables: current.variables.map((row, i) => i === index
      ? { ...row, value: { ...row.value, ...p } } : row) }));
  };
  const removeVariable = (index: number) => {
    edit(current => ({ ...current, variables: current.variables.filter((_, i) => i !== index) }));
  };
  const setVariableOpen = (index: number, open: boolean) =>
    setDraft(current => ({ ...current, variables: current.variables.map((row, i) => i === index ? { ...row, open } : row) }));
  const updateVariableDefault = (index: number, raw: string) => {
    const valueType = template.variables[index]?.value_type ?? "string";
    const variable = template.variables[index];
    const parsed = parseTemplateVariableDefault(valueType, raw,
      variable ? templateReferenceFor(template, variable)?.resource : undefined, structuredDefaults[index] ?? false);
    edit(current => ({ ...current, variables: current.variables.map((row, i) => i === index ? {
      ...row, value: { ...row.value, default_value: parsed.value },
      structuredDefault: row.structuredDefault || valueType !== "json" && isTemplateSelection(parsed.value),
    } : row) }));
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

  const moveVariable = (from: number, to: number): boolean => {
    if (!editable || from === to || from < 0 || to < 0 || from >= draft.variables.length || to >= draft.variables.length) return false;
    edit(current => ({ ...current, variables: moveEditorRow(current.variables, from, to) }));
    return true;
  };
  const moveStep = (from: number, to: number): boolean => {
    if (!editable || from === to || from < 0 || to < 0 || from >= draft.steps.length || to >= draft.steps.length) return false;
    const result = moveTemplateStep(template.steps, from, to);
    if (result.blockingViolation) {
      setOrderError(t("inventory.templates.editor.moveBlocked", {
        step: result.blockingViolation.consumerKey, dependency: result.blockingViolation.producerKey,
      }));
      return false;
    }
    edit(current => ({ ...current, steps: moveEditorRow(current.steps, from, to) }));
    return true;
  };
  const sortableLabels: TemplateSortableListLabels = {
    handle: label => t("inventory.templates.editor.reorderHandle", { label }),
    moveUp: label => t("inventory.templates.editor.moveUp", { label }),
    moveDown: label => t("inventory.templates.editor.moveDown", { label }),
    position: (position, total) => t("inventory.templates.editor.itemPosition", { position, total }),
    instructions: t("inventory.templates.editor.reorderInstructions"),
    roleDescription: t("inventory.templates.editor.sortableItem"),
    pickedUp: (label, position, total) => t("inventory.templates.editor.reorderPickedUp", { label, position, total }),
    moved: (label, position, total) => t("inventory.templates.editor.reorderMoved", { label, position, total }),
    dropped: (label, position, total) => t("inventory.templates.editor.reorderDropped", { label, position, total }),
    cancelled: label => t("inventory.templates.editor.reorderCancelled", { label }),
  };

  // Steps
  const addStep = () => {
    if (!editable) return;
    const row = stepRow(createTemplateStep(), true);
    edit((current) => {
      const values = templateFromEditorDraft(current);
      const withStep: TemplateEditorDraft = {
        ...current,
        steps: [...current.steps, { ...row, value: { ...row.value, step_key: nextUniqueStepKey(row.value.step_key, values.steps), sort_order: current.steps.length } }],
      };
      return applyAutoBoundReferences(withStep, autoBindStepReferences(templateFromEditorDraft(withStep), withStep.steps.length - 1));
    });
    requestAnimationFrame(() => focusEditorRow(row.editorId));
  };
  // General-purpose patch for a single step's fields. The dedicated step-key
  // rename path lives in `renameStep` so reference integrity is maintained.
  const updateStep = (index: number, p: Partial<TemplateStep>) => {
    edit(current => ({ ...current, steps: current.steps.map((row, i) => i === index
      ? { ...row, value: { ...row.value, ...p } } : row) }));
  };
  const renameStep = (index: number, rawKey: string) => {
    edit((current) => {
      const values = templateFromEditorDraft(current);
      const target = values.steps[index];
      if (!target) return current;
      const oldKey = target.step_key;
      const renamed = values.steps.map((s, i) =>
        i === index ? { ...s, step_key: rawKey } : s,
      );
      // Cascade only when the key actually changed to a non-empty value.
      const cascaded =
        rawKey.trim() && rawKey !== oldKey
          ? renameStepReferences(renamed, oldKey, rawKey)
          : renamed;
      return { ...current, steps: current.steps.map((row, i) => ({ ...row, value: cascaded[i]! })) };
    });
  };
  const removeStep = (index: number) => {
    edit(current => {
      const removedKey = current.steps[index]?.value.step_key;
      const remaining = current.steps.filter((_, i) => i !== index);
      const next = remaining.map(row => ({ ...row.value, depends_on_step_keys: row.value.depends_on_step_keys.filter(key => key !== removedKey) }));
      const cleared = removedKey ? clearStepReferences(next, removedKey) : next;
      return { ...current, steps: remaining.map((row, i) => ({ ...row, value: cleared[i]! })) };
    });
  };
  const setStepOpen = (index: number, open: boolean) =>
    setDraft(current => ({ ...current, steps: current.steps.map((row, i) => i === index ? { ...row, open } : row) }));
  const replaceStepResource = (index: number, resourceType: TemplateStep["resource_type"]) => {
    const definition = TEMPLATE_RESOURCE_DEFINITION_MAP[resourceType];
    edit((current) => {
      const values = templateFromEditorDraft(current);
      const reset: InventoryTemplate = {
        ...values,
        steps: values.steps.map((s, i) =>
          i === index
            ? {
                ...s,
                resource_type: resourceType,
                title: definition.defaultTitle,
                step_key: nextUniqueStepKey(definition.defaultStepKey, values.steps, index),
                payload_template: {},
              }
            : s,
        ),
      };
      const bound = autoBindStepReferences(reset, index);
      return applyAutoBoundReferences(current, bound);
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

  const focusEditorRow = (editorId: string) => {
    const row = scrollRef.current?.querySelector<HTMLElement>(`[data-editor-row="${editorId}"]`);
    row?.scrollIntoView({ block: "nearest" });
    row?.querySelector<HTMLElement>('input, textarea, [role="combobox"]')?.focus({ preventScroll: true });
  };

  const handleSave = () => {
    if (!editable) return;
    setSubmitted(true);
    const first = issues[0];
    if (first) {
      setActiveTab(first.section);
      // The panel and its invalid row must mount before targeting the control.
      requestAnimationFrame(() => {
        const el = scrollRef.current?.querySelector<HTMLElement>(`[data-issue="${first.target}"]`);
        if (!el) return;
        el.scrollIntoView({ block: "center" });
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

  const sections: Array<{ value: EditorTab; label: string; icon: typeof VarIcon }> = [
    { value: "meta", label: t("inventory.templates.editor.sectionMeta"), icon: Settings2 },
    { value: "variables", label: `${t("inventory.templates.builder.variables")} (${template.variables.length})`, icon: VarIcon },
    { value: "steps", label: `${t("inventory.templates.builder.steps")} (${template.steps.length})`, icon: Layers },
    { value: "preview", label: t("inventory.templates.editor.previewTab"), icon: Eye },
  ];
  const tabs = sections.map(section => ({ ...section, label: section.value !== "preview" && submitted && issuesBySection[section.value].length
    ? `${section.label} · ${t("inventory.templates.editor.tabErrors", { count: issuesBySection[section.value].length })}`
    : section.label }));

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
      title={template.name.trim() || (initial ? t("inventory.templates.builder.editTitle") : t("inventory.templates.createTemplate"))}
      onBack={onCancel}
      backLabel={t("inventory.templates.editor.backToList")}
      onCancel={onCancel}
      cancelLabel={t("cancel")}
      onSave={canSave ? handleSave : undefined}
      saveLabel={initial ? t("saveChanges") : t("inventory.templates.createTemplate")}
      saving={saving}
      dirty={canSave && JSON.stringify(template) !== JSON.stringify(baseline)}
      dirtyMessage={t("editor.unsavedChanges")}
      error={submitted && issues.length > 0 ? <p className="text-xs text-status-warning">{t("inventory.templates.editor.issueCount", { count: issues.length })}</p> : undefined}
    >

      <Tabs value={activeTab} onValueChange={value => setActiveTab(value as EditorTab)} className="min-w-0">
        <SectionTabs items={tabs} aria-label={t("inventory.templates.editor.sections")} />
        <div ref={scrollRef} className="min-w-0 py-4">
            {/* ── META ─────────────────────────────────────────────────── */}
             <TabsContent value="meta" className="mt-0">
             <fieldset disabled={!editable} className="min-w-0">
             <FormSection title={t("inventory.templates.editor.sectionMeta")} description={t("inventory.templates.editor.intro")} contentClassName="flex flex-col gap-4">

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

              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
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
             </FormSection></fieldset></TabsContent>

            {/* ── VARIABLES ────────────────────────────────────────────── */}
             <TabsContent value="variables" className="mt-0">
             <FormSection title={t("inventory.templates.builder.variables")} description={t("inventory.templates.editor.variableOrderHelp")} contentClassName="flex flex-col gap-4" action={<>
               <FieldHelp text={t("inventory.templates.editor.help_variables")} />
               <Button variant="outline" size="sm" disabled={!editable} onClick={addVariable} className="gap-1.5">
                 <Plus className="size-4" aria-hidden />{t("inventory.templates.builder.addVariable")}
               </Button>
             </>}>

              {template.variables.length === 0 && (
                <p className="rounded-md border border-dashed border-border px-3 py-6 text-center text-xs text-muted-foreground">
                  {t("inventory.templates.builder.noVariables")}
                </p>
              )}

              <TemplateSortableList disabled={!editable} ariaLabel={t("inventory.templates.builder.variables")}
                items={draft.variables.map((row, index) => ({ id: row.editorId, label: row.value.label || row.value.key || t("inventory.templates.editor.variablePosition", { position: index + 1 }) }))}
                labels={sortableLabels} onMove={moveVariable}>
                {(index, { dragHandle, moveControls }) => {
                  const row = draft.variables[index];
                  if (!row) return null;
                  const variable = row.value;
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
                      key={row.editorId}
                      data-editor-row={row.editorId}
                      open={open}
                      onOpenChange={(next) => setVariableOpen(index, next)}
                      className={cn(
                        "rounded-md border bg-background p-2 transition-colors",
                        issueCount > 0 ? "border-status-negative/40" : "border-border",
                      )}
                    >
                      <div className="flex min-w-0 flex-wrap items-center gap-1">
                      {dragHandle}
                      <CollapsibleTrigger
                        className="group flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1 text-left transition-colors hover:bg-accent/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      >
                        <ChevronDown
                          className="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-180"
                          aria-hidden
                        />
                        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                        <code className="order-2 truncate font-mono text-xs text-muted-foreground">
                          {`{{${variable.key || "var"}}}`}
                        </code>
                        {variable.label && (
                          <span className="order-1 truncate text-sm font-medium">
                            {variable.label}
                          </span>
                        )}
                        </span>
                        <Badge variant="outline" className="ml-auto hidden h-5 shrink-0 text-xs sm:inline-flex">
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
                      {moveControls}
                      <Button variant="ghost" size="icon-sm" disabled={!editable} onClick={() => removeVariable(index)}
                        className="text-muted-foreground hover:text-destructive" aria-label={t("inventory.templates.builder.deleteVariable")}>
                        <Trash2 className="size-4" aria-hidden />
                      </Button>
                      </div>

                      <CollapsibleContent className="pt-4"><fieldset disabled={!editable} className="min-w-0">
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
                        </div>
                      </div>
                      </fieldset></CollapsibleContent>
                    </Collapsible>
                  );
                }}
              </TemplateSortableList>

             </FormSection></TabsContent>

            {/* ── STEPS ────────────────────────────────────────────────── */}
             <TabsContent value="steps" className="mt-0" data-issue="steps-add" tabIndex={-1}>
             <FormSection title={t("inventory.templates.builder.steps")} description={t("inventory.templates.editor.stepOrderHelp")} contentClassName="flex flex-col gap-4" action={<>
               <FieldHelp text={t("inventory.templates.editor.help_steps")} />
               <Button variant="outline" size="sm" disabled={!editable} onClick={addStep} className="gap-1.5">
                 <Plus className="size-4" aria-hidden />{t("inventory.templates.builder.addStep")}
               </Button>
             </>}>

              {template.steps.length === 0 && (
                <p className="rounded-md border border-dashed border-border px-3 py-6 text-center text-xs text-muted-foreground">
                  {t("inventory.templates.builder.noSteps")}
                </p>
              )}

              {orderError && <p role="alert" className="rounded-md border border-status-warning/40 bg-status-warning/10 p-3 text-sm text-status-warning">{orderError}</p>}
              <TemplateSortableList disabled={!editable} ariaLabel={t("inventory.templates.builder.steps")}
                items={draft.steps.map((row, index) => ({ id: row.editorId, label: row.value.title || row.value.step_key || t("inventory.templates.editor.stepPosition", { position: index + 1 }) }))}
                labels={sortableLabels} onMove={moveStep}>
                {(index, { dragHandle, moveControls }) => {
                  const row = draft.steps[index];
                  if (!row) return null;
                  const step = row.value;
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
                      key={row.editorId}
                      data-editor-row={row.editorId}
                      open={open}
                      onOpenChange={(next) => setStepOpen(index, next)}
                      className={cn(
                        "rounded-md border bg-background p-2 transition-colors",
                        issueCount > 0 ? "border-status-negative/40" : "border-border",
                      )}
                    >
                      {/* Collapsed summary row + always-available delete control. */}
                      <div className="flex min-w-0 flex-wrap items-center gap-1">
                        {dragHandle}
                        <CollapsibleTrigger className="group flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1 text-left transition-colors hover:bg-accent/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
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
                        {moveControls}
                        <Button
                          variant="ghost"
                          size="icon-sm"
                           className="shrink-0 text-muted-foreground hover:text-destructive"
                           disabled={!editable}
                          onClick={() => removeStep(index)}
                          aria-label={t("inventory.templates.builder.deleteStep")}
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>

                      <CollapsibleContent className="pt-4"><fieldset disabled={!editable} className="min-w-0">
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

                      </fieldset>
                      {/* Payload fields */}
                      <Collapsible defaultOpen className="mt-3">
                        <CollapsibleTrigger className="flex min-h-8 w-full items-center gap-1.5 rounded-md py-1 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground [&[data-state=open]>svg]:rotate-180">
                          <ChevronDown className="h-3.5 w-3.5 transition-transform" />
                          {t("inventory.templates.builder.stepFields")}
                          <Badge variant="outline" className="ml-1 h-4 text-xs">
                            {definition.payloadFields.length}
                          </Badge>
                        </CollapsibleTrigger>
                        <CollapsibleContent className="pt-2"><fieldset disabled={!editable} className="min-w-0">
                          <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 xl:grid-cols-3">
                            {definition.payloadFields.map((field) => {
                              const fieldId = `step-${index}-field-${field.key}`;
                              const fieldLabel = templatePayloadFieldLabel(step.resource_type, field, t);
                              const fieldIssue = issueFor(fieldId);
                              const supportsTokens = field.valueKind === "string";
                              return (
                                <div key={field.key} data-issue={fieldId}>
                                  <FormField
                                    label={
                                      <>
                                        {fieldLabel}
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
                                            field: fieldLabel,
                                          })}
                                        />
                                      )}
                                    </div>
                                  </FormField>
                                </div>
                              );
                            })}
                          </div>
                        </fieldset></CollapsibleContent>
                      </Collapsible>

                      {/* Variables used by this step */}
                      <StepVariableUsage step={step} className="mt-2.5" />
                      </CollapsibleContent>
                    </Collapsible>
                  );
                }}
              </TemplateSortableList>

             </FormSection></TabsContent>
        <TabsContent value="preview" className="mt-0">
          <TemplateEditorPreview template={template} />
        </TabsContent>
        </div>
      </Tabs>
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
