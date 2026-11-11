import { useCallback, useId, useLayoutEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Eye, EyeOff, Loader2, Lock, Plus, ShieldAlert } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import { isUuid } from "@/shared/lib/ids";
import { CustomFieldValidationError, customFieldValidationMessage } from "@/features/inventory/components/custom-fields/customFieldErrors";
import { SECRET_MASK, emptyCustomFieldDraft, customFieldDraftValues, customFieldDraftErrors, buildCustomFieldDraftPayload, type DraftValue, type EntityCustomFieldDraft } from "./entityCustomFieldDraft";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Switch } from "@/shared/ui-primitives/switch";
import { Button } from "@/shared/ui-primitives/button";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import RemovableChip from "@/shared/ui-shared/RemovableChip";
import DefinitionGrid from "@/shared/ui-shared/DefinitionGrid";
import InventoryRelatedValues from "@/features/inventory/components/inventory/InventoryRelatedValues";
import { formatInventoryValue } from "@/features/inventory/components/inventory/inventoryRawFields";
import { CUSTOM_FIELD_CONTENT_TYPE_TOKEN_BY_ENTITY } from "@/features/inventory/tableConfig";
import type { InventoryEntityKey } from "@/shared/api/types";
import { collectAllPages } from "@/shared/api/pagination";
import {
  isChoiceFieldType,
  isMultiFieldType,
  isObjectFieldType,
  listChoiceSets,
  listCustomFields,
  listContentTypes,
  FIELD_TYPES,
  revealEntityCustomFields,
  type ChoiceSet,
  type CustomFieldDefinition,
  type CustomFieldEntityKind,
} from "@/features/inventory/custom-fields-api";

const ENTITY_BY_CONTENT_TYPE_TOKEN = new Map<string, InventoryEntityKey>(
  Object.entries(CUSTOM_FIELD_CONTENT_TYPE_TOKEN_BY_ENTITY)
    .map(([entity, token]) => [token, entity as InventoryEntityKey]),
);

/** Read surfaces never instantiate the draft or register a save payload. */
export function EntityCustomFieldsRead({ entityKind, entityId, customFields }: {
  entityKind: CustomFieldEntityKind | null;
  entityId: string;
  customFields: Record<string, unknown>;
}) {
  const { t } = useTranslation();
  // Full detail must not truncate definitions at the collection's first page.
  // Distinct projection keys keep flattened results out of editor page caches.
  const definitions = useQuery({ queryKey: ["custom-fields", "detail-definitions"], queryFn: () => collectAllPages((limit, offset) => listCustomFields({ limit, offset }), 100) });
  const choices = useQuery({ queryKey: ["custom-field-choice-sets", "detail-choices"], queryFn: () => collectAllPages((limit, offset) => listChoiceSets({ limit, offset }), 100) });
  const hasObjectFields = (definitions.data ?? []).some(def => isObjectFieldType(def.field_type));
  const contentTypes = useQuery({ queryKey: ["inventory-content-types"], queryFn: listContentTypes, enabled: hasObjectFields, staleTime: 5 * 60 * 1000 });
  const tokenById = useMemo(() => new Map((contentTypes.data ?? []).map(type => [type.id, type.token])), [contentTypes.data]);
  if (definitions.isPending) return <p role="status">{t("loading")}</p>;
  if (definitions.isError) return <div role="alert">{t("error")}<Button onClick={() => void definitions.refetch()}>{t("retry")}</Button></div>;
  const fields = definitions.data.filter(def => Object.hasOwn(customFields, def.name) &&
    (def.ui_visible === "always" || (def.ui_visible === "if-set" && !formatInventoryValue(customFields[def.name]).isEmpty)))
    .sort((a, b) => a.weight - b.weight || a.label.localeCompare(b.label));
  return <DefinitionGrid fields={fields.map(def => {
    const raw = customFields[def.name];
    if (isObjectFieldType(def.field_type)) {
      const ids = def.field_type === "multiobject"
        ? (Array.isArray(raw) ? raw.filter((value): value is string => typeof value === "string") : [])
        : (typeof raw === "string" ? [raw] : []);
      const token = def.related_object_type_id ? tokenById.get(def.related_object_type_id) : undefined;
      const related = token ? ENTITY_BY_CONTENT_TYPE_TOKEN.get(token) : undefined;
      if (related && ids.length > 0) {
        return { label: def.label, value: <InventoryRelatedValues entity={related} ids={ids} /> };
      }
    }
    const choiceSet = choices.data?.find(set => set.id === def.choice_set_id);
    const choiceLabel = (value: unknown) => choiceSet?.extra_choices.find(choice => choice.value === value)?.label ?? value;
    const stored = isChoiceFieldType(def.field_type) ? Array.isArray(raw) ? raw.map(choiceLabel) : choiceLabel(raw) : raw;
    return { label: def.label, value: def.is_secret
      ? <CustomFieldSecretRead key={def.name} entityKind={entityKind} entityId={entityId} name={def.name} hasValue={!formatInventoryValue(raw).isEmpty} />
      : <span className="whitespace-pre-wrap break-words">{formatInventoryValue(stored).value || "—"}</span> };
  })} />;
}

