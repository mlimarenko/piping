import { useEffect, useRef, useState } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Boxes, Plus } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { ApiError } from "@/shared/api/client";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { Button } from "@/shared/ui-primitives/button";
import { Badge } from "@/shared/ui-primitives/badge";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/shared/ui-primitives/dropdown-menu";
import EntityPicker from "@/shared/forms/EntityPicker";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageState from "@/shared/ui-shared/PageState";
import RowActions from "@/shared/ui-shared/RowActions";
import SearchInput from "@/shared/ui-shared/SearchInput";
import SurfaceCard, { SurfaceCardContent, SurfaceCardHeader } from "@/shared/ui-shared/SurfaceCard";
import ConfirmDeleteDialog from "@/shared/ui-shared/ConfirmDeleteDialog";
import FormField from "@/shared/ui-shared/FormField";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { useCollectionPreviewFocus } from "@/shared/ui-shared/useCollectionPreviewFocus";
import { useInventoryRowActionFocus } from "./useInventoryRowActionFocus";
import DataflowCatalogNav, { DATAFLOW_CATALOG_ROUTES } from "./DataflowCatalogNav";
import CatalogDetailSheet, { type CatalogDetailNames, type CatalogSelection } from "./CatalogDetailSheet";
import { inventoryErrorMessage } from "./inventoryErrors";
import { inventoryResourceOptions, inventoryResourceOptionsQuery } from "./resourceOptions";
import {
  createApplicationRole,
  deleteApplication,
  deleteApplicationRole,
  listApplicationRoles,
  listApplications,
  updateApplicationRole,
} from "./dataflowCatalog-api";
import type { Application, ApplicationRole } from "./dataflowCatalog-api";

const APPLICATIONS_NEW_PATH = `${DATAFLOW_CATALOG_ROUTES.applications}/new`;
const applicationEditPath = (id: string) => `${DATAFLOW_CATALOG_ROUTES.applications}/${id}/edit`;

type RoleFormState = {
  id?: string;
  name: string;
  description: string;
  comments: string;
  owner_id: string | null;
};

const EMPTY_ROLE: RoleFormState = { name: "", description: "", comments: "", owner_id: null };

function roleForm(role: ApplicationRole): RoleFormState {
  return {
    id: role.id,
    name: role.name ?? "",
    description: role.description ?? "",
    comments: role.comments ?? "",
    owner_id: role.owner_id ?? null,
  };
}

type Translate = (key: string, opts?: Record<string, unknown>) => string;

/** A role write is blocked while applications still reference it. */
function roleErrorMessage(t: Translate, error: unknown): string {
  return error instanceof ApiError && error.status === 409 ? t("dataflows.roleInUse") : inventoryErrorMessage(t, error);
}

function applicationErrorMessage(t: Translate, error: unknown): string {
  return error instanceof ApiError && error.status === 409 ? t("inventory.resources.deleteBlockedToast") : inventoryErrorMessage(t, error);
}

function ResourceRelation({ value, onChange, disabled }: Readonly<{ value: string | null; onChange: (value: string | null) => void; disabled: boolean }>) {
  const { t } = useTranslation();
  const owners = useQuery({ ...inventoryResourceOptionsQuery("Owners"), enabled: !disabled });
  return <EntityPicker entity="Owners" label={t("inventory.resources.fields.owner")} value={value ?? ""} onChange={next => onChange(next || null)}
    options={inventoryResourceOptions("Owners", owners.data?.items ?? [])}
    loading={owners.isPending} disabled={disabled || owners.isError} />;
}

