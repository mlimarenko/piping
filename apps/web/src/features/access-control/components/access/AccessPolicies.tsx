import { useState, useMemo, useEffect, useRef } from "react";
import { useTranslation, Trans } from "react-i18next";
import type { TFunction } from "i18next";
import { useQueries, useQuery } from "@tanstack/react-query";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { PageActions } from "@/shared/ui-shared/PageChrome";
import StatusChip from "@/shared/ui-shared/StatusChip";
import EmptyState from "@/shared/ui-shared/EmptyState";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/shared/ui-primitives/dropdown-menu";
import FormSection from "@/shared/ui-shared/FormSection";
import PermissionPicker from "@/shared/ui-shared/PermissionPicker";
import RemovableChip from "@/shared/ui-shared/RemovableChip";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { TogglePillGroup } from "@/shared/ui-shared/TogglePill";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Label } from "@/shared/ui-primitives/label";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Switch } from "@/shared/ui-primitives/switch";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/shared/ui-primitives/alert-dialog";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import {
  Plus, Pencil, Trash2, FileCheck, ShieldCheck, ShieldX,
  Users, Key, Globe, Server, User, Bot, KeyRound,
  SlidersHorizontal,
} from "lucide-react";
import { notify } from "@/shared/lib/notifications";
import { cn } from "@/shared/lib/utils";
import { usePermissions } from "@/shared/auth/permissionsContext";
import {
  useAccessData, useAccessLabels, actionCatalog,
  type Policy, type SubjectRule, type ScopeRule
} from "./AccessDataContext";
import EntitySelect, { type EntityOption } from "@/shared/ui-shared/EntitySelect";
import EntityRef from "@/shared/ui-shared/EntityRef";
import { listPolicies } from "@/features/access-control/api";
import {
  buildConstraints,
  parseConstraints,
  type AttributeConditionRow,
} from "@/features/access-control/constraints";
import { listAllResources } from "@/features/inventory/api";
import {
  HIERARCHY_KEYS,
  HIERARCHY_SCOPE_DEFS,
  ID_CONSTRAINT_KEYS,
  ID_KEY_ENTITY,
  type HierarchyKey,
} from "@/features/access-control/hierarchyScope";
import {
  AUTOMATION_SECRETS_QUERY_KEY,
  listAutomationSecrets,
} from "@/features/automation-secrets/api";
import { POLICIES_TABLE_CONFIG } from "@/features/access-control/tableConfig";
import { useAccessTableControls, type AccessTableControls } from "@/features/access-control/useAccessTableControls";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";

/* ── Helpers ──────────────────────────── */

/** Kinds the policy picker can target (the read-only `oidc_group` is excluded). */
type SubjectKind = "user" | "group" | "role" | "service_principal";

const subjectIcon = (kind: string) => {
  switch (kind) {
    case "role": return Key;
    case "group": return Users;
    case "service_principal": return Bot;
    case "oidc_group": return Users;
    default: return User;
  }
};
const scopeIcon = (type: string) => {
  if (type === "resource") return KeyRound;
  return Server;
};
const humanizeIdentifier = (value: string) => value
  .split("_")
  .filter(Boolean)
  .map(part => part.charAt(0).toUpperCase() + part.slice(1))
  .join(" ");
const scopeValueLabel = (t: TFunction, type: ScopeRule["type"], value: string) =>
  type === "resource_type"
    // A persisted policy's resource_type is SCREAMING_SNAKE on the wire, but the
    // i18n keys are lowercase — lowercase the lookup so it localizes instead of
    // falling through to the humanized SCREAMING form.
    ? String(t(`accessControl.policies.resourceTypes.${value.toLowerCase()}`, { defaultValue: humanizeIdentifier(value) }))
    : value;
const scopeTypeLabel = (t: TFunction, type: ScopeRule["type"]) => {
  if (type === "resource") return t("accessControl.policies.secret");
  return t("accessControl.policies.resource");
};
/** Friendly, localized label for an action (e.g. "Чтение" for READ). Falls back
 *  to the raw token so an unmapped action is never blank. */
const actionLabel = (t: TFunction, action: string) =>
  String(t(`accessControl.policies.actionLabels.${action.toLowerCase()}`, { defaultValue: action }));
/** Friendly, localized label for a frontend action-catalog group (Connections,
 *  Inventory, …) so the picker headers localize instead of showing English. */
const actionGroupLabel = (t: TFunction, group: string) =>
  String(t(`accessControl.policies.actionGroups.${group.toLowerCase()}`, { defaultValue: group }));
/** Hover tooltip for an action option: "<human label> — <what granting it does>".
 *  Falls back to the raw SCREAMING action token if either string is missing. */
const actionTooltip = (t: TFunction, action: string) => {
  const label = actionLabel(t, action);
  const desc = String(t(`accessControl.policies.actionDescriptions.${action.toLowerCase()}`, { defaultValue: "" }));
  return desc ? `${label} — ${desc}` : label;
};
/** Purpose-first description of a resource type (empty string when unknown). */
const resourceDescription = (t: TFunction, resourceType: string) =>
  String(t(`accessControl.policies.resourceDescriptions.${resourceType}`, { defaultValue: "" }));

/* ── Attribute-constraint value resolution ─────────────────
   The vocabulary tells us WHICH keys a resource can be scoped by; the shared
   hierarchy registry (`../../hierarchyScope.ts` — see its extension guide for
   adding new levels) tells the builder HOW to pick a value for each id key so
   the author never types a raw UUID. `protocol` is a fixed session-protocol
   select; `status` (and anything else) falls back to a free-text input. */
const PROTOCOL_VALUES = ["ssh", "rdp", "vnc"] as const;
/** Human label for a constraint attribute key, i18n-first with a humanized fallback. */
const constraintKeyLabel = (t: TFunction, key: string) =>
  String(t(`accessControl.policies.constraintKeyLabels.${key}`, { defaultValue: humanizeIdentifier(key) }));
/** Which control edits a constraint value: an entity picker for id-valued
 *  keys, the fixed protocol select for `protocol`, else free text. */
const constraintValueKind = (key: string): "entity" | "protocol" | "text" => {
  if (ID_CONSTRAINT_KEYS.includes(key)) return "entity";
  if (key === "protocol") return "protocol";
  return "text";
};

const subjectKindDefs = [
  { key: "user" as const, i18nKey: "users", icon: User },
  { key: "group" as const, i18nKey: "groups", icon: Users },
  { key: "role" as const, i18nKey: "roles", icon: Key },
  { key: "service_principal" as const, i18nKey: "servicePrincipals", icon: Bot },
];

const scopeTypeDefs = [
  { key: "resource_type" as const, i18nKey: "resource", icon: Server },
  { key: "resource" as const, i18nKey: "secret", icon: KeyRound },
];

/** The scope section's active tab: the two policy-scope kinds plus one tab per
 *  inventory-hierarchy attribute key (from the shared hierarchy registry). */
type ScopeTab = ScopeRule["type"] | HierarchyKey;

/** True when the active scope tab is an inventory-hierarchy attribute key
 *  (rather than one of the two policy-scope kinds). */
const isHierarchyTab = (tab: ScopeTab): tab is HierarchyKey =>
  HIERARCHY_KEYS.includes(tab);

