import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import {
  createContext as apiCreateContext,
  deleteContext as apiDeleteContext,
  listContexts as apiListContexts,
  updateContext as apiUpdateContext,
} from "@/features/inventory/config-contexts-api";
import type { ApiError } from "@/shared/api/client";
import type { ConfigContext } from "@/shared/api/types";
import { notify } from "@/shared/lib/notifications";
import { cn } from "@/shared/lib/utils";
import FormSection from "@/shared/ui-shared/FormSection";
import { PageActions } from "@/shared/ui-shared/PageChrome";
import RowActions from "@/shared/ui-shared/RowActions";
import { useInventoryRowActionFocus } from "./useInventoryRowActionFocus";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import StatusChip from "@/shared/ui-shared/StatusChip";
import EmptyState from "@/shared/ui-shared/EmptyState";
import SearchInput from "@/shared/ui-shared/SearchInput";
import DataTable, {
  type Column,
  type DataTableSortState,
} from "@/shared/ui-shared/DataTable";
import { boolFilterOptions } from "@/shared/ui-shared/dataTableLocal";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";
import {
  buildConfigContextsFilterParams,
  CONFIG_CONTEXTS_TABLE_CONFIG,
  configContextsSortParam,
} from "@/features/inventory/configContextsTableConfig";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import ConfigContextScopeBindings from "@/features/inventory/components/config-contexts/ConfigContextScopeBindings";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Switch } from "@/shared/ui-primitives/switch";
import { Label } from "@/shared/ui-primitives/label";
import {
  Plus, Trash2, Layers, AlertCircle, CheckCircle2, Pencil,
} from "lucide-react";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel,
  AlertDialogContent, AlertDialogDescription, AlertDialogFooter,
  AlertDialogHeader, AlertDialogTitle,
} from "@/shared/ui-primitives/alert-dialog";

/* ── Types ── */
/** A blank draft for the create dialog. */
const EMPTY_DRAFT: Omit<ConfigContext, "id"> = {
  name: "",
  description: "",
  weight: 100,
  active: true,
  data: "{\n  \n}",
};

/** Try to parse and prettify JSON; returns null on invalid JSON. */
function tryPrettyJson(raw: string): string | null {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return null;
  }
}

/** Check if a string is valid JSON. */
function isValidJson(raw: string): boolean {
  try {
    JSON.parse(raw);
    return true;
  } catch {
    return false;
  }
}

