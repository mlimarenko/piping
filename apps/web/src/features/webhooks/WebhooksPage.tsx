import { useState, useMemo, useRef } from "react";
import { useTranslation, Trans } from "react-i18next";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Webhook, Plus, Trash2, Pencil, Zap, Loader2, KeyRound } from "lucide-react";
import { notify } from "@/shared/lib/notifications";
import WebhookEventPicker from "@/features/webhooks/WebhookEventPicker";
import WebhookConditionsEditor from "@/features/webhooks/WebhookConditionsEditor";
import {
  deserializeConditions,
  serializeConditions,
  type ConditionLogic,
  type ConditionRow,
} from "@/features/webhooks/webhookEvents";

import PageChrome from "@/shared/ui-shared/PageChrome";
import PageState from "@/shared/ui-shared/PageState";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import FormSection from "@/shared/ui-shared/FormSection";
import DataTable, {
  type DataTableSortState,
  type DataTableFilterOption,
} from "@/shared/ui-shared/DataTable";
import EmptyState from "@/shared/ui-shared/EmptyState";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { usePermissions } from "@/shared/auth/permissionsContext";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";
import {
  buildWebhookFilterParams,
  WEBHOOKS_TABLE_CONFIG,
} from "@/features/webhooks/tableConfig";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import { useDebouncedValue } from "@/shared/hooks/useDebouncedValue";
import { Button } from "@/shared/ui-primitives/button";
import { Badge } from "@/shared/ui-primitives/badge";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import ConfirmDeleteDialog from "@/shared/ui-shared/ConfirmDeleteDialog";
import { Input } from "@/shared/ui-primitives/input";
import { Label } from "@/shared/ui-primitives/label";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import { Checkbox } from "@/shared/ui-primitives/checkbox";

import {
  listWebhooks,
  createWebhook,
  updateWebhook,
  deleteWebhook,
  testFireWebhook,
  clearWebhookSecret,
  getWebhookEventCatalog,
  type WebhookSubscription,
  type CreateWebhookSubscriptionRequest,
  type UpdateWebhookSubscriptionRequest,
} from "@/features/webhooks/api";
import { formatRelativeTime, httpStatusColor } from "@/shared/lib/format";

interface WebhookFormValues {
  name: string;
  target_url: string;
  secret: string;
  event_types: string[];
  enabled: boolean;
  /** Structured condition rows + their top-level logic; serialized at submit. */
  conditionLogic: ConditionLogic;
  conditionRows: ConditionRow[];
}

const defaultForm: WebhookFormValues = {
  name: "",
  target_url: "",
  secret: "",
  event_types: [],
  enabled: true,
  conditionLogic: "and",
  conditionRows: [],
};

function webhookToForm(w: WebhookSubscription): WebhookFormValues {
  const { logic, rows } = deserializeConditions(w.conditions);
  return {
    name: w.name,
    target_url: w.target_url,
    secret: "",
    event_types: w.event_types,
    enabled: w.enabled,
    conditionLogic: logic,
    conditionRows: rows,
  };
}

