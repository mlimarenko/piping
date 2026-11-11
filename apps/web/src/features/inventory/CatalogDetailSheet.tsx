import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";
import { Button } from "@/shared/ui-primitives/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/shared/ui-primitives/sheet";
import DefinitionGrid, { type DefinitionGridField } from "@/shared/ui-shared/DefinitionGrid";
import DataflowEndpointMembers from "./DataflowEndpointMembers";
import { DATAFLOW_CATALOG_ROUTES } from "./DataflowCatalogNav";
import { INVENTORY_RESOURCES_PATH } from "./resourceLinks";
import type { AddressGroup, Application, DataflowGroup } from "./dataflowCatalog-api";

type AddressGroupMember = AddressGroup["members"][number];

export type CatalogSelection =
  | { kind: "application"; item: Application }
  | { kind: "group"; item: DataflowGroup }
  | { kind: "addressGroup"; item: AddressGroup };

/** Read-only label resolution owned by the catalog page that already loaded the lists. */
export interface CatalogDetailNames {
  role?: (id?: string | null) => string | null;
  application?: (id?: string | null) => string | null;
  group?: (id?: string | null) => string | null;
  owner?: (id?: string | null) => string | null;
  tenant?: (id?: string | null) => string | null;
  tags?: (ids?: string[] | null) => string[];
}

interface CatalogDetailSheetProps {
  selection: CatalogSelection | null;
  names: CatalogDetailNames;
  editPath?: string | null;
  canEdit?: boolean;
  canDelete?: boolean;
  deletePending?: boolean;
  deleteErrorMessage?: string | null;
  onDelete?: () => void;
  onClose: () => void;
  onCloseAutoFocus: (event: Event) => void;
}

/** Scoped flow-catalog query for one catalog record; the flow page owns each filter. */
function catalogFlowsSearch(selection: CatalogSelection): string {
  const params = new URLSearchParams();
  if (selection.kind === "application") params.set("application", selection.item.id);
  else if (selection.kind === "group") params.set("group", selection.item.id);
  else {
    params.set("endpointType", "inventory.addressgroup");
    params.set("endpointId", selection.item.id);
  }
  return params.toString();
}

function DetailSection({ title, body }: Readonly<{ title: string; body?: string | null }>) {
  if (!body) return null;
  return <section className="flex flex-col gap-1 border-t border-border pt-4">
    <h3 className="text-sm font-semibold">{title}</h3>
    <p className="whitespace-pre-wrap break-words text-sm text-muted-foreground">{body}</p>
  </section>;
}

function TagList({ label, names }: Readonly<{ label: string; names: string[] }>) {
  const { t } = useTranslation();
  return <section className="flex min-w-0 flex-col gap-1">
    <h3 className="text-sm font-semibold">{label}</h3>
    {names.length > 0
      ? <ul aria-label={label} className="flex flex-wrap gap-1.5">
        {names.map((name, index) => <li key={`${name}:${index}`} className="surface-inset rounded-md px-2 py-0.5 text-xs">{name}</li>)}
      </ul>
      : <p className="text-xs text-muted-foreground">{t("dataflows.noTags")}</p>}
  </section>;
}

function descriptionValue(value?: string | null) {
  return value ? <span className="whitespace-pre-wrap break-words">{value}</span> : "—";
}

function statusLabel(t: ReturnType<typeof useTranslation>["t"], status?: string | null) {
  return status ? t(`dataflows.statuses.${status}`, { defaultValue: status }) : "—";
}

function ApplicationDetail({ item, names }: Readonly<{ item: Application; names: CatalogDetailNames }>) {
  const { t } = useTranslation();
  const fields: DefinitionGridField[] = [
    { label: t("dataflows.applicationRole"), value: names.role?.(item.role_id) ?? t("dataflows.noRole") },
    { label: t("description"), value: descriptionValue(item.description) },
    { label: t("inventory.resources.fields.owner"), value: names.owner?.(item.owner_id) ?? "—" },
    { label: t("inventory.resources.fields.tenant"), value: names.tenant?.(item.tenant_id) ?? "—" },
  ];
  return <>
    <DefinitionGrid fields={fields} />
    <TagList label={t("inventory.resources.fields.tags")} names={names.tags?.(item.tags) ?? []} />
    <DetailSection title={t("inventory.resources.fields.comments")} body={item.comments} />
    <div className="flex flex-wrap gap-2 border-t border-border pt-4">
      <Button asChild variant="outline" size="sm">
        <Link to={`${INVENTORY_RESOURCES_PATH}?entity=${encodeURIComponent("Contact Assignments")}&content_type=inventory.application&object_id=${encodeURIComponent(item.id)}`}>{t("dataflows.applicationContacts")}</Link>
      </Button>
    </div>
  </>;
}

function GroupDetail({ item, names }: Readonly<{ item: DataflowGroup; names: CatalogDetailNames }>) {
  const { t } = useTranslation();
  const fields: DefinitionGridField[] = [
    { label: t("dataflows.groupParent"), value: names.group?.(item.parent_id) ?? "—" },
    { label: t("dataflows.groupApplication"), value: names.application?.(item.application_id) ?? t("dataflows.noApplication") },
    { label: t("dataflows.groupStatus"), value: statusLabel(t, item.status) },
    { label: t("dataflows.effectiveStatus"), value: statusLabel(t, item.effective_status) },
    { label: t("dataflows.blockingAncestor", { defaultValue: "Blocked by" }), value: names.group?.(item.blocked_by_group_id) ?? "—" },
  ];
  return <>
    <DefinitionGrid fields={fields} />
    <div className="grid min-w-0 gap-4 @sm:grid-cols-2">
      <TagList label={t("dataflows.directTags")} names={names.tags?.(item.tags) ?? []} />
      <TagList label={t("dataflows.inheritedTags")} names={names.tags?.(item.inherited_tags) ?? []} />
      <TagList label={t("dataflows.effectiveTags", { defaultValue: "Effective tags" })} names={names.tags?.(item.effective_tags) ?? []} />
    </div>
    <DetailSection title={t("description")} body={item.description} />
    <DetailSection title={t("inventory.resources.fields.comments")} body={item.comments} />
  </>;
}

