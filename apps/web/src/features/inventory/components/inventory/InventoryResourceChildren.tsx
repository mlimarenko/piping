import { useId } from "react";
import { Link, useLocation } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import type { RelationSpec } from "@/shared/nested-editor/descriptorFromSpec";
import { usePermissions } from "@/shared/auth/permissionsContext";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { useServerTable } from "@/shared/ui-shared/useServerTable";
import { listContentTypes } from "@/features/inventory/custom-fields-api";
import { inventoryResourceDetailHref } from "@/features/inventory/resourceLinks";
import { customFieldContentTypeTokenFor } from "@/features/inventory/tableConfig";
import { inventoryResourceChildRelations, inventoryResourceChildTableId, listInventoryResourceChildren, type ChildListParams } from "./inventoryResourceChildLists";

function ChildRelationTable({ entity, parent, spec, returnTo }: { entity: InventoryEntityKey; parent: Resource; spec: RelationSpec; returnTo: string }) {
  const { t } = useTranslation();
  const headingId = useId();
  const childTableId = inventoryResourceChildTableId(entity, spec);
  const needsType = spec.listFilterKey === "assigned_object_id";
  // Reuse the inventory entity/content-type owner rather than inventing a
  // second token matrix; the wire filter takes the resolved content-type UUID.
  const parentToken = customFieldContentTypeTokenFor(entity);
  const types = useQuery({ queryKey: ["inventory-content-types"], queryFn: listContentTypes, enabled: needsType });
  const typeId = types.data?.find(type => type.token === parentToken)?.id;
  const table = useServerTable<Resource, ChildListParams>({
    queryKey: ["inventory", "resources", spec.childEntity, "children", entity, parent.id, spec.key, typeId],
    listFn: params => listInventoryResourceChildren(spec, parent.id, params, typeId),
    sortKeys: [], pageSizeDefault: 50, keepPreviousData: false, staleTime: 0,
    enabled: !needsType || Boolean(typeId),
  });
  const typeUnavailable = needsType && (types.isError || (types.isSuccess && !typeId));
  const failed = typeUnavailable || table.query.isError;
  const columns: Column<Resource>[] = [
    { key: "name", label: t("name"), sortable: false, render: row => <Link className="break-words text-primary underline" to={inventoryResourceDetailHref(spec.childEntity, row.id, returnTo)!}>{row.name || row.id}</Link> },
    { key: "description", label: t("inventory.resources.fields.description"), sortable: false, render: row => <span className="whitespace-pre-wrap break-words">{row.description || "—"}</span> },
  ];
  const page = table.query.data;
  return <section className="flex min-w-0 flex-col gap-2" aria-labelledby={headingId}>
    <h2 id={headingId} className="text-base font-semibold">{t(spec.title)}</h2>
    <p className="break-words text-sm text-muted-foreground">{parent.name} · {parent.id}</p>
    <DataTable tableId={childTableId}
      columns={columns} data={page?.items ?? []} getRowKey={row => row.id}
      emptyState={<p className="p-4 text-sm text-muted-foreground">{t("nestedEditor.empty", { entity: t(spec.title) })}</p>}
      loading={!failed && (needsType && types.isPending || table.query.isPending)}
      error={failed ? new Error(t("dataTable.errorDescription")) : undefined}
      onRetry={() => typeUnavailable ? types.refetch() : table.query.refetch()}
      pagination={page && !failed ? { page: table.page, pageSize: table.pageSize, total: page.total, onPageChange: table.setPage, onPageSizeChange: table.setPageSize } : undefined} />
  </section>;
}

export default function InventoryResourceChildren({ entity, parent }: { entity: InventoryEntityKey; parent: Resource }) {
  const { can } = usePermissions();
  const location = useLocation();
  if (!can("READ", "INVENTORY")) return null;
  const returnTo = `${location.pathname}${location.search}${location.hash}`;
  return <>{inventoryResourceChildRelations(entity).map(spec =>
    <ChildRelationTable key={`${entity}:${parent.id}:${spec.key}`} entity={entity} parent={parent} spec={spec} returnTo={returnTo} />)}</>;
}