function CustomFieldSecretRead({ entityKind, entityId, name, hasValue }: {
  entityKind: CustomFieldEntityKind | null; entityId: string; name: string; hasValue: boolean;
}) {
  const { t } = useTranslation();
  const [revealed, setRevealed] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const reveal = async () => {
    if (revealed !== null) { setRevealed(null); return; }
    if (!entityKind || busy) return;
    setBusy(true);
    try {
      const result = await revealEntityCustomFields(entityKind, entityId);
      // Only this field's explicit authorized endpoint response may be shown.
      setRevealed(typeof result[name] === "string" && result[name] !== SECRET_MASK ? result[name] : null);
      setFailed(false);
    } catch {
      // No backend diagnostic or raw GET value may cross the secret boundary.
      setFailed(true);
    } finally { setBusy(false); }
  };
  return <span className="flex flex-wrap items-center gap-2">
    <span className="whitespace-pre-wrap break-words">{hasValue ? revealed ?? SECRET_MASK : "—"}</span>
    {entityKind && hasValue && <Button variant="ghost" size="sm" disabled={busy} onClick={() => void reveal()}>{t(revealed !== null ? "customFields.entity.hide" : "customFields.entity.reveal")}</Button>}
    {failed && <span role="alert">{t("customFields.entity.noPermission")}</span>}
  </span>;
}

export interface EntityCustomFieldsHandle {
  /**
   * The custom-fields map to send on save, or `undefined` when nothing changed
   * (so the entity's stored custom fields are left untouched). Includes the full
   * current non-secret value set plus only the secret fields the user edited —
   * never the `****` mask, never an unrevealed/untouched secret.
   */
  buildPayload: () => Record<string, unknown> | undefined;
}

interface EntityCustomFieldsSectionProps {
  /** Reveal-endpoint entity kind (`sites` | `devices` | `virtual-machines`). */
  entityKind: CustomFieldEntityKind;
  /** The entity id (for the reveal call). Absent when creating. */
  entityId?: string;
  /** The entity's masked `custom_fields` map from its GET response. */
  customFields: Record<string, unknown> | undefined;
  /** Creation resolves scope from the canonical content-type catalogue, not GET keys. */
  creationScopeToken?: string;
  draftState?: EntityCustomFieldDraft;
  onDraftChange?: (update: (previous: EntityCustomFieldDraft) => EntityCustomFieldDraft) => void;
  /** Imperative handle the host dialog calls on save. */
  onRegister: (handle: EntityCustomFieldsHandle | null) => void;
}

function isValidJson(raw: string): boolean {
  if (raw.trim() === "") return true;
  try { JSON.parse(raw); return true; } catch { return false; }
}

