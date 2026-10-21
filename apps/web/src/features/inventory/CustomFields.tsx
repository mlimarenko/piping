import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Pencil, Trash2, Tags, Lock, ListChecks } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { RequireCapability } from "@/shared/auth/permissions";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageLayout from "@/shared/ui-shared/PageLayout";
import RowActions from "@/shared/ui-shared/RowActions";
import { useInventoryRowActionFocus } from "./useInventoryRowActionFocus";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { useServerTable } from "@/shared/ui-shared/useServerTable";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import EmptyState from "@/shared/ui-shared/EmptyState";
import SearchInput from "@/shared/ui-shared/SearchInput";
import StatusChip from "@/shared/ui-shared/StatusChip";
import { Button } from "@/shared/ui-primitives/button";
import { Badge } from "@/shared/ui-primitives/badge";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel,
  AlertDialogContent, AlertDialogDescription, AlertDialogFooter,
  AlertDialogHeader, AlertDialogTitle,
} from "@/shared/ui-primitives/alert-dialog";
import {
  createChoiceSet,
  createCustomField,
  deleteChoiceSet,
  deleteCustomField,
  FIELD_TYPES,
  listContentTypes,
  listChoiceSets,
  listCustomFields,
  updateChoiceSet,
  updateCustomField,
  type ChoiceSet,
  type ChoiceSetCreatePayload,
  type ChoiceSetUpdatePayload,
  type ChoiceSetListParams,
  type ContentType,
  type CustomFieldDefinition,
  type CustomFieldDefinitionCreatePayload,
  type CustomFieldDefinitionListParams,
  type CustomFieldDefinitionUpdatePayload,
} from "@/features/inventory/custom-fields-api";
import {
  CUSTOM_FIELD_CHOICE_SETS_TABLE_CONFIG,
  CUSTOM_FIELD_DEFINITIONS_TABLE_CONFIG,
} from "@/features/inventory/customFieldsTableConfig";
import CustomFieldDefinitionEditor from "@/features/inventory/components/custom-fields/CustomFieldDefinitionEditor";
import ChoiceSetEditor from "@/features/inventory/components/custom-fields/ChoiceSetEditor";

const CUSTOM_FIELDS_KEY = ["custom-fields"] as const;
const CHOICE_SETS_KEY = ["custom-field-choice-sets"] as const;
const CONTENT_TYPES_KEY = ["inventory-content-types"] as const;

function buildContentTypeLabel(type: ContentType): string {
  return `${type.label} · ${type.token}`;
}

type CustomFieldsTab = "definitions" | "choice-sets";