/** Resolve a stored constraint value to a display label (named entity for id
 *  keys, else the raw value) for the effective-permissions preview. */
const resolveConstraintValueLabel = (
  idEntityOptionsByKey: Record<string, EntityOption[]>,
  key: string,
  value: string,
): string =>
  ID_CONSTRAINT_KEYS.includes(key)
    ? idEntityOptionsByKey[key]?.find((o) => o.id === value)?.label ?? value
    : value;

/** Group attribute-equality rows by key so the preview reads the ENGINE's
 *  semantics: several values of one key OR together, distinct keys AND together. */
const buildConstraintSummary = (
  t: TFunction,
  attributeRows: AttributeConditionRow[],
  resolveValue: (key: string, value: string) => string,
): string[] => {
  const valuesByKey = new Map<string, string[]>();
  for (const row of attributeRows) {
    if (!row.key || row.value === "") continue;
    const values = valuesByKey.get(row.key) ?? [];
    values.push(resolveValue(row.key, row.value));
    valuesByKey.set(row.key, values);
  }
  const orSeparator = ` ${t("accessControl.policies.constraintPreviewOr")} `;
  return Array.from(valuesByKey.entries()).map(
    ([key, values]) => `${constraintKeyLabel(t, key)} = ${values.join(orSeparator)}`,
  );
};

/** Object-attribute keys authorable for a policy = the union of
 *  `constraint_attribute_keys` over its selected resource-type scopes (uppercase
 *  vocabulary → lowercase scope values already reconciled in the context). The
 *  backend rejects any attributes key outside this union, so the builder offers
 *  exactly these — and nothing when no scopable resource type is selected.
 *  `constraintAttributeKeys` is keyed by the lowercase resource_type, but a
 *  scope value loaded from a persisted policy is SCREAMING_SNAKE wire form
 *  (e.g. "CONNECTION_TARGET"), so normalize before lookup — otherwise the
 *  id-entity queries stay disabled on edit and a saved site/role constraint
 *  renders as a raw UUID instead of the resolved name. */
const buildAttributeKeys = (
  selectedScopes: ScopeRule[],
  constraintAttributeKeys: Record<string, string[]>,
): string[] => {
  const keys = new Set<string>();
  for (const scope of selectedScopes) {
    if (scope.type !== "resource_type") continue;
    for (const key of constraintAttributeKeys[scope.value.toLowerCase()] ?? []) keys.add(key);
  }
  return Array.from(keys);
};

/** Option rows for the active scope tab: hierarchy tabs list their id-entities,
 *  the resource-type tab lists localized resource types, the resource tab lists
 *  named secrets. */
const buildScopeTabOptions = (
  activeScopeType: ScopeTab,
  idEntityOptionsByKey: Record<string, EntityOption[]>,
  resourceTypes: string[],
  secretResourceValues: string[],
  t: TFunction,
  renderScope: (type: ScopeRule["type"], value: string) => string,
): { value: string; label: string; desc: string }[] => {
  if (isHierarchyTab(activeScopeType)) {
    return (idEntityOptionsByKey[activeScopeType] ?? []).map((option) => ({
      value: option.id,
      label: option.label,
      desc: "",
    }));
  }
  if (activeScopeType === "resource_type") {
    return resourceTypes.map((rt) => ({
      value: rt,
      label: scopeValueLabel(t, "resource_type", rt),
      desc: resourceDescription(t, rt),
    }));
  }
  return secretResourceValues.map((value) => ({
    value,
    label: renderScope("resource", value),
    desc: "",
  }));
};

/** Debounce the toolbar search box: mirror `search` into the returned value
 *  after 250ms of quiet, resetting pagination alongside (`setPage` is React's
 *  stable state setter, so it is safe in the effect's dependency list). */
const useDebouncedSearch = (search: string, setPage: (page: number) => void) => {
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const debounceTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (debounceTimer.current !== null) clearTimeout(debounceTimer.current);
    debounceTimer.current = setTimeout(() => {
      setDebouncedSearch(search);
      setPage(0);
    }, 250);
    return () => { if (debounceTimer.current !== null) clearTimeout(debounceTimer.current); };
  }, [search, setPage]);
  return debouncedSearch;
};

/** Inputs to `buildPolicyColumns` — bundled into one typed object so the
 *  builder stays a single-argument call (no positional-parameter sprawl). */
type PolicyColumnsConfig = {
  t: TFunction;
  controls: AccessTableControls;
  canUpdate: boolean;
  canDelete: boolean;
  labels: ReturnType<typeof useAccessLabels>;
  renderScopeValue: (type: ScopeRule["type"], value: string) => string;
  toggleEnabled: (p: Policy) => void;
  openEdit: (p: Policy) => void;
  requestDelete: (p: Policy, invoker: HTMLButtonElement | null) => void;
};

function PolicyRowActions({ policy, canUpdate, canDelete, openEdit, requestDelete, t }: Pick<PolicyColumnsConfig, "canUpdate" | "canDelete" | "openEdit" | "requestDelete" | "t"> & { policy: Policy }) {
  const trigger = useRef<HTMLButtonElement>(null);
  const editAfterClose = useRef(false);
  if (!canUpdate && !canDelete) return null;
  return <RowActions label={`${t("actions")}: ${policy.name}`} ref={trigger} contentProps={{ onCloseAutoFocus: event => {
    if (!editAfterClose.current) return;
    editAfterClose.current = false;
    event.preventDefault();
    // The modal menu releases its focus trap here. Anchor the host to the
    // connected row trigger before opening; the disappearing item is not an invoker.
    if (trigger.current?.isConnected) trigger.current.focus();
    openEdit(policy);
  } }}>
    {canUpdate && <DropdownMenuItem onSelect={() => { editAfterClose.current = true; }}><Pencil />{t("edit")}</DropdownMenuItem>}
    {canUpdate && canDelete && <DropdownMenuSeparator />}
    {canDelete && <DropdownMenuItem variant="destructive" onSelect={() => { trigger.current?.focus(); requestDelete(policy, trigger.current); }}><Trash2 />{t("delete")}</DropdownMenuItem>}
  </RowActions>;
}

/** Policy list table columns: name/effect/enabled/subjects/actions/scopes cells
 *  plus capability-gated row actions. Extracted as a pure builder (mirrors
 *  `buildScopeTabOptions`) so the many per-cell ternaries/`&&` chains don't
 *  count against the main component's cognitive complexity. */
