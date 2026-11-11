import { useEffect, useRef, useState, type ReactNode } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Network, Plus } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { ApiError } from "@/shared/api/client";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { Button } from "@/shared/ui-primitives/button";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/shared/ui-primitives/dropdown-menu";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageState from "@/shared/ui-shared/PageState";
import RowActions from "@/shared/ui-shared/RowActions";
import SearchInput from "@/shared/ui-shared/SearchInput";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { useCollectionPreviewFocus } from "@/shared/ui-shared/useCollectionPreviewFocus";
import ConfirmDeleteDialog from "@/shared/ui-shared/ConfirmDeleteDialog";
import { useInventoryRowActionFocus } from "./useInventoryRowActionFocus";
import DataflowCatalogNav, { DATAFLOW_CATALOG_ROUTES } from "./DataflowCatalogNav";
import CatalogDetailSheet, { type CatalogDetailNames, type CatalogSelection } from "./CatalogDetailSheet";
import { inventoryErrorMessage } from "./inventoryErrors";
import { inventoryResourceOptionsQuery } from "./resourceOptions";
import { deleteDataflowGroup, listApplications, listDataflowGroups } from "./dataflowCatalog-api";
import type { DataflowGroup } from "./dataflowCatalog-api";

const GROUP_NEW_PATH = `${DATAFLOW_CATALOG_ROUTES.groups}/new`;
const groupEditPath = (id: string) => `${DATAFLOW_CATALOG_ROUTES.groups}/${id}/edit`;
/** `parent=root` selects root groups; any other value is an exact parent id. */
const ROOT_PARENT = "root";

type TreeRow = { group: DataflowGroup; depth: number };

/** Deterministic name,id pre-order walk; unknown parents stay deliberately unrooted. */
function dataflowGroupTree(groups: DataflowGroup[]): TreeRow[] {
  const ids = new Set(groups.map(group => group.id));
  const children = new Map<string | null, DataflowGroup[]>();
  for (const group of groups) {
    const parent = group.parent_id && ids.has(group.parent_id) ? group.parent_id : null;
    const bucket = children.get(parent) ?? [];
    bucket.push(group);
    children.set(parent, bucket);
  }
  for (const bucket of children.values()) bucket.sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id));
  const rows: TreeRow[] = [];
  const visited = new Set<string>();
  const walk = (parent: string | null, depth: number) => {
    for (const group of children.get(parent) ?? []) {
      if (visited.has(group.id)) continue;
      visited.add(group.id);
      rows.push({ group, depth });
      walk(group.id, depth + 1);
    }
  };
  walk(null, 0);
  for (const group of [...groups].sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id))) {
    if (visited.has(group.id)) continue;
    visited.add(group.id);
    rows.push({ group, depth: 0 });
  }
  return rows;
}

function GroupFilter({ label, value, onChange, children }: Readonly<{
  label: string; value: string; onChange: (value: string) => void; children: ReactNode;
}>) {
  return <label className="flex min-w-0 shrink-0 items-center gap-2 text-xs text-muted-foreground">
    <span className="sr-only">{label}</span>
    <select aria-label={label} value={value} onChange={event => onChange(event.target.value)} className="control-surface h-9 max-w-48 rounded-md px-2 text-sm">{children}</select>
  </label>;
}