const CustomFieldsBody = ({ tab, onTabChange }: { tab: CustomFieldsTab; onTabChange: (tab: CustomFieldsTab) => void }) => {
  const { triggerProps, restoreFocus } = useInventoryRowActionFocus();
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const canCreate = can("CREATE", "INVENTORY");
  const canUpdate = can("UPDATE", "INVENTORY");
  const canDelete = can("DELETE", "INVENTORY");

  const defTable = useServerTable<CustomFieldDefinition, CustomFieldDefinitionListParams>({
    listFn: listCustomFields,
    queryKey: CUSTOM_FIELDS_KEY,
    sortKeys: CUSTOM_FIELD_DEFINITIONS_TABLE_CONFIG.sortKeys,
    filterColumns: CUSTOM_FIELD_DEFINITIONS_TABLE_CONFIG.filterColumns,
  });
  const choiceSetsTable = useServerTable<ChoiceSet, ChoiceSetListParams>({
    listFn: listChoiceSets,
    queryKey: CHOICE_SETS_KEY,
    sortKeys: CUSTOM_FIELD_CHOICE_SETS_TABLE_CONFIG.sortKeys,
  });
  const defsQuery = defTable.query;
  const choiceSetsQuery = choiceSetsTable.query;

  const choiceSetOptionsQuery = useQuery({
    queryKey: [...CHOICE_SETS_KEY, "options"],
    queryFn: () =>
      listChoiceSets({
        limit: 1000,
        offset: 0,
        sort: "name",
        order: "asc",
      }),
  });
  const contentTypesQuery = useQuery({
    queryKey: CONTENT_TYPES_KEY,
    queryFn: () => listContentTypes(),
  });

  const definitions = useMemo(() => defsQuery.data?.items ?? [], [defsQuery.data]);
  const totalDefinitions = defsQuery.data?.total ?? 0;
  const choiceSets = useMemo(() => choiceSetsQuery.data?.items ?? [], [choiceSetsQuery.data]);
  const totalChoiceSets = choiceSetsQuery.data?.total ?? 0;
  const choiceSetOptions = useMemo(
    () => choiceSetOptionsQuery.data?.items ?? [],
    [choiceSetOptionsQuery.data],
  );
  const contentTypes = useMemo(() => contentTypesQuery.data ?? [], [contentTypesQuery.data]);
  const contentTypeLabelById = useMemo(() => {
    const labels = new Map<string, string>();
    for (const type of contentTypes) labels.set(type.id, buildContentTypeLabel(type));
    return labels;
  }, [contentTypes]);
  const choiceSetNameById = useMemo(() => {
    const labels = new Map<string, string>();
    for (const choiceSet of choiceSetOptions) labels.set(choiceSet.id, choiceSet.name);
    return labels;
  }, [choiceSetOptions]);

  // Dialog state
  const [defDialogOpen, setDefDialogOpen] = useState(false);
  const [editingDef, setEditingDef] = useState<CustomFieldDefinition | undefined>(undefined);
  const [csDialogOpen, setCsDialogOpen] = useState(false);
  const [editingCs, setEditingCs] = useState<ChoiceSet | undefined>(undefined);
  const [deleteDef, setDeleteDef] = useState<CustomFieldDefinition | null>(null);
  const [deleteCs, setDeleteCs] = useState<ChoiceSet | null>(null);
  const choiceSetUsageQuery = useQuery({
    queryKey: [...CUSTOM_FIELDS_KEY, "choice-set-usage", deleteCs?.id ?? null],
    enabled: Boolean(deleteCs?.id),
    queryFn: async () =>
      listCustomFields({
        limit: 1,
        offset: 0,
        choice_set_id: deleteCs?.id,
      }),
  });

  const onError = (err: ApiError) => notify("error", err.message || t("unknownError"));
  const invalidateDefs = () => qc.invalidateQueries({ queryKey: CUSTOM_FIELDS_KEY });
  const invalidateCs = () => qc.invalidateQueries({ queryKey: CHOICE_SETS_KEY });

  // ── Definition mutations ──
  const createDefMutation = useApiMutation({
    mutationFn: (body: CustomFieldDefinitionCreatePayload) => createCustomField(body),
    onSuccess: () => { void invalidateDefs(); notify("success", t("customFields.def.created")); },
    onError,
  });
  const updateDefMutation = useApiMutation({
    mutationFn: ({ id, body }: { id: string; body: CustomFieldDefinitionUpdatePayload }) =>
      updateCustomField(id, body),
    onSuccess: () => { void invalidateDefs(); notify("success", t("customFields.def.updated")); },
    onError,
  });
  const deleteDefMutation = useApiMutation({
    mutationFn: (id: string) => deleteCustomField(id),
    onSuccess: () => { void invalidateDefs(); notify("success", t("customFields.def.deleted")); },
    onError,
  });

  // ── Choice-set mutations ──
  const createCsMutation = useApiMutation({
    mutationFn: (body: ChoiceSetCreatePayload) => createChoiceSet(body),
    onSuccess: () => { void invalidateCs(); notify("success", t("customFields.choiceSet.created")); },
    onError,
  });
  const updateCsMutation = useApiMutation({
    mutationFn: ({ id, body }: { id: string; body: ChoiceSetUpdatePayload }) => updateChoiceSet(id, body),
    onSuccess: () => { void invalidateCs(); notify("success", t("customFields.choiceSet.updated")); },
    onError,
  });
  const deleteCsMutation = useApiMutation({
    mutationFn: (id: string) => deleteChoiceSet(id),
    onSuccess: () => { void invalidateCs(); notify("success", t("customFields.choiceSet.deleted")); },
    onError,
  });

  const choiceSetName = (id: string | null | undefined) =>
    choiceSetOptions.find((c) => c.id === id)?.name ?? "";

  const defFilterOptions = useMemo(
    () => ({
      field_type: FIELD_TYPES.map((value) => ({
        value,
        label: value,
        count: totalDefinitions > 0 ? undefined : 0,
      })),
      group_name: Array.from(
        new Set(definitions.map((definition) => definition.group_name).filter(Boolean)),
      )
        .sort((a, b) => a.localeCompare(b))
        .map((value) => ({ value, label: value })),
      ui_visible: ["always", "if-set", "hidden"].map((value) => ({ value, label: value })),
      ui_editable: ["yes", "no", "hidden"].map((value) => ({ value, label: value })),
      filter_logic: ["disabled", "loose", "exact"].map((value) => ({ value, label: value })),
      choice_set: choiceSetOptions.map((choiceSet) => ({
        value: choiceSet.id,
        label: choiceSet.name,
      })),
      related_object_type: contentTypes.map((type) => ({
        value: type.id,
        label: buildContentTypeLabel(type),
      })),
      scope: contentTypes
        .map((type) => ({
          value: type.id,
          label: buildContentTypeLabel(type),
        })),
    }),
    [choiceSetOptions, contentTypes, definitions, totalDefinitions],
  );

  // ── Definition columns ──
  const defColumns = useMemo<Column<CustomFieldDefinition>[]>(() => [
    {
      key: "label",
      label: t("customFields.columns.field"),
      sortable: true,
      getSortValue: (d) => d.label,
      render: (d) => (
        <div className="min-w-0">
          <span className="font-medium text-foreground block truncate">{d.label}</span>
        </div>
      ),
    },
    {
      key: "name",
      label: t("name"),
      sortable: true,
      getSortValue: (d) => d.name,
      width: "180px",
      render: (d) => (
        <span className="block truncate font-mono text-xs text-muted-foreground">{d.name}</span>
      ),
    },
    {
      key: "field_type",
      label: t("type"),
      sortable: true,
      filterOptions: defFilterOptions.field_type.map((option) => ({
        ...option,
        label: t(`customFields.fieldTypes.${option.value}`, { defaultValue: option.label }),
      })),
      getSortValue: (d) => d.field_type,
      width: "130px",
      render: (d) => (
        <Badge variant="outline" className="text-xs h-5 font-normal">
          {t(`customFields.fieldTypes.${d.field_type}`, { defaultValue: d.field_type })}
        </Badge>
      ),
    },
    {
      key: "group_name",
      label: t("customFields.columns.group"),
      sortable: true,
      filterOptions: defFilterOptions.group_name,
      getSortValue: (d) => d.group_name,
      width: "140px",
      render: (d) =>
        d.group_name ? (
          <span className="text-xs text-muted-foreground truncate">{d.group_name}</span>
        ) : (
          <span className="text-xs text-muted-foreground">—</span>
        ),
    },
    {
      key: "weight",
      label: t("customFields.columns.weight", { defaultValue: "Weight" }),
      sortable: true,
      getSortValue: (d) => d.weight,
      width: "90px",
      render: (d) => <span className="text-xs text-muted-foreground tabular-nums">{d.weight}</span>,
    },
    {
      key: "ui_visible",
      label: t("customFields.columns.uiVisible", { defaultValue: "Visible" }),
      sortable: true,
      filterOptions: defFilterOptions.ui_visible,
      getSortValue: (d) => d.ui_visible,
      width: "110px",
      render: (d) => (
        <Badge variant="outline" className="h-5 text-xs font-normal">
          {d.ui_visible}
        </Badge>
      ),
    },
    {
      key: "ui_editable",
      label: t("customFields.columns.uiEditable", { defaultValue: "Editable" }),
      sortable: true,
      filterOptions: defFilterOptions.ui_editable,
      getSortValue: (d) => d.ui_editable,
      width: "110px",
      render: (d) => (
        <Badge variant="outline" className="h-5 text-xs font-normal">
          {d.ui_editable}
        </Badge>
      ),
    },
    {
      key: "filter_logic",
      label: t("customFields.columns.filterLogic", { defaultValue: "Filter" }),
      sortable: true,
      filterOptions: defFilterOptions.filter_logic,
      getSortValue: (d) => d.filter_logic,
      width: "110px",
      render: (d) => (
        <Badge
          variant={d.filter_logic === "disabled" ? "secondary" : "outline"}
          className="h-5 text-xs font-normal"
        >
          {d.filter_logic}
        </Badge>
      ),
    },
    {
      key: "choice_set",
      label: t("customFields.columns.choiceSet", { defaultValue: "Choices" }),
      sortable: true,
      filterOptions: defFilterOptions.choice_set,
      getSortValue: (d) => choiceSetNameById.get(d.choice_set_id ?? "") ?? "",
      width: "150px",
      render: (d) =>
        d.choice_set_id ? (
          <span className="text-xs text-muted-foreground truncate">
            {choiceSetNameById.get(d.choice_set_id) ?? d.choice_set_id}
          </span>
        ) : (
          <span className="text-xs text-muted-foreground">—</span>
        ),
    },
    {
      key: "related_object_type",
      label: t("customFields.columns.relatedObjectType", { defaultValue: "Related type" }),
      sortable: true,
      filterOptions: defFilterOptions.related_object_type,
      getSortValue: (d) => contentTypeLabelById.get(d.related_object_type_id ?? "") ?? "",
      width: "170px",
      render: (d) =>
        d.related_object_type_id ? (
          <span className="text-xs text-muted-foreground truncate">
            {contentTypeLabelById.get(d.related_object_type_id) ?? d.related_object_type_id}
          </span>
        ) : (
          <span className="text-xs text-muted-foreground">—</span>
        ),
    },
    {
      key: "scope",
      label: t("customFields.columns.scope"),
      sortable: true,
      filterOptions: defFilterOptions.scope,
      getSortValue: (d) => d.object_types.length,
      width: "110px",
      render: (d) => (
        <div className="flex max-w-[240px] flex-wrap gap-1">
          {d.object_types.length === 0 ? (
            <span className="text-xs text-muted-foreground">—</span>
          ) : (
            <>
              {d.object_types.slice(0, 2).map((id) => (
                <Badge key={id} variant="outline" className="h-5 max-w-[150px] truncate text-xs font-normal">
                  {contentTypeLabelById.get(id) ?? t("customFields.columns.unknownScope")}
                </Badge>
              ))}
              {d.object_types.length > 2 && (
                <Badge variant="secondary" className="h-5 text-xs font-normal">
                  {t("customFields.columns.moreScope", { count: d.object_types.length - 2 })}
                </Badge>
              )}
            </>
          )}
        </div>
      ),
    },
    {
      key: "flags",
      label: t("customFields.columns.flags"),
      sortable: true,
      getSortValue: (d) => `${d.is_secret ? "1" : "0"}${d.required ? "1" : "0"}`,
      width: "150px",
      render: (d) => (
        <div className="flex flex-wrap gap-1">
          {d.is_secret && (
            <StatusChip status="warning" label={t("customFields.flags.secret")} dot={false} />
          )}
          {d.required && (
            <StatusChip status="info" label={t("customFields.flags.required")} dot={false} />
          )}
        </div>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      width: "88px",
      render: (d) => canUpdate || canDelete ? (
        <RowActions label={`${t("actions")}: ${d.label || d.name}`} {...triggerProps(d.id)}>
          {canUpdate && <DropdownMenuItem onSelect={() => { restoreFocus(); openEditDef(d); }}><Pencil className="size-4" />{t("edit")}</DropdownMenuItem>}
          {canDelete && <DropdownMenuItem variant="destructive" onSelect={() => setDeleteDef(d)}><Trash2 className="size-4" />{t("delete")}</DropdownMenuItem>}
        </RowActions>
      ) : null,
    },
  ], [canDelete, canUpdate, choiceSetNameById, contentTypeLabelById, defFilterOptions, restoreFocus, triggerProps, t]);

  // ── Choice-set columns ──
  const csColumns = useMemo<Column<ChoiceSet>[]>(() => [
    {
      key: "name",
      label: t("name"),
      sortable: true,
      getSortValue: (c) => c.name,
      render: (c) => (
        <div className="min-w-0">
          <span className="font-medium text-foreground block truncate">{c.name}</span>
          {c.description && (
            <p className="text-xs text-muted-foreground max-w-[320px] whitespace-pre-wrap break-words leading-tight mt-0.5">{c.description}</p>
          )}
        </div>
      ),
    },
    {
      key: "choices",
      label: t("customFields.choiceSet.choices"),
      sortable: true,
      getSortValue: (c) => c.extra_choices.length,
      width: "140px",
      render: (c) => (
        <Badge variant="outline" className="text-xs h-5 font-normal">
          {t("customFields.choiceSet.choicesCount", { count: c.extra_choices.length })}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      width: "88px",
      render: (c) => canUpdate || canDelete ? (
        <RowActions label={`${t("actions")}: ${c.name}`} {...triggerProps(c.id)}>
          {canUpdate && <DropdownMenuItem onSelect={() => { restoreFocus(); openEditCs(c); }}><Pencil className="size-4" />{t("edit")}</DropdownMenuItem>}
          {canDelete && <DropdownMenuItem variant="destructive" onSelect={() => setDeleteCs(c)}><Trash2 className="size-4" />{t("delete")}</DropdownMenuItem>}
        </RowActions>
      ) : null,
    },
  ], [canDelete, canUpdate, restoreFocus, triggerProps, t]);

  const openCreateDef = () => { setEditingDef(undefined); setDefDialogOpen(true); };
  const openEditDef = (d: CustomFieldDefinition) => { setEditingDef(d); setDefDialogOpen(true); };
  const openCreateCs = () => { setEditingCs(undefined); setCsDialogOpen(true); };
  const openEditCs = (c: ChoiceSet) => { setEditingCs(c); setCsDialogOpen(true); };

  return (
    <>
      <PageChrome
        title={t("customFields.title")}
        description={t("customFields.description")}
        icon={Tags}
        badge={defsQuery.isSuccess ? String(totalDefinitions) : undefined}
        actions={
          !canCreate ? undefined : tab === "definitions" ? (
            <Button size="sm" className="h-9 text-sm gap-1.5" onClick={openCreateDef}>
              <Plus className="h-3 w-3" /> {t("customFields.def.create")}
            </Button>
          ) : (
            <Button size="sm" className="h-9 text-sm gap-1.5" onClick={openCreateCs}>
              <Plus className="h-3 w-3" /> {t("customFields.choiceSet.create")}
            </Button>
          )
        }
      />

      <Tabs value={tab} onValueChange={(v) => onTabChange(v as CustomFieldsTab)}>
        <SectionTabs
          items={[
            { value: "definitions", label: t("customFields.tabs.definitions"), icon: Lock },
            { value: "choice-sets", label: t("customFields.tabs.choiceSets"), icon: ListChecks },
          ] satisfies Array<{ value: typeof tab; label: string; icon: typeof Lock }>}
          className="surface-chip h-9"
        />

        <TabsContent value="definitions" className="mt-4">
          <DataTable
            tableId="inventory.customFields.definitions"
            columns={defColumns}
            data={definitions}
            getRowKey={(d) => d.id}
            sortState={defTable.sortState}
            onSortChange={defTable.setSortState}
            columnFilters={defTable.columnFilters}
            onColumnFilterChange={defTable.setColumnFilter}
            onRowClick={(d) => openEditDef(d)}
            loading={defsQuery.isPending}
            error={defsQuery.error}
            onRetry={defTable.refresh}
            pagination={{
              page: defTable.page,
              pageSize: defTable.pageSize,
              total: totalDefinitions,
              onPageChange: defTable.setPage,
              onPageSizeChange: defTable.setPageSize,
            }}
            emptyState={
              <EmptyState
                icon={Tags}
                title={defTable.search ? t("customFields.noResults") : t("customFields.def.empty")}
                description={defTable.search ? undefined : t("customFields.def.emptyHint")}
                actionLabel={defTable.search || !canCreate ? undefined : t("customFields.def.create")}
                onAction={defTable.search || !canCreate ? undefined : openCreateDef}
              />
            }
            toolbar={
              <SearchInput
                containerClassName="flex-1 max-w-xs"
                value={defTable.search}
                onValueChange={defTable.setSearch}
                placeholder={t("customFields.def.searchPlaceholder")}
              />
            }
          />
        </TabsContent>

        <TabsContent value="choice-sets" className="mt-4">
          <DataTable
            tableId="inventory.customFields.choiceSets"
            columns={csColumns}
            data={choiceSets}
            getRowKey={(c) => c.id}
            sortState={choiceSetsTable.sortState}
            onSortChange={choiceSetsTable.setSortState}
            onRowClick={(c) => openEditCs(c)}
            loading={choiceSetsQuery.isPending}
            error={choiceSetsQuery.error}
            onRetry={choiceSetsTable.refresh}
            pagination={{
              page: choiceSetsTable.page,
              pageSize: choiceSetsTable.pageSize,
              total: totalChoiceSets,
              onPageChange: choiceSetsTable.setPage,
              onPageSizeChange: choiceSetsTable.setPageSize,
            }}
            emptyState={
              <EmptyState
                icon={ListChecks}
                title={choiceSetsTable.search ? t("customFields.noResults") : t("customFields.choiceSet.empty")}
                description={choiceSetsTable.search ? undefined : t("customFields.choiceSet.emptyHint")}
                actionLabel={choiceSetsTable.search || !canCreate ? undefined : t("customFields.choiceSet.create")}
                onAction={choiceSetsTable.search || !canCreate ? undefined : openCreateCs}
              />
            }
            toolbar={
              <SearchInput
                containerClassName="flex-1 max-w-xs"
                value={choiceSetsTable.search}
                onValueChange={choiceSetsTable.setSearch}
                placeholder={t("customFields.choiceSet.searchPlaceholder")}
              />
            }
          />
        </TabsContent>
      </Tabs>

      {defDialogOpen && (
        <CustomFieldDefinitionEditor
          open={defDialogOpen}
          onOpenChange={setDefDialogOpen}
          editing={editingDef}
          choiceSets={choiceSetOptions}
          contentTypes={contentTypes}
          contentTypesLoading={contentTypesQuery.isPending}
          contentTypesError={contentTypesQuery.isError}
          saving={createDefMutation.isPending || updateDefMutation.isPending}
          onCreate={(body) => createDefMutation.mutate(body)}
          onUpdate={(id, body) => updateDefMutation.mutate({ id, body })}
        />
      )}

      {csDialogOpen && (
        <ChoiceSetEditor
          open={csDialogOpen}
          onOpenChange={setCsDialogOpen}
          editing={editingCs}
          saving={createCsMutation.isPending || updateCsMutation.isPending}
          onCreate={(body) => createCsMutation.mutate(body)}
          onUpdate={(id, body) => updateCsMutation.mutate({ id, body })}
        />
      )}

      {deleteDef && (
        <AlertDialog open={!!deleteDef} onOpenChange={(o) => { if (!o) setDeleteDef(null); }}>
          <AlertDialogContent onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("customFields.def.deleteTitle")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("customFields.def.deleteConfirm", { name: deleteDef.label })}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
              <AlertDialogAction
                className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                onClick={() => { deleteDefMutation.mutate(deleteDef.id); setDeleteDef(null); }}
              >
                {t("delete")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}

      {deleteCs && (
        <AlertDialog open={!!deleteCs} onOpenChange={(o) => { if (!o) setDeleteCs(null); }}>
          <AlertDialogContent onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("customFields.choiceSet.deleteTitle")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("customFields.choiceSet.deleteConfirm", { name: deleteCs.name })}
                {(choiceSetUsageQuery.data?.total ?? 0) > 0 && (
                  <span className="mt-2 block text-status-warning">
                    {t("customFields.choiceSet.deleteInUse", { name: choiceSetName(deleteCs.id) })}
                  </span>
                )}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
              <AlertDialogAction
                className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                onClick={() => { deleteCsMutation.mutate(deleteCs.id); setDeleteCs(null); }}
              >
                {t("delete")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
    </>
  );
};

const CustomFields = () => {
  const { can } = usePermissions();
  const canRead = can("READ", "INVENTORY");
  const [tab, setTab] = useState<CustomFieldsTab>("definitions");
  return <PageLayout width={canRead && tab === "definitions" ? "full" : "standard"} fill className="flex flex-col gap-4">
    <RequireCapability check={() => canRead}>
      <CustomFieldsBody tab={tab} onTabChange={setTab} />
    </RequireCapability>
  </PageLayout>;
};

export default CustomFields;