const WebhooksPage = () => {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const [formOpen, setFormOpen] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [form, setForm] = useState<WebhookFormValues>(defaultForm);
  const [deleteTarget, setDeleteTarget] = useState<WebhookSubscription | null>(null);
  const actionTriggers = useRef(new Map<string, HTMLButtonElement>());
  const deleteInvoker = useRef<HTMLButtonElement | null>(null);
  const [firingId, setFiringId] = useState<string | null>(null);
  const [search, setSearch] = useState("");

  // View-but-not-edit: hide mutation affordances the caller can't use. Editor,
  // e.g., holds CREATE/UPDATE on WEBHOOK but not DELETE, so it sees edit/test/
  // clear-secret but no delete button.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "WEBHOOK");
  const canUpdate = can("UPDATE", "WEBHOOK");
  const canDelete = can("DELETE", "WEBHOOK");
  const [page, setPage] = useState(0);
  const { value: pageSize, setValue: setPageSize } = useTablePageSize();

  // Backend-driven sort for every visible data column + multi-value column filters.
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});
  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, WEBHOOKS_TABLE_CONFIG.sortKeys),
    [sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        columnFilters,
        WEBHOOKS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [columnFilters],
  );
  const webhookFilterParams = useMemo(
    () => buildWebhookFilterParams(sanitizedColumnFilters),
    [sanitizedColumnFilters],
  );
  const eventTypesFilterValues = useMemo(
    () => sanitizedColumnFilters.event_types ?? [],
    [sanitizedColumnFilters.event_types],
  );
  const lastStatusFilterValues = useMemo(
    () => sanitizedColumnFilters.last_status ?? [],
    [sanitizedColumnFilters.last_status],
  );
  const enabledFilterOptions: DataTableFilterOption[] = [
    { value: "true", label: t("enabled") },
    { value: "false", label: t("disabled") },
  ];
  const secretSetFilterOptions: DataTableFilterOption[] = [
    { value: "true", label: t("webhooks.secret.set") },
    { value: "false", label: t("webhooks.secret.notSet") },
  ];

  // Debounced search — reset page on change
  const debouncedSearch = useDebouncedValue(search);

  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: [
      "webhooks",
      {
        page,
        pageSize,
        search: debouncedSearch,
        sort: sanitizedSortState,
        filters: webhookFilterParams,
      },
    ],
    queryFn: () =>
      listWebhooks({
        limit: pageSize,
        offset: page * pageSize,
        search: debouncedSearch || undefined,
        sort: sanitizedSortState?.key,
        order: sanitizedSortState?.dir,
        enabled: webhookFilterParams.enabled,
        secret_set: webhookFilterParams.secret_set,
        event_types: webhookFilterParams.event_types,
        last_status: webhookFilterParams.last_status,
      }),
    placeholderData: (prev) => prev,
  });

  const items = useMemo(() => data?.items ?? [], [data?.items]);
  const total = data?.total ?? 0;

  // Event catalog drives both the picker and the event-type table filter.
  const { data: catalog = [], isPending: catalogPending } = useQuery({
    queryKey: ["webhook-event-catalog"],
    queryFn: getWebhookEventCatalog,
    staleTime: 5 * 60_000,
  });
  const eventTypeFilterOptions = useMemo<DataTableFilterOption[]>(() => {
    const values = [
      ...(catalog.length > 0
        ? catalog.map((item) => item.value)
        : items.flatMap((item) => item.event_types)),
      ...eventTypesFilterValues,
    ];
    return Array.from(new Set(values))
      .sort((a, b) => a.localeCompare(b))
      .map((value) => ({ value, label: value }));
  }, [catalog, eventTypesFilterValues, items]);
  const lastStatusFilterOptions = useMemo<DataTableFilterOption[]>(() => (
    Array.from(
      new Set([
        ...items.flatMap((item) => (item.last_status == null ? [] : [item.last_status])),
        ...lastStatusFilterValues
          .map((status) => Number(status))
          .filter((status) => Number.isFinite(status)),
      ]),
    )
      .sort((a, b) => a - b)
      .map((status) => ({ value: String(status), label: String(status) }))
  ), [items, lastStatusFilterValues]);

  const createMutation = useMutation({
    mutationFn: (body: CreateWebhookSubscriptionRequest) => createWebhook(body),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["webhooks"] });
      setFormOpen(false);
      setForm(defaultForm);
    },
  });

  const updateMutation = useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateWebhookSubscriptionRequest }) =>
      updateWebhook(id, body),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["webhooks"] });
      setFormOpen(false);
      setForm(defaultForm);
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteWebhook(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["webhooks"] });
      setDeleteTarget(null);
    },
    onError: (err) => {
      notify("error", t("webhooks.deleteFailed", { error: String(err) }));
    },
  });

  const clearSecretMutation = useMutation({
    mutationFn: (id: string) => clearWebhookSecret(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["webhooks"] });
      notify("success", t("webhooks.secretCleared"));
    },
    onError: (err) => {
      notify("error", t("webhooks.clearSecretFailed", { error: String(err) }));
    },
  });

  const openCreate = () => {
    createMutation.reset();
    updateMutation.reset();
    setEditingId(null);
    setForm(defaultForm);
    setFormOpen(true);
  };

  const openEdit = (w: WebhookSubscription) => {
    createMutation.reset();
    updateMutation.reset();
    setEditingId(w.id);
    setForm(webhookToForm(w));
    setFormOpen(true);
  };

  const handleFormSubmit = () => {
    if (createMutation.isPending || updateMutation.isPending || !(editingId ? canUpdate : canCreate)) return;
    // Empty condition list → omit `conditions` (no filter); on update we send
    // an explicit empty set so a previously-set filter can be cleared.
    const conditions = serializeConditions(form.conditionLogic, form.conditionRows);
    if (editingId) {
      const body: UpdateWebhookSubscriptionRequest = {
        name: form.name || undefined,
        target_url: form.target_url || undefined,
        event_types: form.event_types.length ? form.event_types : undefined,
        enabled: form.enabled,
        conditions: conditions ?? null,
      };
      if (form.secret) body.secret = form.secret;
      updateMutation.mutate({ id: editingId, body });
    } else {
      const body: CreateWebhookSubscriptionRequest = {
        name: form.name,
        target_url: form.target_url,
        event_types: form.event_types,
        enabled: form.enabled,
      };
      if (conditions) body.conditions = conditions;
      if (form.secret) body.secret = form.secret;
      createMutation.mutate(body);
    }
  };

  const handleTestFire = async (id: string) => {
    setFiringId(id);
    try {
      const result = await testFireWebhook(id);
      void queryClient.invalidateQueries({ queryKey: ["webhooks"] });
      if (result.ok) {
        notify("success",
          t("webhooks.testFireSuccess", { status: result.status, latency: result.latency_ms }),
        );
      } else {
        notify("error", t("webhooks.testFireFailed", { error: result.error ?? String(result.status) }));
      }
    } catch (e) {
      notify("error", t("webhooks.testFireFailed", { error: String(e) }));
    } finally {
      setFiringId(null);
    }
  };

  const isMutating = createMutation.isPending || updateMutation.isPending;
  const canSubmit = form.name.trim() !== "" && form.target_url.trim() !== "" && form.event_types.length > 0;

  // Sort/filter change resets to the first page.
  const handleSortChange = (next: DataTableSortState | null) => {
    setSortState(next);
    setPage(0);
  };
  const handleColumnFilterChange = (key: string, values: string[]) => {
    setColumnFilters((prev) => updateDataTableColumnFilters(prev, key, values));
    setPage(0);
  };

  const columns = [
    {
      key: "name",
      label: t("webhooks.fields.name"),
      sortable: true,
      render: (w: WebhookSubscription) => (
        <span className="font-medium text-foreground">{w.name}</span>
      ),
    },
    {
      key: "target_url",
      label: t("webhooks.fields.targetUrl"),
      sortable: true,
      render: (w: WebhookSubscription) => (
        <span
          className="block max-w-[32ch] truncate text-muted-foreground font-mono text-xs"
          title={w.target_url}
        >
          {w.target_url}
        </span>
      ),
    },
    {
      key: "secret_set",
      label: t("webhooks.fields.secret"),
      sortable: true,
      filterOptions: secretSetFilterOptions,
      render: (w: WebhookSubscription) => (
        <span
          className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${
            w.secret_set
              ? "bg-status-positive/10 text-status-positive border border-status-positive/15"
              : "surface-chip"
          }`}
        >
          {w.secret_set ? t("webhooks.secret.set") : t("webhooks.secret.notSet")}
        </span>
      ),
    },
    {
      key: "event_types",
      label: t("webhooks.fields.eventTypes"),
      sortable: true,
      filterOptions: eventTypeFilterOptions,
      render: (w: WebhookSubscription) => (
        <div className="flex flex-wrap gap-1">
          {w.event_types.map((et) => (
            <Badge key={et} variant="secondary" className="text-xs">
              {et}
            </Badge>
          ))}
        </div>
      ),
    },
    {
      key: "enabled",
      label: t("webhooks.fields.enabled"),
      sortable: true,
      filterOptions: enabledFilterOptions,
      render: (w: WebhookSubscription) => (
        <span
          className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${
            w.enabled
              ? "bg-status-positive/10 text-status-positive border border-status-positive/15"
              : "surface-chip"
          }`}
        >
          {w.enabled ? t("enabled") : t("disabled")}
        </span>
      ),
    },
    {
      key: "last_fired_at",
      label: t("webhooks.fields.lastFired"),
      sortable: true,
      render: (w: WebhookSubscription) => (
        <span className="text-muted-foreground text-xs">{formatRelativeTime(w.last_fired_at)}</span>
      ),
    },
    {
      key: "last_status",
      label: t("webhooks.fields.lastStatus"),
      sortable: true,
      filterOptions: lastStatusFilterOptions,
      render: (w: WebhookSubscription) =>
        w.last_status ? (
          <span
            className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${httpStatusColor(w.last_status)}`}
          >
            {w.last_status}
          </span>
        ) : (
          <span className="text-muted-foreground text-xs">—</span>
        ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      render: (w: WebhookSubscription) => (
        <div className="flex items-center justify-end gap-1">
          {canUpdate && (
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8"
              title={t("webhooks.actions.testFire")}
              aria-label={t("webhooks.actions.testFire")}
              disabled={firingId !== null}
              onClick={() => void handleTestFire(w.id)}
            >
              {firingId === w.id ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Zap className="h-3.5 w-3.5" />
              )}
            </Button>
          )}
          {(canUpdate || canDelete) && <RowActions label={`${t("actions")}: ${w.name}`} ref={node => { if (node) actionTriggers.current.set(w.id, node); else actionTriggers.current.delete(w.id); }}>
            {canUpdate && <DropdownMenuItem onSelect={() => openEdit(w)}><Pencil />{t("webhooks.actions.edit")}</DropdownMenuItem>}
            {canUpdate && w.secret_set && <DropdownMenuItem variant="destructive" disabled={clearSecretMutation.isPending} onSelect={() => clearSecretMutation.mutate(w.id)}><KeyRound />{t("webhooks.actions.clearSecret")}</DropdownMenuItem>}
            {canDelete && <DropdownMenuItem variant="destructive" onSelect={() => { deleteInvoker.current = actionTriggers.current.get(w.id) ?? null; setDeleteTarget(w); }}><Trash2 />{t("webhooks.actions.delete")}</DropdownMenuItem>}
          </RowActions>}
        </div>
      ),
    },
  ];

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <PageChrome
        title={t("webhooks.title")}
        description={t("webhooks.description")}
        icon={Webhook}
        badge={total > 0 ? String(total) : undefined}
        actions={
          canCreate ? (
            <Button onClick={openCreate}>
              <Plus className="h-4 w-4 mr-1.5" />
              {t("webhooks.add")}
            </Button>
          ) : undefined
        }
      />

      <DataTable
        tableId="webhooks.list"
        columns={columns}
        data={items}
        getRowKey={(w) => w.id}
        loading={isPending}
        error={isError ? error : undefined}
        onRetry={() => refetch()}
        sortState={sanitizedSortState}
        onSortChange={handleSortChange}
        columnFilters={sanitizedColumnFilters}
        onColumnFilterChange={handleColumnFilterChange}
        pagination={{
          page,
          pageSize,
          total,
          onPageChange: setPage,
          onPageSizeChange: (s) => { setPageSize(s); setPage(0); },
        }}
        emptyState={
          isPending ? (
            <PageState kind="loading" title={t("webhooks.loading")} />
          ) : (
            <EmptyState
              icon={Webhook}
              title={t("webhooks.empty.title")}
              description={t("webhooks.empty.description")}
              actionLabel={canCreate ? t("webhooks.add") : undefined}
              onAction={canCreate ? openCreate : undefined}
            />
          )
        }
        toolbar={
          <SearchInput
            value={search}
            onValueChange={(v) => { setSearch(v); setPage(0); }}
            placeholder={t("webhooks.searchPlaceholder")}
            className="pl-8"
          />
        }
      />

      <EntityEditorHost open={formOpen} onOpenChange={open => { if (!isMutating) setFormOpen(open); }}
        title={editingId ? t("webhooks.actions.edit") : t("webhooks.add")}
        footer={<>
          <Button data-editor-cancel variant="outline" disabled={isMutating} onClick={() => setFormOpen(false)}>{t("cancel")}</Button>
          <Button onClick={handleFormSubmit} disabled={isMutating || !canSubmit}>
            {isMutating && <Loader2 className="size-4 animate-spin" />}{editingId ? t("save") : t("create")}
          </Button>
        </>}>
          <div className="flex min-w-0 flex-col gap-6 pb-6">
            {(createMutation.error || updateMutation.error) && <p role="alert" className="text-sm text-status-negative">{t("error")}</p>}
            <fieldset disabled={isMutating} className="min-w-0">
            <FormSection contentClassName="flex flex-col gap-4">
            <div className="grid gap-1.5">
              <FieldLabel htmlFor="wh-name" required>{t("webhooks.fields.name")}</FieldLabel>
              <Input
                id="wh-name"
                value={form.name}
                onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
                placeholder={t("webhooks.fields.namePlaceholder")}
              />
            </div>

            <div className="grid gap-1.5">
              <FieldLabel htmlFor="wh-url" required>{t("webhooks.fields.targetUrl")}</FieldLabel>
              <Input
                id="wh-url"
                value={form.target_url}
                onChange={(e) => setForm((f) => ({ ...f, target_url: e.target.value }))}
                placeholder={t("webhooks.fields.targetUrlPlaceholder")}
              />
            </div>

            <div className="grid gap-1.5">
              <Label htmlFor="wh-secret">
                {t("webhooks.fields.secret")}
                <span className="ml-1 text-xs text-muted-foreground">{t("webhooks.fields.secretOptional")}</span>
              </Label>
              <Input
                id="wh-secret"
                type="password"
                value={form.secret}
                onChange={(e) => setForm((f) => ({ ...f, secret: e.target.value }))}
                placeholder={editingId ? t("webhooks.fields.secretKeepExisting") : t("webhooks.fields.secretMinChars")}
              />
            </div>

            </FormSection>
            <FormSection className="mt-6" contentClassName="flex flex-col gap-4">
            <div className="grid gap-2">
              <FieldLabel required>{t("webhooks.fields.eventTypes")}</FieldLabel>
              <WebhookEventPicker
                catalog={catalog}
                loading={catalogPending}
                value={form.event_types}
                onChange={(next) => setForm((f) => ({ ...f, event_types: next }))}
              />
            </div>

            <WebhookConditionsEditor
              logic={form.conditionLogic}
              rows={form.conditionRows}
              onChange={({ logic, rows }) =>
                setForm((f) => ({ ...f, conditionLogic: logic, conditionRows: rows }))
              }
            />

            <div className="flex items-center gap-2">
              <Checkbox
                id="wh-enabled"
                checked={form.enabled}
                onCheckedChange={(v) => setForm((f) => ({ ...f, enabled: Boolean(v) }))}
              />
              <Label htmlFor="wh-enabled" className="font-normal cursor-pointer">
                {t("webhooks.fields.enabled")}
              </Label>
            </div>
            </FormSection>
            </fieldset>
          </div>

      </EntityEditorHost>

      {/* Delete Confirmation */}
      <ConfirmDeleteDialog
        open={!!deleteTarget}
        onOpenChange={(open) => { if (!open && !deleteMutation.isPending) setDeleteTarget(null); }}
        onCloseAutoFocus={event => { if (deleteInvoker.current?.isConnected) { event.preventDefault(); deleteInvoker.current.focus(); } }}
        title={t("webhooks.actions.delete")}
        description={<Trans
          i18nKey="webhooks.deleteDescription"
          values={{ name: deleteTarget?.name ?? "" }}
          components={{ 1: <strong /> }}
        />}
        cancelLabel={t("cancel")}
        cancelDisabled={deleteMutation.isPending}
        confirmLabel={<>
          {deleteMutation.isPending && <Loader2 className="size-4 animate-spin" aria-hidden="true" />}
          {t("webhooks.actions.delete")}
        </>}
        confirmProps={{
          disabled: deleteMutation.isPending || !canDelete,
          "aria-busy": deleteMutation.isPending,
          onClick: event => { event.preventDefault(); if (deleteTarget && canDelete && !deleteMutation.isPending) deleteMutation.mutate(deleteTarget.id); },
        }}
      />
    </div>
  );
};

export default WebhooksPage;