const ConfigContexts = () => {
  const { triggerProps, restoreFocus } = useInventoryRowActionFocus();
  const { t } = useTranslation();
  const qc = useQueryClient();

  const [searchQuery, setSearchQuery] = useState("");
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});
  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, CONFIG_CONTEXTS_TABLE_CONFIG.sortKeys),
    [sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        columnFilters,
        CONFIG_CONTEXTS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [columnFilters],
  );
  const filterParams = useMemo(
    () => buildConfigContextsFilterParams(sanitizedColumnFilters),
    [sanitizedColumnFilters],
  );
  const contextsQuery = useQuery({
    queryKey: ["config-contexts", "list", { searchQuery, sortState, filterParams }],
    queryFn: () =>
      apiListContexts({
        limit: 1000,
        offset: 0,
        name: searchQuery.trim() || undefined,
        isActive: filterParams.is_active,
        sort: configContextsSortParam(sanitizedSortState?.key),
        order: sanitizedSortState?.dir,
      }),
  });
  const contexts = useMemo(() => contextsQuery.data?.items ?? [], [contextsQuery.data]);
  // `null` = closed; `{ id: undefined }` is never used — `editing` carries the
  // full row when editing and `undefined` when creating, while `dialogOpen`
  // gates visibility so the form keeps its draft during the close animation.
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<ConfigContext | undefined>(undefined);
  const [deleteTarget, setDeleteTarget] = useState<ConfigContext | null>(null);

  // View-but-not-edit: a user with READ but not the mutating capability sees the
  // list read-only — create/edit/delete affordances are hidden, not 403-on-click.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "CONFIG_CONTEXT");
  const canUpdate = can("UPDATE", "CONFIG_CONTEXT");
  const canDelete = can("DELETE", "CONFIG_CONTEXT");

  const invalidate = () => qc.invalidateQueries({ queryKey: ["config-contexts"] });

  const createMutation = useApiMutation({
    mutationFn: (body: Omit<ConfigContext, "id">) => apiCreateContext(body),
    onSuccess: (created) => {
      void invalidate();
      notify("success", t("configContexts.contextCreated", { name: created.name }));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const updateMutation = useApiMutation({
    mutationFn: ({ id, body }: { id: string; body: Partial<ConfigContext> }) =>
      apiUpdateContext(id, body),
    onSuccess: () => {
      void invalidate();
      notify("success", t("configContexts.contextUpdated"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deleteMutation = useApiMutation({
    mutationFn: (id: string) => apiDeleteContext(id),
    onSuccess: () => {
      void invalidate();
      notify("success", t("configContexts.contextDeleted"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const openCreate = () => {
    setEditing(undefined);
    setDialogOpen(true);
  };
  const openEdit = (ctx: ConfigContext) => {
    setEditing(ctx);
    setDialogOpen(true);
  };

  const handleColumnFilterChange = (key: string, values: string[]) => {
    setColumnFilters((current) => updateDataTableColumnFilters(current, key, values));
  };

  const columns: Column<ConfigContext>[] = [
    {
      key: "name",
      label: t("name"),
      sortable: true,
      render: (c) => (
        <div className="min-w-0">
          <span className="font-medium text-foreground block truncate">{c.name}</span>
          {c.description && (
            <p className="text-xs text-muted-foreground truncate max-w-[320px] leading-tight mt-0.5">
              {c.description}
            </p>
          )}
        </div>
      ),
    },
    {
      key: "weight",
      label: t("configContexts.weight"),
      sortable: true,
      width: "100px",
      getSortValue: (c) => c.weight,
      render: (c) => (
        <span className="surface-chip rounded px-1.5 py-0.5 text-xs font-mono tabular-nums text-muted-foreground">
          {c.weight}
        </span>
      ),
    },
    {
      key: "status",
      label: t("status"),
      sortable: true,
      filterOptions: boolFilterOptions({
        trueLabel: t("configContexts.active"),
        falseLabel: t("configContexts.inactive"),
      }),
      width: "120px",
      render: (c) => (
        <StatusChip
          status={c.active ? "positive" : "neutral"}
          label={c.active ? t("configContexts.active") : t("configContexts.inactive")}
        />
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      width: "88px",
      render: (c) => canUpdate || canDelete ? (
        <RowActions label={`${t("actions")}: ${c.name}`} {...triggerProps(c.id)}>
          {canUpdate && <DropdownMenuItem onSelect={() => { restoreFocus(); openEdit(c); }}><Pencil className="size-4" />{t("edit")}</DropdownMenuItem>}
          {canDelete && <DropdownMenuItem variant="destructive" onSelect={() => setDeleteTarget(c)}><Trash2 className="size-4" />{t("delete")}</DropdownMenuItem>}
        </RowActions>
      ) : null,
    },
  ];

  const content = (
    <div className="flex min-w-0 flex-col gap-4">
        <PageActions>{canCreate ? (
            <Button size="sm" className="h-9 text-sm gap-1.5" onClick={openCreate}>
              <Plus className="h-3 w-3" /> {t("configContexts.newContext")}
            </Button>
           ) : null}</PageActions>

      <DataTable
        tableId="inventory.config-contexts"
        columns={columns}
        data={contexts}
        getRowKey={(c) => c.id}
        onRowClick={(c) => openEdit(c)}
        loading={contextsQuery.isPending}
        error={contextsQuery.error}
        onRetry={() => contextsQuery.refetch()}
        sortState={sanitizedSortState}
        onSortChange={setSortState}
        columnFilters={sanitizedColumnFilters}
        onColumnFilterChange={handleColumnFilterChange}
        emptyState={
          <EmptyState
            icon={Layers}
            title={searchQuery ? t("configContexts.noResults") : t("configContexts.selectOrCreate")}
            description={searchQuery ? undefined : t("configContexts.emptyHint")}
            actionLabel={searchQuery || !canCreate ? undefined : t("configContexts.newContext")}
            onAction={searchQuery || !canCreate ? undefined : openCreate}
          />
        }
        toolbar={
          <SearchInput
            containerClassName="flex-1 max-w-xs"
            value={searchQuery}
            onValueChange={setSearchQuery}
            placeholder={t("configContexts.searchContexts")}
          />
        }
      />

      {dialogOpen && (
        <ConfigContextEditor
          open={dialogOpen}
          onOpenChange={setDialogOpen}
          editing={editing}
          saving={createMutation.isPending || updateMutation.isPending}
          onSubmit={(values) => {
            if (editing) {
              updateMutation.mutate({ id: editing.id, body: values });
            } else {
              createMutation.mutate(values);
            }
            setDialogOpen(false);
          }}
        />
      )}

      {deleteTarget && (
        <AlertDialog open={!!deleteTarget} onOpenChange={(o) => { if (!o) setDeleteTarget(null); }}>
          <AlertDialogContent onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("configContexts.deleteTitle")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("configContexts.deleteConfirm", { name: deleteTarget.name })}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
              <AlertDialogAction
                className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                onClick={() => { deleteMutation.mutate(deleteTarget.id); setDeleteTarget(null); }}
              >
                {t("delete")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
    </div>
  );
  return content;
};

/* ── Create / edit form ── */
interface ConfigContextEditorProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing?: ConfigContext;
  saving: boolean;
  onSubmit: (values: Omit<ConfigContext, "id">) => void;
}

const ConfigContextEditor = ({ open, onOpenChange, editing, saving, onSubmit }: ConfigContextEditorProps) => {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<Omit<ConfigContext, "id">>(() =>
    editing
      ? { name: editing.name, description: editing.description, weight: editing.weight, active: editing.active, data: editing.data }
      : { ...EMPTY_DRAFT },
  );
  const [nameError, setNameError] = useState("");

  const set = <K extends keyof Omit<ConfigContext, "id">>(key: K, value: Omit<ConfigContext, "id">[K]) =>
    setDraft((prev) => ({ ...prev, [key]: value }));

  const jsonValid = isValidJson(draft.data);
  const hasData = draft.data.trim().length > 0 && draft.data.trim() !== "{}";

  const handleSubmit = () => {
    if (!draft.name.trim()) {
      setNameError(t("accessControl.groups.errors.nameRequired"));
      return;
    }
    if (!jsonValid) return;
    onSubmit({ ...draft, name: draft.name.trim() });
  };

  return (
    <EntityEditorHost
      open={open}
      onOpenChange={onOpenChange}
      title={editing ? t("configContexts.editTitle") : t("configContexts.createTitle")}
      description={editing ? t("configContexts.editDescription") : t("configContexts.createDescription")}
      footer={
        <>
          <Button data-editor-cancel variant="outline" onClick={() => onOpenChange(false)}>
            {t("cancel")}
          </Button>
          <Button onClick={handleSubmit} disabled={saving || !jsonValid || !draft.name.trim()}>
            {editing ? t("saveChanges") : t("create")}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {/* Name */}
        <div className="space-y-1">
          <FieldLabel htmlFor="config-context-name" required>
            {t("name")}
          </FieldLabel>
          <Input
            id="config-context-name"
            className="h-9 text-sm"
            value={draft.name}
            onChange={(e) => { set("name", e.target.value); if (nameError) setNameError(""); }}
            placeholder={t("configContexts.namePlaceholder")}
            maxLength={100}
            autoFocus
          />
          {nameError && <p className="text-xs text-destructive">{nameError}</p>}
        </div>

        {/* Description */}
        <div className="space-y-1">
          <FieldLabel htmlFor="config-context-description">{t("description")}</FieldLabel>
          <Input
            id="config-context-description"
            className="h-9 text-sm"
            value={draft.description}
            onChange={(e) => set("description", e.target.value)}
            placeholder={t("configContexts.describe")}
            maxLength={255}
          />
        </div>

        {/* Weight + active */}
        <div className="grid grid-cols-[120px_1fr] gap-3 items-end">
          <div className="space-y-1">
            <FieldLabel htmlFor="config-context-weight">{t("configContexts.weight")}</FieldLabel>
            <Input
              id="config-context-weight"
              type="number"
              inputMode="numeric"
              className="h-9 text-sm font-mono tabular-nums"
              value={draft.weight}
              onChange={(e) => set("weight", Number(e.target.value) || 0)}
              aria-label={t("configContexts.weight")}
            />
          </div>
          <div className="flex items-center gap-2 h-9">
            <Switch
              id="config-context-active"
              checked={draft.active}
              onCheckedChange={(v) => set("active", v)}
            />
            <Label htmlFor="config-context-active" className="text-sm cursor-pointer">
              {draft.active ? t("configContexts.active") : t("configContexts.inactive")}
            </Label>
          </div>
        </div>

        {/* JSON data */}
        <FormSection title={t("configContexts.contextData")} divider action={
            <div className="flex items-center gap-2">
              {!jsonValid ? (
                <span className="inline-flex items-center gap-1 text-xs text-status-negative">
                  <AlertCircle className="h-3 w-3" />
                  {t("configContexts.invalidJson")}
                </span>
              ) : hasData ? (
                <CheckCircle2 className="h-3.5 w-3.5 text-status-positive/60" />
              ) : null}
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-9 text-sm text-muted-foreground hover:text-foreground"
                disabled={!jsonValid}
                onClick={() => {
                  const pretty = tryPrettyJson(draft.data);
                  if (pretty !== null) set("data", pretty);
                }}
              >
                {t("configContexts.formatJson")}
              </Button>
            </div>
          }>
          <Textarea
            className={cn(
              "font-mono text-sm min-h-[200px] border-border bg-background resize-y",
              !jsonValid && "border-status-negative/40 focus-visible:ring-status-negative/40",
            )}
            value={draft.data}
            onChange={(e) => set("data", e.target.value)}
            spellCheck={false}
            aria-label={t("configContexts.contextData")}
            aria-invalid={!jsonValid}
          />
        </FormSection>

        {/* Scope assignments — edit only (bindings need a persisted context id). */}
        {editing && (
          <div className="border-t border-border pt-4">
            <ConfigContextScopeBindings contextId={editing.id} />
          </div>
        )}
      </div>
    </EntityEditorHost>
  );
};

export default ConfigContexts;