const buildPolicyColumns = ({
  t,
  controls,
  canUpdate,
  canDelete,
  labels,
  renderScopeValue,
  toggleEnabled,
  openEdit,
  requestDelete,
}: PolicyColumnsConfig): Column<Policy>[] => [
  { key: "name", label: t("accessControl.policies.policyName"), ...controls.columnMeta("name"), render: (r: Policy) => (
    <div>
      <div className="flex items-center gap-2">
        <div className={cn("w-5 h-5 rounded flex items-center justify-center", r.effect === "allow" ? "bg-status-positive/10" : "bg-status-negative/10")}>
          {r.effect === "allow" ? <ShieldCheck className="size-4 text-status-positive" /> : <ShieldX className="size-4 text-status-negative" />}
        </div>
        <span className="font-medium text-foreground">{r.name}</span>
        {!r.enabled && <span className="text-xs text-muted-foreground surface-chip px-1 py-0 rounded">OFF</span>}
      </div>
      {r.description && <p className="text-xs text-muted-foreground ml-7 truncate max-w-[250px]">{r.description}</p>}
    </div>
  )},
  { key: "effect", label: t("accessControl.policies.effect"), ...controls.columnMeta("effect"), render: (r: Policy) => <StatusChip status={r.effect === "allow" ? "positive" : "negative"} label={r.effect === "allow" ? t("accessControl.policies.effectAllow") : t("accessControl.policies.effectDeny")} /> },
  { key: "enabled", label: t("connectionSettings.active"), ...controls.columnMeta("enabled"), render: (r: Policy) => (
    <Switch aria-label={`${t("connectionSettings.active")}: ${r.name}`} checked={r.enabled} onCheckedChange={() => toggleEnabled(r)} disabled={!canUpdate} />
  )},
  { key: "subjects", label: t("accessControl.grants.subject"), ...controls.columnMeta("subjects"), render: (r: Policy) => (
    <div className="flex flex-wrap gap-1 max-w-[200px]">
      {r.subjects.slice(0, 3).map(s => (
        <span key={s.value} className="inline-flex items-center gap-1 surface-chip px-1.5 py-0.5 rounded text-xs">
          <EntityRef {...labels.policySubject(s)} icon={subjectIcon(s.kind)} />
        </span>
      ))}
      {r.subjects.length > 3 && <span className="text-xs text-muted-foreground">+{r.subjects.length - 3}</span>}
      {r.subjects.length === 0 && <span className="text-xs text-muted-foreground italic">—</span>}
    </div>
  )},
  { key: "policy_actions", label: t("actions"), ...controls.columnMeta("policy_actions"), render: (r: Policy) => (
    <div className="flex flex-wrap gap-1 max-w-[180px]">
      {r.actions.slice(0, 2).map(a => <span key={a.id} className="font-mono surface-chip px-1.5 py-0.5 rounded text-xs">{a.action}</span>)}
      {r.actions.length > 2 && <span className="text-xs text-muted-foreground">+{r.actions.length - 2}</span>}
    </div>
  )},
  { key: "scopes", label: t("accessControl.grants.scope"), ...controls.columnMeta("scopes"), render: (r: Policy) => (
    <div className="flex flex-wrap gap-1">
      {r.scopes.length === 0 ? <span className="text-xs text-muted-foreground italic">{t("accessControl.grants.scopeGlobal")}</span> :
        r.scopes.slice(0, 2).map(s => <span key={s.id} title={renderScopeValue(s.type, s.value)} className="surface-chip px-1.5 py-0.5 rounded text-xs">{renderScopeValue(s.type, s.value)}</span>)
      }
      {r.scopes.length > 2 && <span className="text-xs text-muted-foreground">+{r.scopes.length - 2}</span>}
    </div>
  )},
  { key: "rowActions", label: "", excludeFromSettings: true, render: (r: Policy) => (
    <PolicyRowActions policy={r} t={t} canUpdate={canUpdate} canDelete={canDelete} openEdit={openEdit} requestDelete={requestDelete} />
  )},
];

/* ── Main Component ──────────────────── */