function RoleDialog({ draft, onDraft, onClose }: Readonly<{
  draft: RoleFormState | null;
  onDraft: (draft: RoleFormState) => void;
  onClose: () => void;
}>) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [nameError, setNameError] = useState<string | null>(null);
  const save = useApiMutation({
    mutationFn: (body: RoleFormState) => body.id
      ? updateApplicationRole(body.id, { name: body.name, description: body.description, comments: body.comments, owner_id: body.owner_id })
      : createApplicationRole({ name: body.name, description: body.description, comments: body.comments, owner_id: body.owner_id }),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["inventory", "catalog", "application-roles"] });
      await qc.invalidateQueries({ queryKey: ["inventory", "catalog", "applications"] });
      setNameError(null);
      save.reset();
      onClose();
    },
  });
  if (!draft) return null;
  const submit = () => {
    if (save.isPending) return;
    if (!draft.name.trim()) { setNameError(t("required")); return; }
    setNameError(null);
    save.mutate({ ...draft, name: draft.name.trim() });
  };
  return <Dialog open onOpenChange={open => { if (!open && !save.isPending) { save.reset(); onClose(); } }}>
    <DialogContent className="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>{draft.id ? t("dataflows.editRole", { name: draft.name }) : t("dataflows.newRole")}</DialogTitle>
        <DialogDescription>{t("dataflows.applicationHint")}</DialogDescription>
      </DialogHeader>
      <form className="flex min-w-0 flex-col gap-4" noValidate onSubmit={event => { event.preventDefault(); submit(); }}>
        <FormField label={t("dataflows.roleName")} htmlFor="role-name" required error={nameError}>
          <Input id="role-name" value={draft.name} onChange={event => onDraft({ ...draft, name: event.target.value })} disabled={save.isPending} />
        </FormField>
        <FormField label={t("dataflows.roleDescription")} htmlFor="role-description">
          <Textarea id="role-description" value={draft.description} rows={2} onChange={event => onDraft({ ...draft, description: event.target.value })} disabled={save.isPending} />
        </FormField>
        <FormField label={t("inventory.resources.fields.comments")} htmlFor="role-comments">
          <Textarea id="role-comments" value={draft.comments} rows={2} onChange={event => onDraft({ ...draft, comments: event.target.value })} disabled={save.isPending} />
        </FormField>
        <FormField label={t("inventory.resources.fields.owner")}>
          <ResourceRelation value={draft.owner_id} onChange={owner_id => onDraft({ ...draft, owner_id })} disabled={save.isPending} />
        </FormField>
        {save.isError && <p role="alert" className="text-sm text-destructive">{roleErrorMessage(t, save.error)}</p>}
        <DialogFooter>
          <Button type="button" variant="outline" onClick={onClose} disabled={save.isPending}>{t("cancel")}</Button>
          <Button type="submit" disabled={save.isPending}>{t(draft.id ? "saveChanges" : "create")}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>;
}