function AddressGroupDetail({ item, names }: Readonly<{ item: AddressGroup; names: CatalogDetailNames }>) {
  const { t } = useTranslation();
  const members: AddressGroupMember[] = item.members ?? [];
  const fields: DefinitionGridField[] = [
    { label: t("inventory.resources.fields.owner"), value: names.owner?.(item.owner_id) ?? "—" },
    { label: t("inventory.resources.fields.tenant"), value: names.tenant?.(item.tenant_id) ?? "—" },
    { label: t("dataflows.addressGroupMembers"), value: t("dataflows.addressGroupMemberCount", { count: members.length, defaultValue: "Objects in this group: {{count}}" }) },
  ];
  return <>
    <DefinitionGrid fields={fields} />
    <TagList label={t("inventory.resources.fields.tags")} names={names.tags?.(item.tags) ?? []} />
    <DetailSection title={t("description")} body={item.description} />
    <DetailSection title={t("inventory.resources.fields.comments")} body={item.comments} />
    <section aria-label={t("dataflows.addressGroupMembers")} className="flex flex-col gap-3 border-t border-border pt-4">
      <h2 className="text-sm font-semibold">{t("dataflows.addressGroupMembers")}</h2>
      {members.length > 0
        ? <ul className="flex flex-col gap-4">{members.map((member, index) => <li key={`${member.object_type}:${member.object_id ?? "none"}:${index}`}>
          <DataflowEndpointMembers value={member} label={t("dataflows.addressGroupMember", { index: index + 1 })} />
        </li>)}</ul>
        : <p className="text-sm text-muted-foreground">{t("dataflows.noMembers")}</p>}
    </section>
  </>;
}

function DeleteAction({ name, pending, errorMessage, onDelete }: Readonly<{
  name: string; pending: boolean; errorMessage: string | null; onDelete: () => void;
}>) {
  const { t } = useTranslation();
  const [confirming, setConfirming] = useState(false);
  return <div className="flex flex-col gap-3 border-t border-border pt-4">
    {!confirming
      ? <Button type="button" variant="destructive" size="sm" className="w-fit" disabled={pending} onClick={() => setConfirming(true)}>{t("delete")}</Button>
      : <div role="group" aria-label={t("delete")} className="flex flex-col gap-2 rounded-md border border-destructive/40 p-3">
        <p className="break-words text-sm">{t("dataflows.deleteCatalogConfirm", { name, defaultValue: "Delete {{name}}? This cannot be undone." })}</p>
        {errorMessage && <p role="alert" className="text-sm text-destructive">{errorMessage}</p>}
        <div className="flex flex-wrap gap-2">
          <Button type="button" variant="outline" size="sm" disabled={pending} onClick={() => setConfirming(false)}>{t("cancel")}</Button>
          <Button type="button" variant="destructive" size="sm" disabled={pending} onClick={onDelete}>{pending ? t("saving") : t("delete")}</Button>
        </div>
      </div>}
  </div>;
}

/** Read-only catalog inspector shared by the three management catalogs. Write controls stay permission-gated. */
export default function CatalogDetailSheet({
  selection, names, editPath, canEdit = false, canDelete = false, deletePending = false, deleteErrorMessage = null, onDelete, onClose, onCloseAutoFocus,
}: Readonly<CatalogDetailSheetProps>) {
  const { t } = useTranslation();
  const item = selection?.item;
  const flowsHref = selection ? { pathname: DATAFLOW_CATALOG_ROUTES.flows, search: `?${catalogFlowsSearch(selection)}` } : undefined;
  return <Sheet open={!!selection} onOpenChange={open => { if (!open) onClose(); }}>
    <SheetContent className="flex w-full flex-col gap-6 overflow-y-auto text-foreground sm:max-w-xl" onCloseAutoFocus={onCloseAutoFocus}>
      {selection && item && <>
        <SheetHeader>
          <SheetTitle className="break-words">{item.name}</SheetTitle>
          <SheetDescription>{t(selection.kind === "application" ? "dataflows.applicationHint" : selection.kind === "group" ? "dataflows.groupHint" : "dataflows.addressGroupHint")}</SheetDescription>
        </SheetHeader>
        <div className="flex flex-wrap items-center gap-2">
          {flowsHref && <Button asChild variant="outline" size="sm"><Link to={flowsHref}>{t("dataflows.catalogFlows")}</Link></Button>}
          {canEdit && editPath && <Button asChild variant="outline" size="sm"><Link to={editPath}>{t("edit")}</Link></Button>}
        </div>
        {selection.kind === "application" && <ApplicationDetail item={selection.item} names={names} />}
        {selection.kind === "group" && <GroupDetail item={selection.item} names={names} />}
        {selection.kind === "addressGroup" && <AddressGroupDetail item={selection.item} names={names} />}
        {canDelete && onDelete && <DeleteAction key={item.id} name={item.name} pending={deletePending} errorMessage={deleteErrorMessage} onDelete={onDelete} />}
      </>}
    </SheetContent>
  </Sheet>;
}