const EntityCustomFieldsSection = ({
  entityKind,
  entityId,
  customFields,
  creationScopeToken,
  draftState,
  onDraftChange,
  onRegister,
}: EntityCustomFieldsSectionProps) => {
  const { t } = useTranslation();

  const creating = creationScopeToken !== undefined;
  const errorsId = useId();
  const scopeQuery = useQuery({ queryKey: ["inventory-content-types"], queryFn: listContentTypes, enabled: creating && creationScopeToken !== "" });
  const scope = scopeQuery.data?.find(type => type.token === creationScopeToken);
  const scopeSupported = !!scope?.supported_for_custom_fields && scope.capabilities?.custom_fields !== false;
  const loadDefinitions = !creating || scopeSupported;
  const defsQuery = useQuery({
    queryKey: ["custom-fields", "editor-definitions"],
    queryFn: () => collectAllPages((limit, offset) => listCustomFields({ limit, offset }), 100),
    enabled: loadDefinitions,
  });

  // The masked map's KEYS are the exact set of definitions scoped to this entity
  // (the backend materializes every applicable definition, valued or not). We
  // intersect global definitions by globally-unique `name`.
  const applicable = useMemo(() => {
    const fields = customFields ?? {};
    const keys = new Set(Object.keys(fields));
    return (defsQuery.data ?? [])
      .filter((d) => creating ? scopeSupported && !!scope && d.object_types.includes(scope.id) : keys.has(d.name))
      .sort((a, b) => a.weight - b.weight || a.label.localeCompare(b.label));
  }, [defsQuery.data, customFields, creating, scope, scopeSupported]);

  const needsChoices = applicable.some(def => isChoiceFieldType(def.field_type));
  const choiceSetsQuery = useQuery({
    queryKey: ["custom-field-choice-sets", "editor-choices"],
    queryFn: () => collectAllPages((limit, offset) => listChoiceSets({ limit, offset }), 100),
    enabled: needsChoices,
  });
  const choiceSetsById = useMemo(() => new Map((choiceSetsQuery.data ?? []).map(choice => [choice.id, choice])), [choiceSetsQuery.data]);
  const pending = (loadDefinitions && defsQuery.isPending) || (creating && creationScopeToken !== "" && scopeQuery.isPending) || (needsChoices && choiceSetsQuery.isPending);
  const unsupported = creating && (!creationScopeToken || (!scopeQuery.isPending && !scopeSupported));
  const unavailable = defsQuery.isError || (creating && scopeQuery.isError) || (needsChoices && choiceSetsQuery.isError) ||
    applicable.some(def => !(FIELD_TYPES as readonly string[]).includes(def.field_type) || (isChoiceFieldType(def.field_type) && !choiceSetsById.has(def.choice_set_id ?? "")));
  const ready = !pending && !unsupported && !unavailable;

  // Per-field draft, dirty-tracking, secret reveal state.
  const [localDraft, setLocalDraft] = useState(emptyCustomFieldDraft);
  const state = draftState ?? localDraft;
  const setDraft = onDraftChange ?? setLocalDraft;
  const draft = useMemo(() => customFieldDraftValues(applicable, customFields, state, creating), [applicable, customFields, state, creating]);
  const errors = useMemo(() => customFieldDraftErrors(applicable, draft, state, creating, choiceSetsById), [applicable, draft, state, creating, choiceSetsById]);
  const [revealed, setRevealed] = useState<Record<string, string>>({});
  const [revealing, setRevealing] = useState<Set<string>>(new Set());
  const [revealDenied, setRevealDenied] = useState<Set<string>>(new Set());

  const setValue = useCallback((name: string, value: DraftValue) => {
    setDraft(prev => ({ ...prev, values: { ...prev.values, [name]: value }, dirty: [...new Set([...prev.dirty, name])] }));
  }, [setDraft]);

  const buildPayload = useCallback((): Record<string, unknown> | undefined => {
    if (!creating && state.dirty.length === 0) return undefined;
    if (!ready) throw new CustomFieldValidationError("", "unavailable");
    setDraft(prev => ({ ...prev, validated: true }));
    return buildCustomFieldDraftPayload(applicable, draft, state, creating, choiceSetsById);
  }, [ready, setDraft, applicable, draft, state, creating, choiceSetsById]);

  useLayoutEffect(() => {
    onRegister({ buildPayload });
    return () => onRegister(null);
  }, [onRegister, buildPayload]);

  const handleReveal = async (def: CustomFieldDefinition) => {
    if (!entityId) return;
    if (revealed[def.name] !== undefined) {
      // Toggle hide.
      setRevealed((prev) => { const n = { ...prev }; delete n[def.name]; return n; });
      return;
    }
    setRevealing((prev) => new Set(prev).add(def.name));
    try {
      const map = await revealEntityCustomFields(entityKind, entityId);
      const value = map[def.name];
      const cleartext = typeof value === "string" && value !== SECRET_MASK ? value : "";
      setRevealed((prev) => ({ ...prev, [def.name]: cleartext }));
      setRevealDenied((prev) => { const n = new Set(prev); n.delete(def.name); return n; });
    } catch (err) {
      const status = (err as { status?: number }).status;
      if (status === 403) {
        setRevealDenied((prev) => new Set(prev).add(def.name));
      } else {
        notify("error", err instanceof Error ? err.message : t("unknownError"));
      }
    } finally {
      setRevealing((prev) => { const n = new Set(prev); n.delete(def.name); return n; });
    }
  };

  // Group applicable + visible fields by group_name, preserving weight order.
  const groups = useMemo(() => {
    // `ui_visible` controls read-only/detail surfaces. The edit form must still
    // expose unset `if-set` fields so the operator can populate them for the
    // first time. Only `ui_editable="hidden"` removes a field from the form.
    const visible = applicable.filter(
      (def) => def.ui_editable !== "hidden",
    );
    const byGroup = new Map<string, CustomFieldDefinition[]>();
    for (const def of visible) {
      const key = def.group_name || "";
      const list = byGroup.get(key) ?? [];
      list.push(def);
      byGroup.set(key, list);
    }
    return [...byGroup.entries()];
  }, [applicable]);

  if (pending) {
    return (
      <div className="flex items-center justify-center py-4">
        <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (unsupported || unavailable) {
    return <p role="alert" className="text-sm text-status-negative">{t("customFields.entity.definitionsUnavailable")}</p>;
  }
  const blockedRequired = [...errors.values()].find(error => error.reason === "uneditable");
  if (blockedRequired && groups.length === 0) {
    return <p role="alert" className="text-sm text-status-negative">{customFieldValidationMessage(t, blockedRequired)}</p>;
  }

  if (applicable.length === 0) {
    return <p className="text-xs text-muted-foreground">{t("customFields.entity.noFields")}</p>;
  }

  if (groups.length === 0) {
    return <p className="text-xs text-muted-foreground">{t("customFields.entity.noFields")}</p>;
  }

  return (
    <div className="space-y-4">
      {blockedRequired && <p role="alert" className="text-sm text-status-negative">{customFieldValidationMessage(t, blockedRequired)}</p>}
      {groups.map(([groupName, defs]) => (
        <div key={groupName || "__ungrouped"} className="space-y-3">
          {groupName && (
            <div className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              {groupName}
            </div>
          )}
          {defs.map((def) => (
            <fieldset key={def.id} aria-label={def.label} aria-invalid={state.validated && errors.has(def.name)} aria-describedby={state.validated && errors.has(def.name) ? `${errorsId}-${def.id}` : undefined} className="min-w-0 flex flex-col gap-1">
            <CustomFieldInput
              def={def}
              value={draft[def.name]}
              choiceSet={def.choice_set_id ? choiceSetsById.get(def.choice_set_id) : undefined}
              onChange={(v) => setValue(def.name, v)}
              hasStoredSecret={
                def.is_secret &&
                typeof (customFields ?? {})[def.name] === "string" &&
                (customFields ?? {})[def.name] === SECRET_MASK
              }
              revealedValue={revealed[def.name]}
              revealing={revealing.has(def.name)}
              revealDenied={revealDenied.has(def.name)}
              canReveal={!!entityId}
              onReveal={() => void handleReveal(def)}
            />
            {state.validated && errors.has(def.name) && <p id={`${errorsId}-${def.id}`} role="alert" className="text-xs text-status-negative">{customFieldValidationMessage(t, errors.get(def.name)!)}</p>}
            </fieldset>
          ))}
        </div>
      ))}
    </div>
  );
};

/* ── Per-field typed input ── */
interface CustomFieldInputProps {
  def: CustomFieldDefinition;
  value: DraftValue | undefined;
  choiceSet?: ChoiceSet;
  onChange: (value: DraftValue) => void;
  hasStoredSecret: boolean;
  revealedValue?: string;
  revealing: boolean;
  revealDenied: boolean;
  canReveal: boolean;
  onReveal: () => void;
}

const CustomFieldInput = ({
  def, value, choiceSet, onChange, hasStoredSecret,
  revealedValue, revealing, revealDenied, canReveal, onReveal,
}: CustomFieldInputProps) => {
  const { t } = useTranslation();
  const readOnly = def.ui_editable !== "yes";
  const str = typeof value === "string" ? value : "";
  const bool = value === true;
  const arr = Array.isArray(value) ? value : [];

  const choiceItems = useMemo(
    () => (choiceSet?.extra_choices ?? []).map((c) => ({ id: c.value, label: c.label })),
    [choiceSet],
  );

  const label = (
    <FieldLabel required={def.required}>
      {def.label}
      {readOnly && (
        <span className="ml-1.5 inline-flex items-center gap-0.5 text-xs font-normal text-muted-foreground">
          <Lock className="h-2.5 w-2.5" /> {t("customFields.entity.readOnly")}
        </span>
      )}
    </FieldLabel>
  );

  const help = def.description ? (
    <p className="text-xs text-muted-foreground">{def.description}</p>
  ) : null;

  // ── Secret field ──
  if (def.is_secret) {
    const showCleartext = revealedValue !== undefined;
    return (
      <div className="space-y-1">
        {label}
        <div className="relative">
          <Input
            className="h-9 text-sm font-mono pr-8"
            type={showCleartext ? "text" : "password"}
            value={showCleartext && str === "" ? revealedValue : str}
            placeholder={hasStoredSecret ? SECRET_MASK : t("customFields.entity.secretUnset")}
            onChange={(e) => onChange(e.target.value)}
            disabled={readOnly}
            autoComplete="off"
            aria-label={def.label}
            spellCheck={false}
          />
          {canReveal && hasStoredSecret && !revealDenied && (
            <Button variant="ghost"
              type="button"
              onClick={onReveal}
              disabled={revealing}
              aria-label={showCleartext ? t("customFields.entity.hide") : t("customFields.entity.reveal")}
              className="absolute right-1.5 top-1/2 -translate-y-1/2 flex items-center justify-center text-muted-foreground hover:text-foreground transition-colors rounded outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              {revealing ? <Loader2 className="h-3.5 w-3.5 animate-spin" />
                : showCleartext ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
            </Button>
          )}
        </div>
        {revealDenied ? (
          <p className="inline-flex items-center gap-1 text-xs text-status-warning">
            <ShieldAlert className="h-3 w-3" /> {t("customFields.entity.noPermission")}
          </p>
        ) : (
          <p className="text-xs text-muted-foreground">
            {hasStoredSecret ? t("customFields.entity.secretEditHint") : t("customFields.entity.secretUnset")}
          </p>
        )}
      </div>
    );
  }

  // ── Boolean ──
  if (def.field_type === "boolean") {
    return (
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          {label}
          {help}
        </div>
        <Switch checked={bool} onCheckedChange={onChange} disabled={readOnly} aria-label={def.label} />
      </div>
    );
  }

  // ── Select (single choice) ──
  if (isChoiceFieldType(def.field_type) && !isMultiFieldType(def.field_type)) {
    return (
      <div className="space-y-1">
        {label}
        <EntitySelect
          items={choiceItems}
          value={str}
          onValueChange={onChange}
          placeholder={t("select")}
          disabled={readOnly}
          aria-label={def.label}
        />
        {help}
      </div>
    );
  }

  // ── Multiselect (choice-backed) ──
  if (def.field_type === "multiselect") {
    return (
      <div className="space-y-1">
        {label}
        <MultiSelect
          items={choiceItems}
          value={arr}
          onValueChange={onChange}
          disabled={readOnly}
          aria-label={def.label}
        />
        {help}
      </div>
    );
  }

  // ── Multiobject (multiple object references) ── No choice set; related-entity
  // options are not resolvable client-side without a content-type listing, so we
  // accept object ids via the same multi-UUID chip entry used by the admin scope
  // picker.
  if (def.field_type === "multiobject") {
    return (
      <div className="space-y-1">
        {label}
        <MultiUuidInput value={arr} onChange={onChange} disabled={readOnly} ariaLabel={def.label} />
        {help}
      </div>
    );
  }

  // ── Single object reference ── (choice items not applicable; rendered as
  // a content-type-scoped id input — the related entity options are not
  // resolvable client-side without a content-type listing, so we accept the id).
  if (isObjectFieldType(def.field_type)) {
    return (
      <div className="space-y-1">
        {label}
        <Input
          className="h-9 text-sm font-mono"
          value={str}
          onChange={(e) => onChange(e.target.value)}
          placeholder={t("customFields.def.contentTypeIdPlaceholder")}
          disabled={readOnly}
          spellCheck={false}
        />
        {help}
      </div>
    );
  }

  // ── JSON ──
  if (def.field_type === "json") {
    const valid = isValidJson(str);
    return (
      <div className="space-y-1">
        {label}
        <Textarea
          className={cn(
            "font-mono text-sm min-h-[120px] resize-y border-border",
            !valid && "border-status-negative/40 focus-visible:ring-status-negative/40",
          )}
          value={str}
          onChange={(e) => onChange(e.target.value)}
          disabled={readOnly}
          spellCheck={false}
          aria-invalid={!valid}
          aria-label={def.label}
        />
        {!valid ? (
          <p className="text-xs text-status-negative">{t("customFields.entity.invalidJson")}</p>
        ) : help}
      </div>
    );
  }

  // ── Long text ──
  if (def.field_type === "longtext") {
    return (
      <div className="space-y-1">
        {label}
        <Textarea
          className="text-sm min-h-[80px] resize-y"
          value={str}
          onChange={(e) => onChange(e.target.value)}
          disabled={readOnly}
          aria-label={def.label}
        />
        {help}
      </div>
    );
  }

  // ── Scalar inputs: text / url / integer / decimal / date / datetime ──
  const inputType =
    def.field_type === "integer" || def.field_type === "decimal" ? "number"
    : def.field_type === "date" ? "date"
    : def.field_type === "datetime" ? "datetime-local"
    : def.field_type === "url" ? "url"
    : "text";

  return (
    <div className="space-y-1">
      {label}
      <Input
        className="h-9 text-sm"
        type={inputType}
        inputMode={def.field_type === "decimal" ? "decimal" : def.field_type === "integer" ? "numeric" : undefined}
        step={def.field_type === "decimal" ? "any" : def.field_type === "integer" ? "1" : undefined}
        value={str}
        onChange={(e) => onChange(e.target.value)}
        disabled={readOnly}
        aria-label={def.label}
        spellCheck={inputType === "text" ? undefined : false}
      />
      {help}
    </div>
  );
};

/* ── Multi-UUID chip entry (for multiobject values) ── */
interface MultiUuidInputProps {
  value: string[];
  onChange: (next: string[]) => void;
  disabled?: boolean;
  ariaLabel: string;
}

const MultiUuidInput = ({ value, onChange, disabled, ariaLabel }: MultiUuidInputProps) => {
  const { t } = useTranslation();
  const [input, setInput] = useState("");
  const [error, setError] = useState(false);

  const add = () => {
    const raw = input.trim();
    if (!raw) return;
    if (!isUuid(raw)) { setError(true); return; }
    if (!value.includes(raw)) onChange([...value, raw]);
    setInput("");
    setError(false);
  };

  return (
    <div className="space-y-1.5">
      <div className="flex gap-2">
        <Input
          className="h-9 text-sm font-mono"
          value={input}
          onChange={(e) => { setInput(e.target.value); if (error) setError(false); }}
          onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); add(); } }}
          placeholder={t("customFields.def.contentTypeIdPlaceholder")}
          disabled={disabled}
          spellCheck={false}
          aria-label={ariaLabel}
        />
        <Button type="button" variant="outline" size="sm" className="h-9 shrink-0 gap-1" onClick={add} disabled={disabled}>
          <Plus className="h-3 w-3" /> {t("add")}
        </Button>
      </div>
      {error && <p className="text-xs text-destructive">{t("customFields.def.errors.invalidUuid")}</p>}
      {value.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {value.map((id) => (
            <RemovableChip
              key={id}
              mono
              label={`${id.slice(0, 8)}…`}
              onRemove={disabled ? undefined : () => onChange(value.filter((v) => v !== id))}
              removeLabel={t("remove")}
            />
          ))}
        </div>
      )}
    </div>
  );
};

export default EntityCustomFieldsSection;