const AccessPolicies = () => {
  const { t } = useTranslation();
  const [deleteInvoker, setDeleteInvoker] = useState<HTMLButtonElement | null>(null);
  const subjectKinds = subjectKindDefs.map(st => ({ ...st, label: t(`accessControl.tabs.${st.i18nKey}`) }));
  // The two policy-scope tabs reuse `scopeTypeLabel` so the tab, the chip
  // sublabel and the table cells all name the scope kind identically.
  const scopeTypes = scopeTypeDefs.map(st => ({ ...st, label: scopeTypeLabel(t, st.key) }));
  // Hierarchy tabs reuse the constraint-key dictionary so the tab, the chip
  // sublabel and the preview all name the level identically.
  const hierarchyTypes = HIERARCHY_SCOPE_DEFS.map(def => ({
    ...def,
    label: constraintKeyLabel(t, def.key),
  }));
  const {
    createPolicy,
    updatePolicy,
    deletePolicy,
    enablePolicy,
    disablePolicy,
    roles,
    groups,
    users,
    servicePrincipals,
    resourceTypes,
    constraintAttributeKeys,
    uid,
  } = useAccessData();
  const labels = useAccessLabels();
  // View-but-not-edit: hide create/edit/delete/enable affordances the caller can't
  // use, so a READ-only role sees the policy list without dead mutation controls.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "POLICY");
  const canUpdate = can("UPDATE", "POLICY");
  const canDelete = can("DELETE", "POLICY");
  const [page, setPage] = useState(0);
  const { value: pageSize, setValue: setPageSize } = useTablePageSize();
  const [submitting, setSubmitting] = useState(false);
  const [search, setSearch] = useState("");
  const debouncedSearch = useDebouncedSearch(search, setPage);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Policy | null>(null);
  const [editing, setEditing] = useState<Policy | null>(null);

  const controls = useAccessTableControls(POLICIES_TABLE_CONFIG, { onReset: () => setPage(0) });
  const policiesQuery = useQuery({
    queryKey: ["access", "policies", "paginated", { page, pageSize, search: debouncedSearch }, { sort: controls.sortParam, order: controls.orderParam, filters: controls.filterParams }],
    queryFn: () => listPolicies({
      limit: pageSize,
      offset: page * pageSize,
      search: debouncedSearch || undefined,
      sort: controls.sortParam,
      order: controls.orderParam,
      filters: controls.filterParams,
    }),
    placeholderData: (prev) => prev,
  });
  const policies = policiesQuery.data?.items ?? [];
  const total = policiesQuery.data?.total ?? 0;

  const handlePageChange = (p: number) => setPage(p);
  const handlePageSizeChange = (s: number) => { setPageSize(s); setPage(0); };

  // Builder state
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [effect, setEffect] = useState<"allow" | "deny">("allow");
  const [enabled, setEnabled] = useState(true);
  const [selectedActions, setSelectedActions] = useState<string[]>([]);
  const [nameError, setNameError] = useState("");

  // Subjects — canonical `{ id, kind, value: "kind:uuid", label }` rules.
  const [selectedSubjects, setSelectedSubjects] = useState<SubjectRule[]>([]);
  // Subject picker (kind selector + entity selector + add).
  const [subjectKind, setSubjectKind] = useState<SubjectKind>("user");
  const [subjectPick, setSubjectPick] = useState("");
  // Scopes — stored as flat set of "type:value"
  const [selectedScopes, setSelectedScopes] = useState<ScopeRule[]>([]);
  const [activeScopeType, setActiveScopeType] = useState<ScopeTab>("resource_type");
  // Attribute constraints — editable equality rows plus any opaque constraint
  // nodes (ip_range/time_window/…) preserved verbatim from an existing policy.
  const [attributeRows, setAttributeRows] = useState<AttributeConditionRow[]>([]);
  const [constraintExtra, setConstraintExtra] = useState<unknown[]>([]);

  // Search within builder sections
  const [actionSearch, setActionSearch] = useState("");
  const [scopeSearch, setScopeSearch] = useState("");

  // Entity sources per kind, keyed on UUID (EntitySelect submits the id, never
  // the display name). The picker chooses one source by the selected kind.
  const subjectSources = useMemo<Record<SubjectKind, EntityOption[]>>(() => ({
    user: users.map(u => ({ id: u.id, label: u.username, icon: User })),
    group: groups.map(g => ({ id: g.id, label: g.name, icon: Users })),
    role: roles.map(r => ({ id: r.id, label: r.name, icon: Key })),
    service_principal: servicePrincipals.map(s => ({ id: s.id, label: s.name, icon: Bot })),
  }), [users, groups, roles, servicePrincipals]);

  // Named secrets power the per-secret resource multiselect (the first policy
  // type where the UI grants specific ids, not just a resource-type wildcard).
  const secretsQuery = useQuery({
    queryKey: AUTOMATION_SECRETS_QUERY_KEY,
    queryFn: listAutomationSecrets,
  });
  const secretResourceValues = useMemo(
    () => (secretsQuery.data ?? []).map((s) => `secret_variable:${s.id}`),
    [secretsQuery.data],
  );
  const secretLabelByResource = useMemo(
    () => new Map((secretsQuery.data ?? []).map((s) => [`secret_variable:${s.id}`, s.key])),
    [secretsQuery.data],
  );
  // Resolve a scope's display value, showing a named secret's key for the
  // `resource` type (the raw `secret_variable:{uuid}` id is unfriendly).
  const renderScopeValue = (type: ScopeRule["type"], value: string) =>
    type === "resource"
      ? secretLabelByResource.get(value) ?? value
      : scopeValueLabel(t, type, value);

  const selectedScopeCounts = useMemo(() => selectedScopes.reduce<Record<ScopeRule["type"], number>>((acc, scope) => {
    acc[scope.type] += 1;
    return acc;
  }, { resource_type: 0, resource: 0 }), [selectedScopes]);

  // Attribute keys authorable for THIS policy — see `buildAttributeKeys` for
  // the resource-type union + wire-form-normalization rationale. Nothing is
  // offered when no scopable resource type is selected.
  const hasResourceTypeScope = selectedScopes.some((s) => s.type === "resource_type");
  const attributeKeys = useMemo(
    () => buildAttributeKeys(selectedScopes, constraintAttributeKeys),
    [selectedScopes, constraintAttributeKeys],
  );

  // Entity option sources for id-valued keys (role_id → Device Roles, …). Fetched
  // lazily: only while the dialog is open and only for keys the selected resource
  // types actually offer. Exhaustive load so an out-of-first-page entity is still
  // pickable (mirrors the FK-picker completeness rule in inventory/api).
  const idEntityQueries = useQueries({
    queries: ID_CONSTRAINT_KEYS.map((key) => ({
      queryKey: ["inventory", "resources", "all", ID_KEY_ENTITY[key]!],
      queryFn: () => listAllResources(ID_KEY_ENTITY[key]!),
      enabled: dialogOpen && attributeKeys.includes(key),
      staleTime: 5 * 60 * 1000,
    })),
  });
  const idEntityOptionsByKey: Record<string, EntityOption[]> = {};
  ID_CONSTRAINT_KEYS.forEach((key, i) => {
    idEntityOptionsByKey[key] = (idEntityQueries[i]?.data ?? []).map((r) => ({ id: r.id, label: r.name }));
  });
  // Resolve a stored constraint value to a display label (named entity for id
  // keys, else the raw value) for the effective-permissions preview.
  const constraintValueLabel = (key: string, value: string): string =>
    resolveConstraintValueLabel(idEntityOptionsByKey, key, value);
  // Group by key so the preview reads the ENGINE's semantics: several values
  // of one key OR together («Площадка = A или B»), distinct keys AND together.
  const constraintSummary = buildConstraintSummary(t, attributeRows, constraintValueLabel);

  /* ── Scope tabs: unified option plumbing ─────────────
     The two policy-scope tabs (resource types / named secrets) keep writing
     `selectedScopes`; each hierarchy tab reads/writes the `attributeRows` of
     its key — several checked values of one level OR together, different
     levels AND together, so checkboxes can never author a contradiction. */
  // Hierarchy scoping is only enforceable for resource types that declare
  // these attribute keys (today: Цель подключения) — mirror the write-time gate.
  const hierarchyAvailable = HIERARCHY_KEYS.some((key) => attributeKeys.includes(key));
  const hierarchyRowCount = (key: string) =>
    attributeRows.filter((row) => row.key === key && row.value !== "").length;
  const activeScope = isHierarchyTab(activeScopeType)
    ? hierarchyTypes.find((ht) => ht.key === activeScopeType)!
    : scopeTypes.find((st) => st.key === activeScopeType) ?? scopeTypes[0]!;
  const activeTabCount = isHierarchyTab(activeScopeType)
    ? hierarchyRowCount(activeScopeType)
    : selectedScopeCounts[activeScopeType];
  const scopeTabOptions = buildScopeTabOptions(
    activeScopeType,
    idEntityOptionsByKey,
    resourceTypes,
    secretResourceValues,
    t,
    renderScopeValue,
  );
  const scopeQuery = scopeSearch.trim().toLowerCase();
  const filteredScopeOptions = scopeQuery
    ? scopeTabOptions.filter(
        (option) =>
          option.label.toLowerCase().includes(scopeQuery) ||
          option.value.toLowerCase().includes(scopeQuery),
      )
    : scopeTabOptions;
  const isTabValueSelected = (tab: ScopeTab, value: string) =>
    isHierarchyTab(tab)
      ? attributeRows.some((row) => row.key === tab && row.value === value)
      : isScopeSelected(tab, value);
  const toggleTabValue = (tab: ScopeTab, value: string) => {
    if (isHierarchyTab(tab)) {
      setAttributeRows((prev) => {
        const existing = prev.find((row) => row.key === tab && row.value === value);
        if (existing) return prev.filter((row) => row.id !== existing.id);
        return [...prev, { id: uid(), key: tab, value }];
      });
      return;
    }
    toggleScope(tab, value);
  };
  // Hierarchy selections rendered as removable chips next to the scope chips.
  const hierarchyChips = attributeRows.filter(
    (row) => HIERARCHY_KEYS.includes(row.key) && row.value !== "",
  );
  // The conditions section keeps only the NON-hierarchy keys (protocol, status,
  // …): inventory levels are authored via the scope tabs above, so offering
  // them here again would be the same state behind two controls.
  const conditionKeys = attributeKeys.filter((key) => !HIERARCHY_KEYS.includes(key));
  const conditionRows = attributeRows.filter((row) => !HIERARCHY_KEYS.includes(row.key));

  const resetForm = () => {
    setName(""); setDescription(""); setEffect("allow"); setEnabled(true);
    setSelectedActions([]); setSelectedSubjects([]); setSelectedScopes([]);
    setNameError(""); setActionSearch(""); setScopeSearch("");
    setActiveScopeType("resource_type");
    setSubjectKind("user"); setSubjectPick("");
    setAttributeRows([]); setConstraintExtra([]);
  };

  const openCreate = () => { setEditing(null); resetForm(); setDialogOpen(true); };
  const openEdit = (p: Policy) => {
    setEditing(p); setName(p.name); setDescription(p.description);
    setEffect(p.effect); setEnabled(p.enabled);
    setSelectedActions(p.actions.map(a => a.action));
    setSelectedSubjects([...p.subjects]);
    setSelectedScopes([...p.scopes]);
    // Seed the constraint builder from the stored expression; unmodeled nodes
    // (ip_range/time_window/…) are held in `constraintExtra` and re-emitted on save.
    const parsedConstraints = parseConstraints(p.constraints);
    setAttributeRows(parsedConstraints.rows);
    setConstraintExtra(parsedConstraints.extra);
    setNameError(""); setActionSearch(""); setScopeSearch("");
    setActiveScopeType("resource_type");
    setSubjectKind("user"); setSubjectPick("");
    setDialogOpen(true);
  };

  /* Action toggles */
  const selectedActionSet = useMemo(() => new Set(selectedActions), [selectedActions]);
  const allCatalogActions = useMemo(
    () => Array.from(new Set(actionCatalog.flatMap(g => g.actions))),
    [],
  );
  const filteredActionGroups = useMemo(() => {
    const query = actionSearch.trim().toLowerCase();
    return actionCatalog
      .map(g => ({
        ...g,
        actions: query ? g.actions.filter(a => a.toLowerCase().includes(query)) : g.actions,
      }))
      .filter(g => g.actions.length > 0);
  }, [actionSearch]);

  const toggleAction = (action: string) => {
    setSelectedActions(prev => prev.includes(action) ? prev.filter(a => a !== action) : [...prev, action]);
  };
  const toggleActionGroup = (groupActions: string[]) => {
    const allSelected = groupActions.every(a => selectedActions.includes(a));
    if (allSelected) {
      setSelectedActions(prev => prev.filter(a => !groupActions.includes(a)));
    } else {
      setSelectedActions(prev => [...new Set([...prev, ...groupActions])]);
    }
  };
  const selectOnlyActionGroup = (groupActions: string[]) => setSelectedActions([...new Set(groupActions)]);
  const allActionsSelected = allCatalogActions.every(a => selectedActionSet.has(a));
  const toggleAllCatalogActions = () => {
    setSelectedActions(allActionsSelected ? [] : allCatalogActions);
  };

  /* Subject add — append the picked entity as a canonical `kind:uuid` rule. */
  const addSubject = () => {
    if (!subjectPick) return;
    const value = `${subjectKind}:${subjectPick}`;
    if (selectedSubjects.some(s => s.value === value)) { setSubjectPick(""); return; }
    const label = subjectSources[subjectKind].find(o => o.id === subjectPick)?.label ?? subjectPick;
    setSelectedSubjects(prev => [...prev, { id: subjectPick, kind: subjectKind, value, label }]);
    setSubjectPick("");
  };

  /* Scope toggles — direct click */
  const toggleScope = (type: ScopeRule["type"], value: string) => {
    setSelectedScopes(prev => {
      const exists = prev.find(s => s.type === type && s.value === value);
      if (exists) return prev.filter(s => s.id !== exists.id);
      return [...prev, { id: uid(), type, value }];
    });
  };
  const isScopeSelected = (type: string, value: string) =>
    selectedScopes.some(s => s.type === type && s.value === value);

  /* Attribute-constraint rows — add / change key / change value / remove. A key
     change resets the value (the value control's shape depends on the key). */
  const addAttributeRow = () => {
    // Only non-hierarchy keys are authorable here — inventory levels live in
    // the scope tabs (see `conditionKeys`).
    const firstKey = conditionKeys[0];
    if (!firstKey) return;
    setAttributeRows(prev => [...prev, { id: uid(), key: firstKey, value: "" }]);
  };
  const setAttributeRowKey = (id: string, key: string) =>
    setAttributeRows(prev => prev.map(r => (r.id === id ? { ...r, key, value: "" } : r)));
  const setAttributeRowValue = (id: string, value: string) =>
    setAttributeRows(prev => prev.map(r => (r.id === id ? { ...r, value } : r)));
  const removeAttributeRow = (id: string) =>
    setAttributeRows(prev => prev.filter(r => r.id !== id));

  const handleSave = async () => {
    if (!name.trim()) { setNameError(t("accessControl.policies.errors.nameRequired")); return; }
    if (selectedActions.length === 0) { notify("error", t("accessControl.policies.errors.noActions")); return; }

    const payload: Omit<Policy, "id"> = {
      name: name.trim(),
      description: description.trim(),
      effect,
      enabled,
      actions: selectedActions.map((a) => ({ id: uid(), action: a })),
      subjects: selectedSubjects,
      scopes: selectedScopes,
      // Attribute-equality rows + preserved opaque nodes → the constraint grammar
      // (undefined when nothing is set, keeping unconstrained policies clean).
      constraints: buildConstraints(attributeRows, constraintExtra),
    };

    setSubmitting(true);
    try {
      if (editing) {
        await updatePolicy(editing.id, payload);
      } else {
        await createPolicy(payload);
      }
      setDialogOpen(false);
    } finally {
      setSubmitting(false);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    await deletePolicy(deleteTarget.id);
    setDeleteTarget(null);
  };

  const toggleEnabled = async (p: Policy) => {
    if (p.enabled) {
      await disablePolicy(p.id);
    } else {
      await enablePolicy(p.id);
    }
  };

  /* ── Table ──────────────────────────── */
  const columns = buildPolicyColumns({
    t,
    controls,
    canUpdate,
    canDelete,
    labels,
    renderScopeValue,
    toggleEnabled: (p) => {
      toggleEnabled(p).catch(() => undefined);
    },
    openEdit,
    requestDelete: (policy, invoker) => { setDeleteInvoker(invoker); setDeleteTarget(policy); },
  });

  // Toolbar: search + "New rule".
  const toolbar = (
    <>
      <SearchInput
        value={search}
        onValueChange={setSearch}
        placeholder={t("accessControl.rules.searchPlaceholder")}
        containerClassName="flex-1 max-w-xs"
      />
    </>
  );

  const body = (
    <DataTable
      tableId="access.policies"
      className="min-h-0"
      columns={columns}
      data={policies}
      getRowKey={r => r.id}
      loading={policiesQuery.isPending}
      error={policiesQuery.error}
      onRetry={() => policiesQuery.refetch()}
      emptyState={<EmptyState icon={FileCheck} title={t("accessControl.policies.noPolicies")} description={t("accessControl.policies.noPoliciesDescription")} actionLabel={canCreate ? t("accessControl.policies.createPolicy") : undefined} onAction={canCreate ? openCreate : undefined} />}
      pagination={{ page, pageSize, total, onPageChange: handlePageChange, onPageSizeChange: handlePageSizeChange }}
      sortState={controls.sortState}
      onSortChange={controls.onSortChange}
      columnFilters={controls.columnFilters}
      onColumnFilterChange={controls.onColumnFilterChange}
      toolbar={toolbar}
    />
  );

  return (
    <>
      <PageActions>{canCreate && <Button size="sm" onClick={openCreate}><Plus className="size-4" /> {t("accessControl.rules.newRule")}</Button>}</PageActions>
      {body}

      {/* ── Policy Builder Dialog — token-form aesthetic: a single scrolling
          column of flat FormSections sharing the same chrome as Create-Token. ── */}
      <EntityEditorHost
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        className="w-full"
        title={
          <span className="inline-flex items-center gap-2">
            {effect === "allow" ? <ShieldCheck className="h-4 w-4 text-status-positive" /> : <ShieldX className="h-4 w-4 text-status-negative" />}
            {editing ? t("accessControl.policies.editPolicy") : t("accessControl.policies.createPolicy")}
          </span>
        }
        description={editing ? t("accessControl.policies.updateDescription") : t("accessControl.policies.createDescription")}
        footer={
          <>
            <Button data-editor-cancel variant="outline" onClick={() => setDialogOpen(false)} disabled={submitting}>{t("cancel")}</Button>
            <Button onClick={handleSave} className="gap-1" disabled={submitting}>
              {effect === "allow" ? <ShieldCheck /> : <ShieldX />}
              {editing ? t("saveChanges") : t("accessControl.policies.createPolicy")}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-6">
          {/* ─ Basics ─ */}
          <FormSection title={t("accessControl.policies.builderBasics")} description={t("accessControl.policies.policyDescription")}>
            <div className="space-y-3">
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-[minmax(0,1fr)_auto]">
                <div className="space-y-1">
                  <FieldLabel htmlFor="policy-name" className="text-sm" required>{t("name")}</FieldLabel>
                  <Input id="policy-name" aria-invalid={!!nameError} value={name} onChange={e => { setName(e.target.value); setNameError(""); }} placeholder={t("accessControl.policies.namePlaceholder")} maxLength={100} autoFocus />
                  {nameError && <p className="text-xs text-destructive">{nameError}</p>}
                </div>
                <div className="space-y-1">
                  <span id="policy-effect-label" className="text-sm">{t("accessControl.policies.effect")}</span>
                  <div role="group" aria-labelledby="policy-effect-label">
                  <TogglePillGroup<"allow" | "deny">
                    value={effect}
                    onValueChange={setEffect}
                    options={[
                      {
                        value: "allow",
                        label: <span className="inline-flex items-center gap-1.5"><ShieldCheck className="size-4" /> {t("accessControl.policies.effectAllow")}</span>,
                        activeClassName: "border-status-positive/30 bg-status-positive/10 text-status-positive",
                      },
                      {
                        value: "deny",
                        label: <span className="inline-flex items-center gap-1.5"><ShieldX className="size-4" /> {t("accessControl.policies.effectDeny")}</span>,
                        activeClassName: "border-status-negative/30 bg-status-negative/10 text-status-negative",
                      },
                    ]}
                  />
                  </div>
                </div>
              </div>
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-[minmax(0,1fr)_auto]">
                <div className="space-y-1">
                  <Label htmlFor="policy-description" className="text-sm">{t("description")}</Label>
                  <Textarea id="policy-description" value={description} onChange={e => setDescription(e.target.value)} placeholder={t("configContexts.describe")} maxLength={500} />
                </div>
                <div className="space-y-1">
                  <span className="text-sm">{t("status")}</span>
                  <div className="flex items-center gap-2 h-9 px-3 rounded-md surface-inset">
                    <Switch checked={enabled} onCheckedChange={setEnabled} id="pol-enabled" className="scale-90" />
                    <Label htmlFor="pol-enabled" className="text-sm">{enabled ? t("connectionSettings.active") : t("disabled")}</Label>
                  </div>
                </div>
              </div>
            </div>
          </FormSection>

          {/* ─ Actions ─ */}
          <FormSection divider required title={t("accessControl.policies.builderActions")} description={t("accessControl.policies.rules")}>
            <PermissionPicker
              groups={filteredActionGroups.map(g => ({
                key: g.group,
                title: actionGroupLabel(t, g.group),
                subtitle: `${g.actions.filter(a => selectedActionSet.has(a)).length}/${g.actions.length} · ${t("accessControl.policies.actionGroupHint", { count: g.actions.length })}`,
                items: g.actions.map(action => ({ value: action, label: actionLabel(t, action), hint: action.toLowerCase(), description: actionTooltip(t, action) })),
              }))}
              selectedValues={selectedActionSet}
              onToggleValue={toggleAction}
              countLabel={t("accessControl.policies.actionsSelectedSummary", { count: selectedActions.length })}
              hasSelection={selectedActions.length > 0}
              onClearAll={() => setSelectedActions([])}
              clearAllLabel={t("accessControl.policies.clearAllActions")}
              toolbarActions={
                <Button type="button" variant="outline" size="sm" onClick={toggleAllCatalogActions}>
                  {allActionsSelected ? t("accessControl.policies.clearAllActions") : t("selectAll")}
                </Button>
              }
              searchValue={actionSearch}
              onSearchChange={setActionSearch}
              searchPlaceholder={t("accessControl.policies.searchActions")}
              emptyLabel={t("noResults")}
              renderGroupActions={(group, state) => {
                const values = group.items.map(item => item.value);
                return (
                  <>
                    <button
                      type="button"
                      onClick={() => toggleActionGroup(values)}
                      className={cn(
                        "rounded border px-1.5 py-0.5 text-xs font-medium transition-colors",
                        state.allSelected ? "border-primary/60 bg-primary/10 text-primary" : "border-border text-muted-foreground hover:border-primary/40 hover:text-foreground",
                      )}
                    >
                      {state.allSelected ? t("accessControl.policies.clearSection") : t("accessControl.policies.selectSection")}
                    </button>
                    <button
                      type="button"
                      onClick={() => selectOnlyActionGroup(values)}
                      className="rounded border border-border px-1.5 py-0.5 text-xs font-medium text-muted-foreground transition-colors hover:border-primary/40 hover:text-foreground"
                    >
                      {t("accessControl.policies.onlySection")}
                    </button>
                  </>
                );
              }}
            />
            {selectedActions.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-1 border-t border-border pt-2">
                {selectedActions.map(a => (
                  <RemovableChip key={a} label={actionLabel(t, a)} title={actionTooltip(t, a)} onRemove={() => toggleAction(a)} removeLabel={t("delete")} />
                ))}
              </div>
            )}
          </FormSection>

          {/* ─ Subjects ─ */}
          <FormSection divider title={t("accessControl.policies.builderSubjects")} description={t("accessControl.grants.noGrantsDescription")}>
            <div className="flex flex-wrap items-end gap-2">
              <div className="w-36 space-y-1">
                <FormSelect<SubjectKind>
                  label={t("type")}
                  labelClassName="text-xs text-muted-foreground"
                  value={subjectKind}
                  onValueChange={(value) => {
                    setSubjectKind(value);
                    setSubjectPick("");
                  }}
                  options={subjectKinds.map((sk) => ({
                    value: sk.key,
                    label: (
                      <span className="inline-flex items-center gap-1.5">
                        <sk.icon className="size-4 text-muted-foreground" />
                        {sk.label}
                      </span>
                    ),
                  }))}
                />
              </div>
              <div className="min-w-[140px] flex-1 space-y-1">
                <FieldLabel htmlFor="policy-subject" className="text-sm font-medium">{t("accessControl.grants.subject")}</FieldLabel>
                <EntitySelect
                  id="policy-subject"
                  items={subjectSources[subjectKind]}
                  value={subjectPick}
                  onValueChange={setSubjectPick}
                  placeholder={t("accessControl.grants.selectSubject")}
                  triggerClassName="h-9 text-sm"
                />
              </div>
              <Button size="sm" className="shrink-0 gap-1" onClick={addSubject} disabled={!subjectPick}>
                <Plus /> {t("add")}
              </Button>
            </div>
            {selectedSubjects.length > 0 ? (
              <div className="mt-2 flex flex-wrap gap-1 border-t border-border pt-2">
                {selectedSubjects.map(s => {
                  const Icon = subjectIcon(s.kind);
                  const resolved = labels.policySubject(s);
                  return (
                    <RemovableChip
                      key={s.value}
                      icon={<Icon className="h-3.5 w-3.5" />}
                      label={resolved.name ?? s.label ?? s.id}
                      title={resolved.name ?? s.label ?? s.id}
                      sublabel={s.kind}
                      onRemove={() => setSelectedSubjects(prev => prev.filter(x => x.value !== s.value))}
                      removeLabel={t("delete")}
                    />
                  );
                })}
              </div>
            ) : (
              <p className="mt-1 text-xs italic text-muted-foreground">{t("accessControl.policies.noSubjectsSelected")}</p>
            )}
          </FormSection>

          {/* ─ Scope ─ */}
          <FormSection divider title={t("accessControl.policies.builderScope")} description={t("accessControl.policies.globalScopeHint")}>
            <TogglePillGroup<ScopeTab>
              shape="md"
              value={activeScopeType}
              onValueChange={(value) => { setActiveScopeType(value); setScopeSearch(""); }}
              options={[
                ...scopeTypes.map(st => ({
                  value: st.key,
                  count: selectedScopeCounts[st.key],
                  label: <span className="inline-flex items-center gap-1.5"><st.icon className="size-4" /> {st.label}</span>,
                })),
                ...hierarchyTypes.map(ht => ({
                  value: ht.key,
                  count: hierarchyRowCount(ht.key),
                  disabled: !attributeKeys.includes(ht.key),
                  label: <span className="inline-flex items-center gap-1.5"><ht.icon className="size-4" /> {ht.label}</span>,
                })),
              ]}
            />
            {!hierarchyAvailable && (
              <p className="mt-1.5 text-xs italic text-muted-foreground">
                {t("accessControl.policies.hierarchyScopeUnavailable")}
              </p>
            )}
            <SearchInput
              value={scopeSearch}
              onValueChange={setScopeSearch}
              placeholder={t("search")}
              containerClassName="mt-2"
            />
            <div className="mt-2 overflow-hidden rounded-md border border-border bg-card">
              <div className="flex min-w-0 items-center gap-2 border-b border-border px-2.5 py-1.5">
                <activeScope.icon className="size-4 shrink-0 text-muted-foreground" />
                <span className="min-w-0 flex-1 truncate text-xs font-semibold uppercase tracking-wider text-muted-foreground">{activeScope.label}</span>
                <span className="shrink-0 text-xs text-muted-foreground">
                  {t("accessControl.policies.scopeSelected", { count: activeTabCount })}
                </span>
              </div>
              <div className="h-36 overflow-y-auto overflow-x-hidden px-1 py-1" data-testid="policy-scope-options">
                {filteredScopeOptions.length > 0 ? filteredScopeOptions.map(opt => (
                  <label
                    key={`${activeScopeType}:${opt.value}`}
                    title={opt.desc || opt.label}
                    className={cn(
                      "flex min-h-9 min-w-0 cursor-pointer gap-2 rounded px-2 py-1 hover:bg-accent",
                      opt.desc ? "items-start" : "items-center",
                    )}
                  >
                    <Checkbox
                      checked={isTabValueSelected(activeScopeType, opt.value)}
                      onCheckedChange={() => toggleTabValue(activeScopeType, opt.value)}
                      className={cn("size-4 shrink-0", opt.desc && "mt-0.5")}
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block break-words text-sm">{opt.label}</span>
                      {opt.desc && <span className="block truncate text-xs text-muted-foreground">{opt.desc}</span>}
                    </span>
                  </label>
                )) : (
                  <div className="px-2 py-5 text-center text-xs text-muted-foreground">{t("noResults")}</div>
                )}
              </div>
            </div>
            {selectedScopes.length > 0 || hierarchyChips.length > 0 ? (
              <div className="mt-2 flex flex-wrap gap-1 border-t border-border pt-2">
                {selectedScopes.map(s => {
                  const Icon = scopeIcon(s.type);
                  return (
                    <RemovableChip
                      key={s.id}
                      icon={<Icon className="h-3.5 w-3.5" />}
                      label={renderScopeValue(s.type, s.value)}
                      title={renderScopeValue(s.type, s.value)}
                      sublabel={scopeTypeLabel(t, s.type)}
                      onRemove={() => setSelectedScopes(prev => prev.filter(x => x.id !== s.id))}
                      removeLabel={t("delete")}
                    />
                  );
                })}
                {hierarchyChips.map(row => {
                  const def = HIERARCHY_SCOPE_DEFS.find(d => d.key === row.key);
                  const Icon = def?.icon ?? Globe;
                  return (
                    <RemovableChip
                      key={row.id}
                      icon={<Icon className="h-3.5 w-3.5" />}
                      label={constraintValueLabel(row.key, row.value)}
                      title={constraintValueLabel(row.key, row.value)}
                      sublabel={constraintKeyLabel(t, row.key)}
                      onRemove={() => removeAttributeRow(row.id)}
                      removeLabel={t("delete")}
                    />
                  );
                })}
              </div>
            ) : (
              <p className="mt-1 text-xs italic text-muted-foreground">{t("accessControl.grants.scopeGlobal")}</p>
            )}
          </FormSection>

          {/* ─ Attribute conditions ─ */}
          <FormSection divider title={t("accessControl.policies.builderConstraints")} description={t("accessControl.policies.constraintsHint")}>
            <AttributeConditionsSection
              t={t}
              hasResourceTypeScope={hasResourceTypeScope}
              conditionKeys={conditionKeys}
              conditionRows={conditionRows}
              idEntityOptionsByKey={idEntityOptionsByKey}
              onKeyChange={setAttributeRowKey}
              onValueChange={setAttributeRowValue}
              onRemoveRow={removeAttributeRow}
              onAddRow={addAttributeRow}
            />
          </FormSection>

          {/* ─ Effective-permissions preview (readability anchor) ─ */}
          <FormSection divider title={t("accessControl.policies.effectivePermissions")}>
            <SurfaceCard variant="inset" className="p-3">
              <PolicyPreview
                effect={effect}
                enabled={enabled}
                selectedSubjects={selectedSubjects}
                selectedActions={selectedActions}
                selectedScopes={selectedScopes}
                constraintSummary={constraintSummary}
                labels={labels}
                renderScope={renderScopeValue}
                t={t}
              />
            </SurfaceCard>
          </FormSection>
        </div>
      </EntityEditorHost>

      {/* Delete confirm */}
      <AlertDialog open={!!deleteTarget} onOpenChange={open => !open && setDeleteTarget(null)}>
        <AlertDialogContent className="border border-border bg-card" onCloseAutoFocus={event => { if (deleteInvoker?.isConnected) { event.preventDefault(); deleteInvoker.focus(); } }}>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("accessControl.policies.deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              <Trans
                i18nKey="accessControl.policies.deleteDescription"
                values={{ name: deleteTarget?.name ?? "" }}
                components={{ 1: <strong /> }}
              />
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete} className="bg-destructive text-destructive-foreground hover:bg-destructive/90">{t("delete")}</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
};

/* ── Sub-components ──────────────────── */

/** The "Attribute conditions" builder section: an empty-state hint when the
 *  selected resource types offer no attribute keys, else the editable
 *  key/value condition rows. Hoisted to module scope (no hooks needed — pure
 *  props → JSX) so its ternary/`&&` chains don't count against the main
 *  component's cognitive complexity. */
const AttributeConditionsSection = ({
  t, hasResourceTypeScope, conditionKeys, conditionRows, idEntityOptionsByKey,
  onKeyChange, onValueChange, onRemoveRow, onAddRow,
}: {
  t: TFunction;
  hasResourceTypeScope: boolean;
  conditionKeys: string[];
  conditionRows: AttributeConditionRow[];
  idEntityOptionsByKey: Record<string, EntityOption[]>;
  onKeyChange: (id: string, key: string) => void;
  onValueChange: (id: string, value: string) => void;
  onRemoveRow: (id: string) => void;
  onAddRow: () => void;
}) => {
  if (conditionKeys.length === 0 && conditionRows.length === 0) {
    return (
      <p className="flex items-center gap-1.5 text-xs italic text-muted-foreground">
        <SlidersHorizontal className="size-4 shrink-0" />
        {hasResourceTypeScope
          ? t("accessControl.policies.constraintsUnavailable")
          : t("accessControl.policies.constraintsSelectResourceType")}
      </p>
    );
  }
  return (
    <div className="space-y-2">
      {conditionRows.map((row) => {
        const valueKind = constraintValueKind(row.key);
        // Keep the row's own key selectable even if the resource-type
        // selection later stops offering it (so it stays editable/removable).
        const keyOptions = conditionKeys.includes(row.key) ? conditionKeys : [row.key, ...conditionKeys];
        const baseIdItems = idEntityOptionsByKey[row.key] ?? [];
        // Surface a stored id that isn't in the loaded list (e.g. a
        // deleted entity) so the value remains visible, not blanked.
        const idItems = row.value === "" || baseIdItems.some((o) => o.id === row.value)
          ? baseIdItems
          : [...baseIdItems, { id: row.value, label: row.value }];
        return (
          <div key={row.id} className="flex flex-wrap items-end gap-2">
            <div className="w-40 space-y-1">
              <FormSelect<string>
                label={t("accessControl.policies.constraintKey")}
                labelClassName="text-xs text-muted-foreground"
                value={row.key}
                onValueChange={(value) => onKeyChange(row.id, value)}
                options={keyOptions.map((k) => ({ value: k, label: constraintKeyLabel(t, k) }))}
                triggerClassName="h-9 text-sm"
              />
            </div>
            <div className="min-w-[160px] flex-1 space-y-1">
              <FieldLabel htmlFor={`policy-condition-${row.id}`} className="text-sm font-medium">{t("accessControl.policies.constraintValue")}</FieldLabel>
              {valueKind === "entity" && (
                <EntitySelect
                  id={`policy-condition-${row.id}`}
                  items={idItems}
                  value={row.value}
                  onValueChange={(value) => onValueChange(row.id, value)}
                  placeholder={t("accessControl.policies.selectValue")}
                  triggerClassName="h-9 text-sm"
                />
              )}
              {valueKind === "protocol" && (
                <FormSelect<string>
                  id={`policy-condition-${row.id}`}
                  value={row.value}
                  onValueChange={(value) => onValueChange(row.id, value)}
                  placeholder={t("accessControl.policies.selectValue")}
                  aria-label={t("accessControl.policies.constraintValue")}
                  options={PROTOCOL_VALUES.map((p) => ({ value: p, label: p.toUpperCase() }))}
                  triggerClassName="h-9 text-sm"
                />
              )}
              {valueKind === "text" && (
                <Input
                  id={`policy-condition-${row.id}`}
                  value={row.value}
                  onChange={(e) => onValueChange(row.id, e.target.value)}
                  placeholder={t("accessControl.policies.constraintValuePlaceholder")}
                  aria-label={t("accessControl.policies.constraintValue")}
                />
              )}
            </div>
            <Button
              variant="ghost"
              size="icon-sm"
              className="shrink-0 text-muted-foreground hover:text-destructive"
              aria-label={t("delete")}
              onClick={() => onRemoveRow(row.id)}
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        );
      })}
      {conditionRows.length === 0 && (
        <p className="text-xs italic text-muted-foreground">{t("accessControl.policies.noConditions")}</p>
      )}
      {conditionKeys.length > 0 && (
        <Button type="button" variant="outline" size="sm" className="gap-1" onClick={onAddRow}>
          <Plus /> {t("accessControl.policies.addCondition")}
        </Button>
      )}
    </div>
  );
};

const PolicyPreview = ({ effect, enabled, selectedSubjects, selectedActions, selectedScopes, constraintSummary, labels, renderScope, t }: {
  effect: "allow" | "deny";
  enabled: boolean;
  selectedSubjects: SubjectRule[];
  selectedActions: string[];
  selectedScopes: ScopeRule[];
  constraintSummary: string[];
  labels: ReturnType<typeof useAccessLabels>;
  /** Resolves a scope to its friendly label (named secret key for `resource`
   *  scopes, localized resource-type otherwise). Passed from the parent so the
   *  secret-name cache is shared, not re-fetched per sub-component. */
  renderScope: (type: ScopeRule["type"], value: string) => string;
  t: TFunction;
}) => (
  <div className="text-xs leading-relaxed text-foreground">
    <span className={cn("font-bold", effect === "allow" ? "text-status-positive" : "text-status-negative")}>
      {effect.toUpperCase()}
    </span>
    {" "}
    {selectedSubjects.length > 0
      ? selectedSubjects.map((s, i) => (
          <span key={s.value}>
            {i > 0 && ", "}
            <span className="font-mono text-xs font-medium">{s.kind}:<EntityRef {...labels.policySubject(s)} /></span>
          </span>
        ))
      : <span className="text-muted-foreground">—</span>
    }
    {" "}
    {selectedActions.length > 0
      ? selectedActions.length <= 3
        ? selectedActions.map((a, i) => (
            <span key={a}>
              {i > 0 && ", "}
              <span className="font-mono text-xs font-medium">{a}</span>
            </span>
          ))
        : <span className="font-mono text-xs font-medium">{selectedActions.length} {t("actions").toLowerCase()}</span>
      : <span className="text-muted-foreground">…</span>
    }
    {selectedScopes.length > 0 && (
      <>
        {" "}
        {selectedScopes.map((s, i) => (
          <span key={s.id}>
            {i > 0 && ", "}
            <span className="font-mono text-xs" title={`${s.type}:${s.value}`}>
              {scopeTypeLabel(t, s.type)}:{renderScope(s.type, s.value)}
            </span>
          </span>
        ))}
      </>
    )}
    {constraintSummary.length > 0 && (
      <>
        {" "}
        <span className="text-foreground/70">{t("accessControl.policies.constraintPreviewWhere")}</span>
        {" "}
        {constraintSummary.map((clause, i) => (
          <span key={clause}>
            {i > 0 && <span className="text-muted-foreground"> {t("accessControl.policies.constraintPreviewAnd")} </span>}
            <span className="font-mono text-xs">{clause}</span>
          </span>
        ))}
      </>
    )}
    {!enabled && <span className="ml-1 text-status-warning">({t("disabled").toLowerCase()})</span>}
  </div>
);

export default AccessPolicies;