export default function DataflowGroupsPage() {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const location = useLocation();
  const [params] = useSearchParams();
  const searchInput = useRef<HTMLInputElement>(null);
  const previewFocus = useCollectionPreviewFocus(searchInput);
  const capturePreview = previewFocus.capture;
  const search = params.get("q") ?? "";
  const applicationFilter = params.get("application") ?? "";
  const parentFilter = params.get("parent") ?? "";
  const selectedId = params.get("selected");
  const { triggerProps, restoreFocus } = useInventoryRowActionFocus();
  const [target, setTarget] = useState<DataflowGroup | null>(null);
  const groups = useQuery({ queryKey: ["inventory", "catalog", "dataflow-groups"], queryFn: () => listDataflowGroups() });
  const applications = useQuery({ queryKey: ["inventory", "catalog", "applications"], queryFn: () => listApplications() });
  const tags = useQuery(inventoryResourceOptionsQuery("Tags"));

  const changeParams = (changes: Record<string, string | null>, replace = false) => {
    const next = new URLSearchParams(params);
    for (const [key, value] of Object.entries(changes)) { if (value) next.set(key, value); else next.delete(key); }
    void navigate({ pathname: location.pathname, search: `?${next}`, hash: location.hash }, { replace });
  };
  const remove = useApiMutation({
    mutationFn: deleteDataflowGroup,
    onSuccess: async () => {
      setTarget(null);
      changeParams({ selected: null }, true);
      await qc.invalidateQueries({ queryKey: ["inventory", "catalog", "dataflow-groups"] });
    },
  });
  const applicationName = (id?: string | null) => id ? applications.data?.find(app => app.id === id)?.name ?? t("dataflows.unresolved") : t("dataflows.noApplication");
  const groupName = (id?: string | null) => id ? groups.data?.find(group => group.id === id)?.name ?? t("dataflows.unresolved") : null;
  const tagNames = (ids?: string[] | null) => (ids ?? [])
    .map(id => tags.data?.items.find(tag => tag.id === id)?.name ?? t("dataflows.unresolved"));
  const statusLabel = (status?: string | null) => status ? t(`dataflows.statuses.${status}`, { defaultValue: status }) : "—";
  const tree = dataflowGroupTree(groups.data ?? []);
  const query = search.trim().toLocaleLowerCase();
  const rows = tree.filter(({ group, depth }) =>
    (!applicationFilter || group.application_id === applicationFilter) &&
    (!parentFilter || (parentFilter === ROOT_PARENT ? depth === 0 : group.parent_id === parentFilter)) &&
    [group.name, applicationName(group.application_id), groupName(group.parent_id) ?? ""]
      .some(value => value.toLocaleLowerCase().includes(query)));
  const selectedRow = selectedId ? tree.find(row => row.group.id === selectedId) ?? null : null;
  useEffect(() => {
    if (!selectedId) return;
    const opener = Array.from(document.querySelectorAll<HTMLElement>("[data-catalog-opener]")).find(element => element.dataset.catalogOpener === selectedId);
    if (opener) capturePreview(opener);
  }, [selectedId, groups.data, capturePreview]);
  const selection: CatalogSelection | null = selectedRow ? { kind: "group", item: selectedRow.group } : null;
  const canUpdate = can("UPDATE", "INVENTORY");
  const canDelete = can("DELETE", "INVENTORY");

  const inspect = (group: DataflowGroup, opener: HTMLElement) => {
    previewFocus.capture(opener);
    remove.reset();
    changeParams({ selected: group.id });
  };

  /** Ancestor names keep the hierarchy readable when a filtered row loses its parent row. */
  const parentChain = (group: DataflowGroup) => {
    const chain: string[] = [];
    const byId = new Map((groups.data ?? []).map(item => [item.id, item]));
    const seen = new Set<string>();
    let parentId = group.parent_id;
    while (parentId && !seen.has(parentId)) {
      seen.add(parentId);
      const parent = byId.get(parentId);
      if (!parent) break;
      chain.unshift(parent.name);
      parentId = parent.parent_id;
    }
    return chain;
  };

  const renderActions = (group: DataflowGroup) => canUpdate || canDelete ? <RowActions label={`${t("actions")}: ${group.name}`} {...triggerProps(group.id)}>
    {canUpdate && <DropdownMenuItem asChild><Link to={groupEditPath(group.id)}>{t("edit")}</Link></DropdownMenuItem>}
    {canDelete && <>
      {canUpdate && <DropdownMenuSeparator />}
      <DropdownMenuItem variant="destructive" disabled={remove.isPending} onSelect={() => { remove.reset(); setTarget(group); }}>{t("delete")}</DropdownMenuItem>
    </>}
  </RowActions> : null;

  const nameCell = (row: TreeRow) => <div style={{ paddingInlineStart: `${row.depth * 1.25}rem` }} className="flex min-w-0 flex-col gap-0.5">
    <button type="button" aria-haspopup="dialog" data-catalog-opener={row.group.id}
      aria-label={t("dataflows.inspectRecord", { name: row.group.name, defaultValue: "Inspect {{name}}" })}
      onClick={event => inspect(row.group, event.currentTarget)}
      className="w-fit max-w-full break-words rounded-sm text-left text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{row.group.name}</button>
    {parentChain(row.group).length > 0 && <span className="break-words text-xs text-muted-foreground">{parentChain(row.group).join(" / ")}</span>}
  </div>;

  const tagsCell = (group: DataflowGroup) => {
    const names = tagNames(group.effective_tags);
    const inherited = new Set(tagNames(group.inherited_tags));
    if (!names.length) return <span className="text-muted-foreground">—</span>;
    return <div className="flex flex-wrap gap-1">{names.map((name, index) =>
      <span key={`${name}:${index}`} className="rounded border border-border px-1.5 py-0.5 text-xs">{name}
        {inherited.has(name) && <span className="sr-only"> ({t("dataflows.inheritedTags")})</span>}
      </span>)}</div>;
  };

  const showBlocked = rows.some(row => !!row.group.blocked_by_group_id);
  const showTags = rows.some(row => (row.group.effective_tags ?? []).length > 0);
  const columns: Column<TreeRow>[] = [
    { key: "name", label: t("dataflows.groupName"), className: "min-w-56 whitespace-normal", render: nameCell },
    { key: "application", label: t("dataflows.groupApplication"), className: "text-muted-foreground", render: row => applicationName(row.group.application_id) },
    { key: "status", label: t("dataflows.groupStatus"), className: "text-muted-foreground", render: row => statusLabel(row.group.status) },
    { key: "effective_status", label: t("dataflows.effectiveStatus"), render: row => statusLabel(row.group.effective_status) },
    ...(showBlocked ? [{ key: "blocked_by", label: t("dataflows.blockingAncestor", { defaultValue: "Blocked by" }), className: "text-muted-foreground", render: (row: TreeRow) => row.group.blocked_by_group_id ? groupName(row.group.blocked_by_group_id) : "—" }] : []),
    ...(showTags ? [{ key: "tags", label: t("inventory.resources.fields.tags"), render: (row: TreeRow) => tagsCell(row.group) }] : []),
    { key: "actions", label: t("actions"), excludeFromSettings: true, className: "text-right", render: row => renderActions(row.group) },
  ];
  const detailNames: CatalogDetailNames = { application: applicationName, group: groupName, tags: tagNames };

  return <div className="flex min-w-0 flex-col gap-4">
    <PageChrome title={t("dataflows.flowGroupsTitle")} description={t("dataflows.groupHint")} icon={Network}
      actions={can("CREATE", "INVENTORY") ? <Button asChild><Link to={GROUP_NEW_PATH}><Plus aria-hidden="true" className="size-4" />{t("dataflows.newGroup")}</Link></Button> : undefined} />
    <DataflowCatalogNav />
    <DataTable<TreeRow>
      data={rows}
      columns={columns}
      tableId="inventory-dataflow-groups-catalog"
      toolbar={<>
        <SearchInput ref={searchInput} value={search} onValueChange={value => changeParams({ q: value || null }, true)} placeholder={t("search")} onClear={() => changeParams({ q: null }, true)} clearLabel={t("clear")} />
        <GroupFilter label={t("dataflows.applicationFilter")} value={applicationFilter} onChange={value => changeParams({ application: value || null }, true)}>
          <option value="">{t("dataflows.allApplications")}</option>
          {(applications.data ?? []).map(app => <option key={app.id} value={app.id}>{app.name}</option>)}
        </GroupFilter>
        <GroupFilter label={t("dataflows.groupParent")} value={parentFilter} onChange={value => changeParams({ parent: value || null }, true)}>
          <option value="">{t("dataflows.allGroups")}</option>
          <option value={ROOT_PARENT}>{t("dataflows.rootGroups", { defaultValue: "Root groups" })}</option>
          {tree.map(row => <option key={row.group.id} value={row.group.id}>{`${"— ".repeat(row.depth)}${row.group.name}`}</option>)}
        </GroupFilter>
        {!groups.isPending && !groups.isError && <span className="text-xs tabular-nums text-muted-foreground">{rows.length}</span>}
      </>}
      loading={groups.isPending}
      error={groups.error}
      onRetry={() => groups.refetch()}
      emptyState={<PageState kind="empty" title={t("noResults")} />}
      getRowKey={row => row.group.id}
      onRowClick={(row, rowElement) => inspect(row.group, rowElement)}
      isRowClickable={() => true}
      selectedRow={selectedRow}
    />
    {selectedId && !groups.isPending && !groups.isError && !selectedRow && <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <output>{t("dataflows.selectionUnavailable")}</output>
      <Button variant="ghost" size="sm" onClick={() => changeParams({ selected: null })}>{t("close")}</Button>
    </div>}

    <CatalogDetailSheet
      selection={selection}
      names={detailNames}
      editPath={selectedRow ? groupEditPath(selectedRow.group.id) : null}
      canEdit={canUpdate}
      canDelete={canDelete}
      deletePending={remove.isPending}
      deleteErrorMessage={remove.isError ? (remove.error instanceof ApiError && remove.error.status === 409 ? t("dataflows.groupAppConflict") : inventoryErrorMessage(t, remove.error)) : null}
      onDelete={() => { if (selectedRow && canDelete) remove.mutate(selectedRow.group.id); }}
      onClose={() => { remove.reset(); changeParams({ selected: null }); }}
      onCloseAutoFocus={previewFocus.onCloseAutoFocus}
    />

    <ConfirmDeleteDialog open={!!target} onOpenChange={open => { if (!open && !remove.isPending) setTarget(null); }}
      title={t("delete")} cancelLabel={t("cancel")} confirmLabel={t("delete")} cancelDisabled={remove.isPending}
      description={<>{target?.name}{remove.isError && <span role="alert" className="mt-2 block text-destructive">
        {remove.error instanceof ApiError && remove.error.status === 409 ? t("dataflows.groupAppConflict") : inventoryErrorMessage(t, remove.error)}
      </span>}</>}
      onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}
      confirmProps={{ disabled: remove.isPending, onClick: event => { event.preventDefault(); if (target && canDelete) remove.mutate(target.id); } }} />
  </div>;
}
