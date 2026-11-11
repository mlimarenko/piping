import { useEffect, useRef, useState } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Server } from "lucide-react";
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
import { deleteAddressGroup, listAddressGroups } from "./dataflowCatalog-api";
import type { AddressGroup } from "./dataflowCatalog-api";

const ADDRESS_GROUP_NEW_PATH = `${DATAFLOW_CATALOG_ROUTES.addressGroups}/new`;
const addressGroupEditPath = (id: string) => `${DATAFLOW_CATALOG_ROUTES.addressGroups}/${id}/edit`;

export default function AddressGroupsPage() {
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
  const selectedId = params.get("selected");
  const { triggerProps, restoreFocus } = useInventoryRowActionFocus();
  const [target, setTarget] = useState<AddressGroup | null>(null);
  const groups = useQuery({ queryKey: ["inventory", "catalog", "address-groups"], queryFn: () => listAddressGroups() });
  const owners = useQuery(inventoryResourceOptionsQuery("Owners"));
  const tenants = useQuery(inventoryResourceOptionsQuery("Tenants"));
  const tags = useQuery(inventoryResourceOptionsQuery("Tags"));

  const changeParams = (changes: Record<string, string | null>, replace = false) => {
    const next = new URLSearchParams(params);
    for (const [key, value] of Object.entries(changes)) { if (value) next.set(key, value); else next.delete(key); }
    void navigate({ pathname: location.pathname, search: `?${next}`, hash: location.hash }, { replace });
  };
  const remove = useApiMutation({
    mutationFn: deleteAddressGroup,
    onSuccess: async () => {
      setTarget(null);
      changeParams({ selected: null }, true);
      await qc.invalidateQueries({ queryKey: ["inventory", "catalog", "address-groups"] });
    },
  });
  const resourceName = (list: { id: string; name: string }[] | undefined, id?: string | null) =>
    id ? list?.find(row => row.id === id)?.name ?? t("dataflows.unresolved") : null;
  const ownerName = (id?: string | null) => resourceName(owners.data?.items, id);
  const tenantName = (id?: string | null) => resourceName(tenants.data?.items, id);
  const tagNames = (ids?: string[] | null) => (ids ?? [])
    .map(id => tags.data?.items.find(tag => tag.id === id)?.name ?? t("dataflows.unresolved"));
  const query = search.trim().toLocaleLowerCase();
  const rows = (groups.data ?? [])
    .filter(group => [group.name, group.description ?? "", ownerName(group.owner_id) ?? "", tenantName(group.tenant_id) ?? ""]
      .some(value => value.toLocaleLowerCase().includes(query)))
    .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id));
  const selected = selectedId ? (groups.data ?? []).find(group => group.id === selectedId) ?? null : null;
  useEffect(() => {
    if (!selectedId) return;
    const opener = Array.from(document.querySelectorAll<HTMLElement>("[data-catalog-opener]")).find(element => element.dataset.catalogOpener === selectedId);
    if (opener) capturePreview(opener);
  }, [selectedId, groups.data, capturePreview]);
  const selection: CatalogSelection | null = selected ? { kind: "addressGroup", item: selected } : null;
  const canUpdate = can("UPDATE", "INVENTORY");
  const canDelete = can("DELETE", "INVENTORY");

  const inspect = (group: AddressGroup, opener: HTMLElement) => {
    previewFocus.capture(opener);
    remove.reset();
    changeParams({ selected: group.id });
  };

  const renderActions = (group: AddressGroup) => canUpdate || canDelete ? <RowActions label={`${t("actions")}: ${group.name}`} {...triggerProps(group.id)}>
    {canUpdate && <DropdownMenuItem asChild><Link to={addressGroupEditPath(group.id)}>{t("edit")}</Link></DropdownMenuItem>}
    {canDelete && <>
      {canUpdate && <DropdownMenuSeparator />}
      <DropdownMenuItem variant="destructive" disabled={remove.isPending} onSelect={() => { remove.reset(); setTarget(group); }}>{t("delete")}</DropdownMenuItem>
    </>}
  </RowActions> : null;

  const nameCell = (group: AddressGroup) => <button type="button" aria-haspopup="dialog" data-catalog-opener={group.id}
    aria-label={t("dataflows.inspectRecord", { name: group.name, defaultValue: "Inspect {{name}}" })}
    onClick={event => inspect(group, event.currentTarget)}
    className="w-fit max-w-full break-words rounded-sm text-left text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{group.name}</button>;

  const showOwner = rows.some(group => !!group.owner_id);
  const showTenant = rows.some(group => !!group.tenant_id);
  const showTags = rows.some(group => (group.tags ?? []).length > 0);
  const columns: Column<AddressGroup>[] = [
    { key: "name", label: t("name"), className: "min-w-48 whitespace-normal", render: nameCell },
    { key: "members", label: t("dataflows.addressGroupMembers"), className: "tabular-nums", render: group => (group.members ?? []).length },
    { key: "description", label: t("description"), className: "max-w-96 whitespace-normal text-muted-foreground", render: group => group.description || "—" },
    ...(showOwner ? [{ key: "owner", label: t("inventory.resources.fields.owner"), className: "text-muted-foreground", render: (group: AddressGroup) => ownerName(group.owner_id) ?? "—" }] : []),
    ...(showTenant ? [{ key: "tenant", label: t("inventory.resources.fields.tenant"), className: "text-muted-foreground", render: (group: AddressGroup) => tenantName(group.tenant_id) ?? "—" }] : []),
    ...(showTags ? [{ key: "tags", label: t("inventory.resources.fields.tags"), render: (group: AddressGroup) => {
      const names = tagNames(group.tags);
      return names.length
        ? <div className="flex flex-wrap gap-1">{names.map((name, index) => <span key={`${name}:${index}`} className="rounded border border-border px-1.5 py-0.5 text-xs">{name}</span>)}</div>
        : <span className="text-muted-foreground">—</span>;
    } }] : []),
    { key: "actions", label: t("actions"), excludeFromSettings: true, className: "text-right", render: renderActions },
  ];
  const detailNames: CatalogDetailNames = { owner: ownerName, tenant: tenantName, tags: tagNames };

  return <div className="flex min-w-0 flex-col gap-4">
    <PageChrome title={t("dataflows.addressGroupsTitle")} description={t("dataflows.addressGroupHint")} icon={Server}
      actions={can("CREATE", "INVENTORY") ? <Button asChild><Link to={ADDRESS_GROUP_NEW_PATH}><Plus aria-hidden="true" className="size-4" />{t("dataflows.newAddressGroup")}</Link></Button> : undefined} />
    <DataflowCatalogNav />
    <DataTable<AddressGroup>
      data={rows}
      columns={columns}
      tableId="inventory-address-groups-catalog"
      toolbar={<>
        <SearchInput ref={searchInput} value={search} onValueChange={value => changeParams({ q: value || null }, true)} placeholder={t("search")} onClear={() => changeParams({ q: null }, true)} clearLabel={t("clear")} />
        {!groups.isPending && !groups.isError && <span className="text-xs tabular-nums text-muted-foreground">{rows.length}</span>}
      </>}
      loading={groups.isPending}
      error={groups.error}
      onRetry={() => groups.refetch()}
      emptyState={<PageState kind="empty" title={t("noResults")} />}
      getRowKey={group => group.id}
      onRowClick={(group, row) => inspect(group, row)}
      isRowClickable={() => true}
      selectedRow={selected}
    />
    {selectedId && !groups.isPending && !groups.isError && !selected && <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <output>{t("dataflows.selectionUnavailable")}</output>
      <Button variant="ghost" size="sm" onClick={() => changeParams({ selected: null })}>{t("close")}</Button>
    </div>}

    <CatalogDetailSheet
      selection={selection}
      names={detailNames}
      editPath={selected ? addressGroupEditPath(selected.id) : null}
      canEdit={canUpdate}
      canDelete={canDelete}
      deletePending={remove.isPending}
      deleteErrorMessage={remove.isError ? (remove.error instanceof ApiError && remove.error.status === 409 ? t("inventory.resources.deleteBlockedToast") : inventoryErrorMessage(t, remove.error)) : null}
      onDelete={() => { if (selected && canDelete) remove.mutate(selected.id); }}
      onClose={() => { remove.reset(); changeParams({ selected: null }); }}
      onCloseAutoFocus={previewFocus.onCloseAutoFocus}
    />

    <ConfirmDeleteDialog open={!!target} onOpenChange={open => { if (!open && !remove.isPending) setTarget(null); }}
      title={t("delete")} cancelLabel={t("cancel")} confirmLabel={t("delete")} cancelDisabled={remove.isPending}
      description={<>{target?.name}{remove.isError && <span role="alert" className="mt-2 block text-destructive">
        {remove.error instanceof ApiError && remove.error.status === 409 ? t("inventory.resources.deleteBlockedToast") : inventoryErrorMessage(t, remove.error)}
      </span>}</>}
      onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}
      confirmProps={{ disabled: remove.isPending, onClick: event => { event.preventDefault(); if (target && canDelete) remove.mutate(target.id); } }} />
  </div>;
}