export default function ApplicationsPage() {
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
  const [target, setTarget] = useState<Application | null>(null);
  const [roleDraft, setRoleDraft] = useState<RoleFormState | null>(null);
  const [roleTarget, setRoleTarget] = useState<ApplicationRole | null>(null);

  const applications = useQuery({ queryKey: ["inventory", "catalog", "applications"], queryFn: () => listApplications() });
  const roles = useQuery({ queryKey: ["inventory", "catalog", "application-roles"], queryFn: () => listApplicationRoles() });
  const owners = useQuery(inventoryResourceOptionsQuery("Owners"));
  const tenants = useQuery(inventoryResourceOptionsQuery("Tenants"));
  const tags = useQuery(inventoryResourceOptionsQuery("Tags"));

  const changeParams = (changes: Record<string, string | null>, replace = false) => {
    const next = new URLSearchParams(params);
    for (const [key, value] of Object.entries(changes)) { if (value) next.set(key, value); else next.delete(key); }
    void navigate({ pathname: location.pathname, search: `?${next}`, hash: location.hash }, { replace });
  };
  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ["inventory", "catalog", "applications"] });
    await qc.invalidateQueries({ queryKey: ["inventory", "catalog", "application-roles"] });
  };
  const remove = useApiMutation({
    mutationFn: deleteApplication,
    onSuccess: async () => { setTarget(null); changeParams({ selected: null }, true); await refresh(); },
  });
  const removeRole = useApiMutation({
    mutationFn: deleteApplicationRole,
    onSuccess: async () => { setRoleTarget(null); await refresh(); },
  });

  const roleName = (id?: string | null) => id ? roles.data?.find(role => role.id === id)?.name ?? t("dataflows.unresolved") : t("dataflows.noRole");
  const resourceName = (list: { id: string; name: string }[] | undefined, id?: string | null) =>
    id ? list?.find(row => row.id === id)?.name ?? null : null;
  const ownerName = (id?: string | null) => resourceName(owners.data?.items, id) ?? (id ? t("dataflows.unresolved") : null);
  const tenantName = (id?: string | null) => resourceName(tenants.data?.items, id) ?? (id ? t("dataflows.unresolved") : null);
  const namesFor = (list: { id: string; name: string }[] | undefined, ids?: string[] | null) =>
    (ids ?? []).map(id => list?.find(row => row.id === id)?.name ?? t("dataflows.unresolved"));
  const tagNames = (ids?: string[] | null) => namesFor(tags.data?.items, ids);
  const query = search.trim().toLocaleLowerCase();
  const rows = (applications.data ?? [])
    .filter(app => [app.name, app.description ?? "", roleName(app.role_id), ownerName(app.owner_id) ?? "", tenantName(app.tenant_id) ?? ""]
      .some(value => value.toLocaleLowerCase().includes(query)))
    .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id));
  const selected = selectedId ? (applications.data ?? []).find(app => app.id === selectedId) ?? null : null;
  useEffect(() => {
    if (!selectedId) return;
    const opener = Array.from(document.querySelectorAll<HTMLElement>("[data-catalog-opener]")).find(element => element.dataset.catalogOpener === selectedId);
    if (opener) capturePreview(opener);
  }, [selectedId, applications.data, capturePreview]);
  const selection: CatalogSelection | null = selected ? { kind: "application", item: selected } : null;
  const canUpdate = can("UPDATE", "INVENTORY");
  const canDelete = can("DELETE", "INVENTORY");

  const inspect = (app: Application, opener: HTMLElement) => {
    previewFocus.capture(opener);
    remove.reset();
    changeParams({ selected: app.id });
  };

  const renderActions = (app: Application) => canUpdate || canDelete ? <RowActions label={`${t("actions")}: ${app.name}`} {...triggerProps(app.id)}>
    {canUpdate && <DropdownMenuItem asChild><Link to={applicationEditPath(app.id)}>{t("edit")}</Link></DropdownMenuItem>}
    {canDelete && <>
      {canUpdate && <DropdownMenuSeparator />}
      <DropdownMenuItem variant="destructive" disabled={remove.isPending} onSelect={() => { remove.reset(); setTarget(app); }}>{t("delete")}</DropdownMenuItem>
    </>}
  </RowActions> : null;

  const nameCell = (app: Application) => <button type="button" aria-haspopup="dialog" data-catalog-opener={app.id}
    aria-label={t("dataflows.inspectRecord", { name: app.name, defaultValue: "Inspect {{name}}" })}
    onClick={event => inspect(app, event.currentTarget)}
    className="w-fit max-w-full break-words rounded-sm text-left text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{app.name}</button>;

  const showOwner = rows.some(app => !!app.owner_id);
  const showTenant = rows.some(app => !!app.tenant_id);
  const showTags = rows.some(app => (app.tags ?? []).length > 0);
  const columns: Column<Application>[] = [
    { key: "name", label: t("name"), className: "min-w-48 whitespace-normal", render: nameCell },
    { key: "role", label: t("dataflows.applicationRole"), render: app => app.role_id ? roleName(app.role_id) : t("dataflows.noRole") },
    { key: "description", label: t("description"), className: "max-w-80 whitespace-normal text-muted-foreground", render: app => app.description || "—" },
    ...(showOwner ? [{ key: "owner", label: t("inventory.resources.fields.owner"), className: "text-muted-foreground", render: (app: Application) => ownerName(app.owner_id) ?? "—" }] : []),
    ...(showTenant ? [{ key: "tenant", label: t("inventory.resources.fields.tenant"), className: "text-muted-foreground", render: (app: Application) => tenantName(app.tenant_id) ?? "—" }] : []),
    ...(showTags ? [{ key: "tags", label: t("inventory.resources.fields.tags"), render: (app: Application) => {
      const names = tagNames(app.tags);
      return names.length
        ? <div className="flex flex-wrap gap-1">{names.map((name, index) => <Badge key={`${name}:${index}`} variant="secondary">{name}</Badge>)}</div>
        : <span className="text-muted-foreground">—</span>;
    } }] : []),
    { key: "actions", label: t("actions"), excludeFromSettings: true, className: "text-right", render: renderActions },
  ];

  const detailNames: CatalogDetailNames = { role: roleName, owner: ownerName, tenant: tenantName, tags: tagNames };

  return <div className="flex min-w-0 flex-col gap-4">
    <PageChrome title={t("dataflows.applicationsTitle")} description={t("dataflows.applicationHint")} icon={Boxes}
      actions={can("CREATE", "INVENTORY") ? <Button asChild><Link to={APPLICATIONS_NEW_PATH}><Plus aria-hidden="true" className="size-4" />{t("dataflows.newApplication")}</Link></Button> : undefined} />
    <DataflowCatalogNav />
    <DataTable<Application>
      data={rows}
      columns={columns}
      tableId="inventory-applications-catalog"
      toolbar={<>
        <SearchInput ref={searchInput} value={search} onValueChange={value => changeParams({ q: value || null }, true)} placeholder={t("search")} onClear={() => changeParams({ q: null }, true)} clearLabel={t("clear")} />
        {!applications.isPending && !applications.isError && <span className="text-xs tabular-nums text-muted-foreground">{rows.length}</span>}
      </>}
      loading={applications.isPending}
      error={applications.error}
      onRetry={() => applications.refetch()}
      emptyState={<PageState kind="empty" title={t("noResults")} />}
      getRowKey={app => app.id}
      onRowClick={(app, row) => inspect(app, row)}
      isRowClickable={() => true}
      selectedRow={selected}
    />
    {selectedId && !applications.isPending && !applications.isError && !selected && <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <output>{t("dataflows.selectionUnavailable")}</output>
      <Button variant="ghost" size="sm" onClick={() => changeParams({ selected: null })}>{t("close")}</Button>
    </div>}

    <SurfaceCard role="region" aria-label={t("dataflows.applicationRoles")}>
      <SurfaceCardHeader title={t("dataflows.applicationRoles")} description={t("dataflows.applicationHint")}
        actions={can("CREATE", "INVENTORY") ? <Button type="button" variant="outline" onClick={() => setRoleDraft(EMPTY_ROLE)}><Plus aria-hidden="true" className="size-4" />{t("dataflows.newRole")}</Button> : undefined} />
      <SurfaceCardContent>
        {roles.isPending ? <PageState kind="loading" title={t("loading")} />
          : roles.isError ? <PageState kind="error" description={t("dataTable.errorDescription")}
            action={<Button variant="outline" onClick={() => void roles.refetch()}>{t("retry")}</Button>} />
          : !(roles.data ?? []).length ? <p className="text-sm text-muted-foreground">{t("dataflows.rolesEmpty")}</p>
          : <ul className="flex flex-col divide-y divide-border">
            {[...(roles.data ?? [])].sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id)).map(role => <li key={role.id} className="flex flex-wrap items-start justify-between gap-3 py-3 first:pt-0 last:pb-0">
              <div className="min-w-0">
                <p className="font-medium">{role.name}</p>
                {role.description && <p className="text-sm text-muted-foreground">{role.description}</p>}
                {role.comments && <p className="text-xs text-muted-foreground">{role.comments}</p>}
              </div>
              {(canUpdate || canDelete) && <div className="flex shrink-0 gap-1">
                {canUpdate && <Button type="button" variant="ghost" size="sm" onClick={() => setRoleDraft(roleForm(role))} aria-label={`${t("edit")}: ${role.name}`}>{t("edit")}</Button>}
                {canDelete && <Button type="button" variant="ghost" size="sm" onClick={() => { removeRole.reset(); setRoleTarget(role); }} aria-label={`${t("delete")}: ${role.name}`}>{t("delete")}</Button>}
              </div>}
            </li>)}
          </ul>}
      </SurfaceCardContent>
    </SurfaceCard>

    <CatalogDetailSheet
      selection={selection}
      names={detailNames}
      editPath={selected ? applicationEditPath(selected.id) : null}
      canEdit={canUpdate}
      canDelete={canDelete}
      deletePending={remove.isPending}
      deleteErrorMessage={remove.isError ? applicationErrorMessage(t, remove.error) : null}
      onDelete={() => { if (selected && canDelete) remove.mutate(selected.id); }}
      onClose={() => { remove.reset(); changeParams({ selected: null }); }}
      onCloseAutoFocus={previewFocus.onCloseAutoFocus}
    />

    <ConfirmDeleteDialog open={!!target} onOpenChange={open => { if (!open && !remove.isPending) setTarget(null); }}
      title={t("delete")} description={<>{target?.name}{remove.isError && <span role="alert" className="mt-2 block text-destructive">{applicationErrorMessage(t, remove.error)}</span>}</>}
      cancelLabel={t("cancel")} confirmLabel={t("delete")} cancelDisabled={remove.isPending}
      onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}
      confirmProps={{ disabled: remove.isPending, onClick: event => { event.preventDefault(); if (target && canDelete) remove.mutate(target.id); } }} />

    <RoleDialog draft={roleDraft} onDraft={setRoleDraft} onClose={() => setRoleDraft(null)} />

    <ConfirmDeleteDialog open={!!roleTarget} onOpenChange={open => { if (!open && !removeRole.isPending) setRoleTarget(null); }}
      title={t("delete")} description={<>{roleTarget?.name}{removeRole.isError && <span role="alert" className="mt-2 block text-destructive">{roleErrorMessage(t, removeRole.error)}</span>}</>}
      cancelLabel={t("cancel")} confirmLabel={t("delete")} cancelDisabled={removeRole.isPending}
      onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}
      confirmProps={{ disabled: removeRole.isPending, onClick: event => { event.preventDefault(); if (roleTarget && canDelete) removeRole.mutate(roleTarget.id); } }} />
  </div>;
}
