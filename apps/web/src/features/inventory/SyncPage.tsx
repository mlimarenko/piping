import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type { SyncJobStatus, SyncProvider } from "@/shared/api/types";
import type { Column, DataTableFilterOption, DataTableSortState } from "@/shared/ui-shared/DataTable";
import PageChrome from "@/shared/ui-shared/PageChrome";
import DataTable from "@/shared/ui-shared/DataTable";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import EmptyState from "@/shared/ui-shared/EmptyState";
import {
  createProvider as apiCreateProvider,
  createSyncJob as apiCreateSyncJob,
  deleteProvider as apiDeleteProvider,
  getLatestSyncJob as apiGetLatestSyncJob,
  listProviders as apiListProviders,
  updateProvider as apiUpdateProvider,
  validateProvider as apiValidateProvider,
} from "@/features/inventory/sync-api";
import type { ApiError } from "@/shared/api/client";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import FormSection from "@/shared/ui-shared/FormSection";
import DefinitionGrid from "@/shared/ui-shared/DefinitionGrid";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Switch } from "@/shared/ui-primitives/switch";
import { cn } from "@/shared/lib/utils";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import {
  mapDataTableSortKey,
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";
import {
  Trash2, RefreshCw, Play, Plus, Loader2, Save,
  Link2, Eye, EyeOff, Globe, Plug,
   ChevronRight, Database,
} from "lucide-react";
import { notify } from "@/shared/lib/notifications";
import {
  buildSyncProvidersFilterParams,
  SYNC_PROVIDERS_TABLE_CONFIG,
} from "@/features/inventory/syncProvidersTableConfig";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import { SYNC_PROVIDER_PARAM, syncProviderNavigation, syncSectionLinks } from "./syncSections";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import type { SearchableSelectOption } from "@/shared/ui-shared/SearchableSelect";
import type { LatestSyncJob } from "@/features/inventory/sync-api";
import { listMappingConfigurations, isRunnableMappingConfiguration } from "@/features/inventory/sync-mapping-api";

/* ── Provider registry ── */
const JOB_STATUS_LABEL: Record<SyncJobStatus, string> = {
  queued: "inventory.sync.jobStatusQueued",
  running: "inventory.sync.jobStatusRunning",
  completed: "inventory.sync.jobStatusCompleted",
  failed: "inventory.sync.jobStatusFailed",
};

/** Canonical job counters rendered only when the job actually reported them. */
const RUN_STAT_KEYS = [
  "entities_extracted",
  "entities_committed",
  "tag_memberships_committed",
] as const;

interface ProviderDef {
  id: string;
  name: string;
  icon: React.ElementType;
  color: string;
  description: string;
  fields: { key: string; label: string; placeholder: string; secret?: boolean; required?: boolean; inputType?: "url" }[];
  remoteEntities: string[];
}

function useProviders(): ProviderDef[] {
  const { t } = useTranslation();
  return [
    {
      id: "netbox",
      name: "NetBox",
      icon: Database,
      color: "text-status-positive",
      description: t("inventory.sync.providerNetboxDescription"),
      fields: [
        { key: "base_url", label: t("inventory.sync.netboxUrl"), placeholder: "https://netbox.example.com", required: true, inputType: "url" },
        { key: "api_token", label: t("inventory.sync.apiToken"), placeholder: "your-api-token-here", secret: true, required: true },
      ],
      remoteEntities: [
        "dcim.device", "dcim.site", "dcim.region", "dcim.device-role", "dcim.platform",
        "dcim.interface", "dcim.rack", "dcim.manufacturer",
        "virtualization.virtual-machine", "virtualization.cluster", "virtualization.vminterface",
        "tenancy.tenant", "tenancy.tenant-group",
        "ipam.ip-address", "ipam.prefix", "ipam.vlan", "ipam.vrf",
        "extras.tag", "extras.custom-field",
      ],
    },
    {
      id: "custom",
      name: "Custom API",
      icon: Globe,
      color: "text-status-info",
      description: t("inventory.sync.providerCustomDescription"),
      fields: [
        { key: "base_url", label: t("inventory.sync.baseUrl"), placeholder: "https://api.example.com", required: true, inputType: "url" },
        { key: "api_token", label: t("inventory.sync.bearerToken"), placeholder: "token", secret: true },
        { key: "auth_header", label: t("inventory.sync.authHeader"), placeholder: "Authorization" },
      ],
      remoteEntities: ["resource"],
    },
  ];
}

const SyncPage = () => {
  const { t } = useTranslation();
  const location = useLocation();
  const navigate = useNavigate();
  const providerDefs = useProviders();
  const qc = useQueryClient();

  // View-but-not-edit: hide/lock mutation affordances the caller can't use.
  // Backend guards (ResourceType::Inventory): create_provider→Create,
  // update_provider→Update, delete_provider→Delete, validate_provider &
  // create_sync_job→Execute.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "INVENTORY");
  const canUpdate = can("UPDATE", "INVENTORY");
  const canDelete = can("DELETE", "INVENTORY");
  const canExecute = can("EXECUTE", "INVENTORY");
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});
  const [page, setPage] = useState(0);
  const { value: pageSize, setValue: setPageSize } = useTablePageSize();
  const sanitizedSortState = sanitizeDataTableSortState(
    sortState,
    SYNC_PROVIDERS_TABLE_CONFIG.sortKeys,
  );
  const sanitizedColumnFilters = sanitizeDataTableColumnFilters(
    columnFilters,
    SYNC_PROVIDERS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
  );
  const filterParams = buildSyncProvidersFilterParams(sanitizedColumnFilters);
  const providersQuery = useQuery({
    queryKey: [
      "sync",
      "providers",
      {
        page,
        pageSize,
        sort: sanitizedSortState,
        filters: sanitizedColumnFilters,
      },
    ],
    queryFn: () =>
      apiListProviders({
        limit: pageSize,
        offset: page * pageSize,
        sort: mapDataTableSortKey(
          sanitizedSortState,
          SYNC_PROVIDERS_TABLE_CONFIG.sortKeyByColumn,
        ),
        order: sanitizedSortState?.dir,
        ...filterParams,
      }),
  });
  const providers: SyncProvider[] = useMemo(
    () => providersQuery.data?.items ?? [],
    [providersQuery.data],
  );
  const [searchParams] = useSearchParams();
  const [showAddDialog, setShowAddDialog] = useState(false);

  // The URL is the canonical source of provider selection (`?provider=<id>`), so
  // reload/back/forward keep the inspector identity, detail and actions. An
  // explicit param is authoritative and fails closed: when X is not on the
  // current page/filter (or was deleted), the inspector shows an empty/pending
  // state instead of silently switching to the first row (no X→Y). Only a
  // missing param falls back to the first provider (root default).
  const providerParam = searchParams.get(SYNC_PROVIDER_PARAM);
  const selectedId = providerParam ?? providers[0]?.id ?? null;
  const selected = providers.find((p) => p.id === selectedId);
  const selectedDef = selected ? providerDefs.find((d) => d.id === selected.providerId) : null;
  // Explicit param whose provider is absent from the loaded current results.
  const providerUnavailable = !!providerParam && !selected && !providersQuery.isFetching;

  const invalidate = () => qc.invalidateQueries({ queryKey: ["sync", "providers"] });

  const createMutation = useApiMutation({
    mutationFn: (body: Omit<SyncProvider, "id" | "secretFields">) =>
      apiCreateProvider(body),
    onSuccess: (created) => {
      void invalidate();
      void navigate(syncProviderNavigation(location, created.id), { replace: true });
      setShowAddDialog(false);
      notify("success", t("inventory.sync.providerAdded", { name: created.name }));
    },
    onError: (err: ApiError) => notify("error", err.message || t("inventory.sync.createFailed")),
  });

  const updateMutation = useApiMutation({
    mutationFn: ({ id, body }: { id: string; body: Partial<SyncProvider> }) =>
      apiUpdateProvider(id, body),
    onSuccess: (_provider, variables) => {
      void invalidate();
      // Only the connection-settings form persists `config`; the enable/disable
      // toggle is a separate, single-field action that must not claim a
      // "configuration saved" result.
      if (variables.body.config) {
        notify("success", t("inventory.sync.configSaved"));
      }
    },
    onError: (err: ApiError) => notify("error", err.message || t("inventory.sync.updateFailed")),
  });

  const deleteMutation = useApiMutation({
    mutationFn: (id: string) => apiDeleteProvider(id),
    onSuccess: () => {
      void invalidate();
      notify("success", t("inventory.sync.providerRemoved"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("inventory.sync.deleteFailed")),
  });

  const validateMutation = useApiMutation({
    mutationFn: (id: string) => apiValidateProvider(id),
    onSuccess: (result) => {
      if (result.ok) {
        notify("success", t("inventory.sync.connectionSuccessful"));
      } else {
        notify("error", result.message || t("inventory.sync.connectionFailed"));
      }
      void qc.invalidateQueries({ queryKey: ["sync", "providers"] });
    },
    onError: (err: ApiError) => notify("error", err.message || t("inventory.sync.validationFailed")),
  });

  const runSyncMutation = useApiMutation({
    mutationFn: ({ providerId, mappingRef }: { providerId: string; mappingRef: string }) =>
      apiCreateSyncJob(providerId, mappingRef),
    onSuccess: () => {
      notify("success", t("inventory.sync.syncStarted"));
      void invalidate();
      void qc.invalidateQueries({ queryKey: ["sync", "latest-job"] });
    },
    onError: (err: ApiError) => notify("error", err.message || t("inventory.sync.syncFailed")),
  });

  const addProvider = (providerId: string, name: string, config: Record<string, string>) => {
    createMutation.mutate({
      providerId,
      name,
      config,
      enabled: true,
    });
  };

  const updateProviderLocal = (id: string, patch: Partial<SyncProvider>) => {
    updateMutation.mutate({ id, body: patch });
  };

  const saveProviderConnection = (id: string, config: Record<string, string>) =>
    updateMutation.mutateAsync({ id, body: { config } });

  const deleteProvider = (id: string) => {
    deleteMutation.mutate(id);
  };

  const testConnection = (id: string) => {
    validateMutation.mutate(id);
  };

  const providerTypeOptions = useMemo<DataTableFilterOption[]>(
    () =>
      providerDefs.map((providerDef) => ({
        value: providerDef.id,
        label: providerDef.name,
        count: providersQuery.data?.items.filter(
          (provider) => provider.providerId === providerDef.id,
        ).length,
      })),
    [providerDefs, providersQuery.data?.items],
  );
  const enabledOptions = useMemo<DataTableFilterOption[]>(
    () => [
      {
        value: "true",
        label: t("enabled"),
        count: providersQuery.data?.items.filter((provider) => provider.enabled).length,
      },
      {
        value: "false",
        label: t("disabled"),
        count: providersQuery.data?.items.filter((provider) => !provider.enabled).length,
      },
    ],
    [providersQuery.data?.items, t],
  );
  const columns = useMemo<Column<SyncProvider>[]>(
    () => [
      {
        key: "name",
        label: t("name"),
        sortable: true,
        getSortValue: (provider) => provider.name,
        render: (provider) => {
          const def = providerDefs.find((entry) => entry.id === provider.providerId);
          return (
            <div className="flex min-w-0 items-center gap-2.5">
              <div className="flex size-9 shrink-0 items-center justify-center rounded-md surface-inset">
                {def ? <def.icon className={cn("h-3.5 w-3.5", def.color)} /> : <Plug className="h-3.5 w-3.5 text-muted-foreground" />}
              </div>
              <div className="min-w-0">
                <div className="truncate text-xs font-medium text-foreground">{provider.name}</div>
              </div>
            </div>
          );
        },
      },
      {
        key: "providerType",
        label: t("inventory.sync.providerType", { defaultValue: "Provider type" }),
        sortable: true,
        filterable: true,
        filterOptions: providerTypeOptions,
        getSortValue: (provider) => provider.providerId,
        render: (provider) => {
          const def = providerDefs.find((entry) => entry.id === provider.providerId);
          return (
            <span className="truncate text-xs text-muted-foreground">
              {def?.name ?? provider.providerId}
            </span>
          );
        },
      },
      {
        key: "enabled",
        label: t("enabled"),
        sortable: true,
        filterable: true,
        filterOptions: enabledOptions,
        getSortValue: (provider) => provider.enabled,
        render: (provider) => (
          <span className={cn("text-xs font-medium", provider.enabled ? "text-status-positive" : "text-muted-foreground")}>
            {provider.enabled ? t("enabled") : t("disabled")}
          </span>
        ),
      },
    ],
    [enabledOptions, providerDefs, providerTypeOptions, t],
  );

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <PageChrome title={t("inventory.sync.title")} description={t("inventory.sync.description")}
        actions={
          canCreate ? (
            <Button size="sm" className="h-9 text-sm gap-1" onClick={() => setShowAddDialog(true)}>
              <Plus className="h-3 w-3" /> {t("inventory.sync.addProvider")}
            </Button>
          ) : undefined
        }
      />

      <SectionLinks label={t("inventory.sync.sectionsLabel")} activeValue="providers"
        items={syncSectionLinks(t, { providers: providersQuery.data?.total ?? 0 }, location, "providers", providerParam ?? undefined)} />

      <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_24rem]">
        <DataTable
          tableId="inventory.syncProviders"
          className="max-h-[60vh] min-h-0"
          columns={columns}
          data={providers}
          getRowKey={(provider) => provider.id}
          selectedRow={selected ?? null}
          onRowClick={(provider) => navigate(syncProviderNavigation(location, provider.id))}
          loading={providersQuery.isPending}
          error={providersQuery.isError ? providersQuery.error : undefined}
          onRetry={() => providersQuery.refetch()}
          sortState={sanitizedSortState}
          onSortChange={(next) => {
            setSortState(next);
            setPage(0);
          }}
          columnFilters={sanitizedColumnFilters}
          onColumnFilterChange={(key, values) => {
            setColumnFilters((current) => updateDataTableColumnFilters(current, key, values));
            setPage(0);
          }}
          pagination={{
            page,
            pageSize,
            total: providersQuery.data?.total ?? 0,
            onPageChange: setPage,
            onPageSizeChange: (next) => {
              setPageSize(next);
              setPage(0);
            },
          }}
          emptyState={
            <EmptyState
              icon={Plug}
              title={t("inventory.sync.noProvidersConfigured")}
              description={t("inventory.sync.noProvidersConfiguredDescription")}
              actionLabel={canCreate ? t("inventory.sync.addProvider") : undefined}
              onAction={canCreate ? () => setShowAddDialog(true) : undefined}
            />
          }
          toolbar={
            <div className="flex min-w-0 items-center gap-2 px-1 text-xs font-medium text-muted-foreground">
              <Plug className="h-3.5 w-3.5 text-primary" />
              {t("inventory.sync.providersCount", {
                count: providersQuery.data?.total ?? 0,
              })}
            </div>
          }
        />

        {selected && selectedDef ? (
          <ProviderInspector
            key={selected.id}
            provider={selected}
            def={selectedDef}
            canUpdate={canUpdate}
            canDelete={canDelete}
            canExecute={canExecute}
            saving={updateMutation.isPending}
            testing={validateMutation.isPending}
            running={runSyncMutation.isPending}
            onSave={(config) => saveProviderConnection(selected.id, config)}
            onToggleEnabled={(enabled) => updateProviderLocal(selected.id, { enabled })}
            onDelete={() => deleteProvider(selected.id)}
            onTest={() => testConnection(selected.id)}
            onRefresh={() => providersQuery.refetch()}
            onRunSync={(mappingRef) => runSyncMutation.mutate({
              providerId: selected.id,
              mappingRef,
            })}
          />
        ) : (
          <SurfaceCard className="flex min-h-0 items-center justify-center">
            <EmptyState
              icon={Plug}
              title={providerUnavailable ? t("inventory.sync.providerUnavailable") : t("inventory.sync.selectProvider")}
              description={providerUnavailable ? t("inventory.sync.providerUnavailableDescription") : t("inventory.sync.selectProviderDescription")}
              actionLabel={providerUnavailable ? t("inventory.sync.clearProviderSelection") : undefined}
              onAction={providerUnavailable ? () => navigate(syncProviderNavigation(location, null), { replace: true }) : undefined}
              className="py-10"
            />
          </SurfaceCard>
        )}
      </div>

      <SyncProviderEditor open={showAddDialog} onOpenChange={setShowAddDialog} onAdd={addProvider} />
    </div>
  );
};

/* ── Provider Inspector ── */
const ProviderInspector = ({ provider, def, canUpdate, canDelete, canExecute, saving, testing, running, onSave, onToggleEnabled, onDelete, onTest, onRefresh, onRunSync }: {
  provider: SyncProvider; def: ProviderDef;
  canUpdate: boolean;
  canDelete: boolean;
  canExecute: boolean;
  saving: boolean;
  testing: boolean;
  running: boolean;
  onSave: (config: Record<string, string>) => Promise<unknown>;
  onToggleEnabled: (enabled: boolean) => void;
  onDelete: () => void;
  onTest: () => void;
  onRefresh: () => void;
  onRunSync: (mappingRef: string) => void;
}) => {
  const { t } = useTranslation();
  // Pending edits, seeded once per selected provider (the parent remounts this
  // inspector with `key={provider.id}`). Non-secret fields prefill from the wire
  // config; secret fields always start blank because the wire never returns the
  // stored value, and an unchanged blank must not overwrite it.
  const [draft, setDraft] = useState<Record<string, string>>(() => {
    const initial: Record<string, string> = {};
    for (const field of def.fields) {
      if (!field.secret) initial[field.key] = provider.config[field.key] ?? "";
    }
    return initial;
  });
  const [secretDrafts, setSecretDrafts] = useState<Record<string, string>>({});
  const [revealed, setRevealed] = useState<Record<string, boolean>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const inputRefs = useRef<Record<string, HTMLInputElement | null>>({});

  // A sync job pins one immutable mapping configuration version. The picker
  // lists the provider's configurations and sends the canonical
  // `<logical_id>:<version>` reference — never a free-form or implicit version.
  const [mappingRef, setMappingRef] = useState("");
  const mappingConfigsQuery = useQuery({
    queryKey: ["sync", "mapping-configurations", provider.id],
    queryFn: () => listMappingConfigurations({ provider_id: provider.id, limit: 200, offset: 0 }),
  });
  const mappingConfigs = mappingConfigsQuery.data?.items ?? [];
  // Only the supported job shape is runnable: an active config that pulls into
  // the local inventory with NetBox-wins resolution. The predicate is explicit
  // about the target, so a provider-target config is never offered as runnable.
  const mappingItems = mappingConfigs.map((config) => ({
    id: `${config.logical_id}:${config.version}`,
    label: `${config.name} · v${config.version}`,
    searchText: config.logical_id,
    disabled: !isRunnableMappingConfiguration(config),
  }));
  const selectedMapping =
    mappingConfigs.find((config) => `${config.logical_id}:${config.version}` === mappingRef) ?? null;
  const selectedMappingRunnable =
    selectedMapping != null && isRunnableMappingConfiguration(selectedMapping);
  const hasRunnableMapping = mappingConfigs.some(isRunnableMappingConfiguration);

  const latestJobQuery = useQuery({
    queryKey: ["sync", "latest-job", provider.id],
    queryFn: () => apiGetLatestSyncJob(provider.id),
  });
  const latestJob = latestJobQuery.data ?? null;
  const latestJobStats = latestJob
    ? RUN_STAT_KEYS.filter((key) => typeof latestJob.stats[key] === "number")
    : [];

  const secretHasValue = (key: string) =>
    provider.secretFields.find((secret) => secret.fieldKey === key)?.hasValue ?? false;

  // Only meaningful edits become the PATCH body: a blank secret is omitted so
  // the backend merge keeps the stored write-only value, and an unchanged field
  // is omitted so an unrelated config key is never overwritten.
  const buildPatch = (): Record<string, string> => {
    const patch: Record<string, string> = {};
    for (const field of def.fields) {
      if (field.secret) {
        const value = (secretDrafts[field.key] ?? "").trim();
        if (value) patch[field.key] = value;
      } else {
        const value = draft[field.key] ?? "";
        if (value !== (provider.config[field.key] ?? "")) patch[field.key] = value;
      }
    }
    return patch;
  };
  const patch = buildPatch();
  const hasChanges = Object.keys(patch).length > 0;

  const validate = (): Record<string, string> => {
    const next: Record<string, string> = {};
    for (const field of def.fields) {
      const value = field.secret
        ? (secretDrafts[field.key] ?? "").trim()
        : (draft[field.key] ?? "").trim();
      // A required secret is satisfied by an already-stored value; a blank input
      // means "keep it", not "clear it".
      const required = field.required && !(field.secret && secretHasValue(field.key));
      if (required && !value) next[field.key] = t("required");
    }
    return next;
  };

  const handleSave = async () => {
    const nextErrors = validate();
    setErrors(nextErrors);
    const firstInvalid = def.fields.find((field) => nextErrors[field.key]);
    if (firstInvalid) {
      inputRefs.current[firstInvalid.key]?.focus();
      return;
    }
    if (!hasChanges) return;
    try {
      await onSave(patch);
      // The stored secret now has a value; drop the entered one from state.
      setSecretDrafts({});
    } catch {
      // The parent mutation reports the failure through a toast.
    }
  };

  const visibleEntities = def.remoteEntities.slice(0, 8);
  const hiddenEntityCount = Math.max(0, def.remoteEntities.length - visibleEntities.length);

  return (
    <aside className="flex min-w-0 flex-col gap-6">
      <ProviderHeaderCard
        provider={provider}
        def={def}
        canUpdate={canUpdate}
        saving={saving}
        onToggleEnabled={onToggleEnabled}
      />

      <FormSection title={t("inventory.sync.connectionSettings")} description={t("inventory.sync.connectionSettingsDescription")} divider>
        <div className="flex flex-col gap-3">
          {def.fields.map((field) => {
            const inputId = `sync-provider-field-${provider.id}-${field.key}`;
            const errorId = `${inputId}-error`;
            return (
              <ProviderConfigField
                key={field.key}
                inputId={inputId}
                errorId={errorId}
                field={field}
                error={errors[field.key]}
                canUpdate={canUpdate}
                draftValue={draft[field.key] ?? ""}
                secretValue={secretDrafts[field.key] ?? ""}
                revealed={revealed[field.key] ?? false}
                hasStoredSecret={field.secret === true && secretHasValue(field.key)}
                onDraftChange={(value) => setDraft((prev) => ({ ...prev, [field.key]: value }))}
                onSecretChange={(value) => setSecretDrafts((prev) => ({ ...prev, [field.key]: value }))}
                onToggleReveal={() => setRevealed((prev) => ({ ...prev, [field.key]: !prev[field.key] }))}
                inputRef={(node) => { inputRefs.current[field.key] = node; }}
              />
            );
          })}
        </div>
        <div className="flex flex-wrap items-center gap-2 border-t border-border pt-2">
          {canUpdate && (
            <Button
              type="button"
              size="sm"
              className="h-9 min-w-[7rem] flex-1 gap-1.5 text-sm"
              disabled={saving || !hasChanges}
              onClick={handleSave}
            >
              {saving ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Save className="h-3.5 w-3.5" />}
              {t("save")}
            </Button>
          )}
          {canExecute && (
            <Button size="sm" variant="outline" className="h-9 gap-1.5 text-sm" onClick={onTest} disabled={testing}>
              {testing ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Link2 className="h-3.5 w-3.5" />}
              {t("inventory.sync.testConnection")}
            </Button>
          )}
          <Button size="icon" variant="outline" className="size-9" onClick={onRefresh} aria-label={t("refresh")}>
            <RefreshCw className="h-3.5 w-3.5" />
          </Button>
        </div>
      </FormSection>

      <RunSyncCard
        providerId={provider.id}
        canExecute={canExecute}
        mappingConfigsPending={mappingConfigsQuery.isPending}
        mappingConfigsError={mappingConfigsQuery.isError}
        onRetryMappingConfigs={() => { void mappingConfigsQuery.refetch(); }}
        mappingConfigsEmpty={mappingConfigs.length === 0}
        mappingItems={mappingItems}
        mappingRef={mappingRef}
        onMappingRefChange={setMappingRef}
        hasSelectedMapping={selectedMapping != null}
        selectedMappingRunnable={selectedMappingRunnable}
        hasRunnableMapping={hasRunnableMapping}
        running={running}
        onRunSync={onRunSync}
        latestJobPending={latestJobQuery.isPending}
        latestJobError={latestJobQuery.isError}
        latestJob={latestJob}
        latestJobStats={latestJobStats}
      />

      <FormSection title={t("inventory.sync.sourceEntities")} description={t("inventory.sync.sourceEntitiesDescription")} divider>
        <div className="flex flex-wrap gap-1.5">
          {visibleEntities.map((entity) => (
            <span key={entity} className="rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-xs text-muted-foreground">
              {entity}
            </span>
          ))}
          {hiddenEntityCount > 0 ? (
            <span className="rounded border border-border bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
              {t("inventory.sync.moreEntities", { count: hiddenEntityCount })}
            </span>
          ) : null}
        </div>
      </FormSection>

      {canDelete && (
        <FormSection title={t("inventory.sync.dangerZone")} description={t("inventory.sync.dangerZoneDescription")} divider>
          <Button size="sm" variant="destructive" className="h-9 w-full gap-1.5 text-sm" onClick={onDelete}>
            <Trash2 className="h-3.5 w-3.5" /> {t("remove")}
          </Button>
        </FormSection>
      )}
    </aside>
  );
};

/* ── Provider Header ── */
const ProviderHeaderCard = ({
  provider,
  def,
  canUpdate,
  saving,
  onToggleEnabled,
}: {
  provider: SyncProvider;
  def: ProviderDef;
  canUpdate: boolean;
  saving: boolean;
  onToggleEnabled: (enabled: boolean) => void;
}) => {
  const { t } = useTranslation();
  return (
    <FormSection title={t("inventory.sync.inspectorTitle")}>
      <div className="flex flex-col gap-4">
        <div className="flex items-start gap-3">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-border bg-primary/10">
            <def.icon className={cn("h-5 w-5", def.color)} />
          </div>
          <div className="min-w-0 flex-1">
            <h3 className="truncate text-sm font-semibold leading-tight text-foreground">
              {provider.name}
            </h3>
            <p className="mt-1 line-clamp-2 text-xs leading-relaxed text-muted-foreground">
              {def.description}
            </p>
          </div>
          <EnabledBadge enabled={provider.enabled} />
        </div>

        <DefinitionGrid fields={[
          { label: t("inventory.sync.providerType"), value: def.name },
          { label: t("enabled"), value: (
              <div className="flex items-center justify-between gap-2">
                <span>{provider.enabled ? t("enabled") : t("disabled")}</span>
                {canUpdate && (
                  <Switch
                    checked={provider.enabled}
                    disabled={saving}
                    onCheckedChange={onToggleEnabled}
                    aria-label={t("inventory.sync.toggleProvider")}
                  />
                )}
              </div>
            ) },
        ]} />
      </div>
    </FormSection>
  );
};

/* ── Provider Connection Field ── */
interface ProviderConfigFieldProps {
  inputId: string;
  errorId: string;
  field: ProviderDef["fields"][number];
  error?: string;
  canUpdate: boolean;
  draftValue: string;
  secretValue: string;
  revealed: boolean;
  hasStoredSecret: boolean;
  onDraftChange: (value: string) => void;
  onSecretChange: (value: string) => void;
  onToggleReveal: () => void;
  inputRef: (node: HTMLInputElement | null) => void;
}

const ProviderSecretInput = ({
  inputId, errorId, field, error, canUpdate, secretValue, revealed,
  hasStoredSecret, onSecretChange, onToggleReveal, inputRef,
}: Omit<ProviderConfigFieldProps, "draftValue" | "onDraftChange">) => {
  const { t } = useTranslation();
  return (
    <div className="flex gap-1">
      <Input
        id={inputId}
        ref={inputRef}
        type={revealed ? "text" : "password"}
        className="h-9 min-w-0 flex-1 border-border text-sm"
        value={secretValue}
        onChange={(e) => onSecretChange(e.target.value)}
        placeholder={hasStoredSecret ? t("inventory.sync.secretKeepPlaceholder") : field.placeholder}
        autoComplete="new-password"
        readOnly={!canUpdate}
        aria-invalid={error ? true : undefined}
        aria-describedby={error ? errorId : undefined}
      />
      {secretValue.length > 0 && (
        <Button
          type="button"
          variant="outline"
          size="icon"
          className="size-9"
          aria-label={revealed ? t("inventory.sync.hideSecret") : t("inventory.sync.showSecret")}
          aria-pressed={revealed}
          onClick={onToggleReveal}
        >
          {revealed ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
        </Button>
      )}
    </div>
  );
};

const ProviderConfigField = ({
  inputId,
  errorId,
  field,
  error,
  canUpdate,
  draftValue,
  secretValue,
  revealed,
  hasStoredSecret,
  onDraftChange,
  onSecretChange,
  onToggleReveal,
  inputRef,
}: ProviderConfigFieldProps) => {
  const { t } = useTranslation();
  return (
    <div className="min-w-0">
      <FieldLabel
        htmlFor={inputId}
        className="mb-1 flex items-center gap-1 text-sm font-medium"
        required={field.required}
      >
        {field.label}
      </FieldLabel>
      {field.secret ? (
        <ProviderSecretInput
          inputId={inputId} errorId={errorId} field={field} error={error}
          canUpdate={canUpdate} secretValue={secretValue} revealed={revealed}
          hasStoredSecret={hasStoredSecret} onSecretChange={onSecretChange}
          onToggleReveal={onToggleReveal} inputRef={inputRef}
        />
      ) : (
        <Input
          id={inputId}
          ref={inputRef}
          type={field.inputType ?? "text"}
          className="h-9 border-border text-sm"
          value={draftValue}
          onChange={(e) => onDraftChange(e.target.value)}
          placeholder={field.placeholder}
          autoComplete="off"
          readOnly={!canUpdate}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? errorId : undefined}
        />
      )}
      {field.secret && (
        <p className="mt-1 text-xs leading-4 text-muted-foreground">
          {hasStoredSecret ? t("inventory.sync.secretHasValue") : t("inventory.sync.secretMissing")}
        </p>
      )}
      {error && (
        <p id={errorId} className="mt-1 text-xs text-destructive">{error}</p>
      )}
    </div>
  );
};

/* ── Run Sync ── */
const MappingRefHint = ({
  hasRunnableMapping,
  hasSelectedMapping,
  mappingRef,
}: {
  hasRunnableMapping: boolean;
  hasSelectedMapping: boolean;
  mappingRef: string;
}) => {
  const { t } = useTranslation();
  if (!hasRunnableMapping) {
    return <p className="text-xs text-muted-foreground">{t("inventory.sync.runnableConfigurationRequired")}</p>;
  }
  if (hasSelectedMapping) {
    return (
      <p className="text-xs text-muted-foreground">
        {t("inventory.sync.mappingRefHelp", { ref: mappingRef })}
      </p>
    );
  }
  return null;
};

const LatestJobSummary = ({
  job,
  stats,
}: {
  job: LatestSyncJob;
  stats: readonly (typeof RUN_STAT_KEYS)[number][];
}) => {
  const { t } = useTranslation();
  return (
    <>
      <span>
        {t(JOB_STATUS_LABEL[job.status])}
        {job.finishedAt ? ` · ${job.finishedAt}` : ""}
      </span>
      {stats.length > 0 ? (
        <span className="font-mono">
          {stats.map((key) => `${key}=${String(job.stats[key])}`).join(" · ")}
        </span>
      ) : null}
    </>
  );
};

const LatestJobPanel = ({
  pending,
  error,
  job,
  stats,
}: {
  pending: boolean;
  error: boolean;
  job: LatestSyncJob | null;
  stats: readonly (typeof RUN_STAT_KEYS)[number][];
}) => {
  const { t } = useTranslation();
  let body: React.ReactNode;
  if (pending) {
    body = <span>{t("inventory.sync.latestJobLoading")}</span>;
  } else if (error) {
    body = <span className="text-destructive">{t("inventory.sync.latestJobLoadFailed")}</span>;
  } else if (job) {
    body = <LatestJobSummary job={job} stats={stats} />;
  } else {
    body = <span>{t("inventory.sync.latestJobNone")}</span>;
  }
  return (
    <div className="flex flex-col gap-0.5 border-t border-border pt-2 text-xs text-muted-foreground">
      <span className="font-medium text-foreground">{t("inventory.sync.latestJob")}</span>
      {body}
    </div>
  );
};

const RunSyncCard = ({
  providerId,
  canExecute,
  mappingConfigsPending,
  mappingConfigsError,
  onRetryMappingConfigs,
  mappingConfigsEmpty,
  mappingItems,
  mappingRef,
  onMappingRefChange,
  hasSelectedMapping,
  selectedMappingRunnable,
  hasRunnableMapping,
  running,
  onRunSync,
  latestJobPending,
  latestJobError,
  latestJob,
  latestJobStats,
}: {
  providerId: string;
  canExecute: boolean;
  mappingConfigsPending: boolean;
  mappingConfigsError: boolean;
  onRetryMappingConfigs: () => void;
  mappingConfigsEmpty: boolean;
  mappingItems: SearchableSelectOption[];
  mappingRef: string;
  onMappingRefChange: (value: string) => void;
  hasSelectedMapping: boolean;
  selectedMappingRunnable: boolean;
  hasRunnableMapping: boolean;
  running: boolean;
  onRunSync: (mappingRef: string) => void;
  latestJobPending: boolean;
  latestJobError: boolean;
  latestJob: LatestSyncJob | null;
  latestJobStats: readonly (typeof RUN_STAT_KEYS)[number][];
}) => {
  const { t } = useTranslation();
  let content: React.ReactNode;
  if (!canExecute) {
    content = null;
  } else if (mappingConfigsPending) {
    content = (
      <p className="flex items-center gap-2 text-xs text-muted-foreground">
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
        {t("inventory.sync.mappingConfigurationsLoading")}
      </p>
    );
  } else if (mappingConfigsError) {
    content = (
      <div className="flex flex-col gap-2">
        <p className="text-xs text-destructive">{t("inventory.sync.mappingConfigurationsLoadFailed")}</p>
        <Button size="sm" variant="outline" className="h-9 w-fit gap-1.5 text-sm" onClick={onRetryMappingConfigs}>
          <RefreshCw className="h-3.5 w-3.5" />
          {t("retry")}
        </Button>
      </div>
    );
  } else if (mappingConfigsEmpty) {
    content = (
      <div className="flex flex-col gap-2">
        <p className="text-xs text-muted-foreground">{t("inventory.sync.noMappingConfigurations")}</p>
        <Button asChild size="sm" variant="outline" className="h-9 w-fit gap-1.5 text-sm">
          <Link to={`/inventory/sync/${providerId}/mappings/new`}>
            <Plus className="h-3.5 w-3.5" />
            {t("inventory.sync.createMappingConfiguration")}
          </Link>
        </Button>
      </div>
    );
  } else {
    content = (
      <div className="flex flex-col gap-2">
        <div className="flex flex-col gap-1">
          <span className="text-xs font-medium text-muted-foreground">
            {t("inventory.sync.mappingConfiguration")}
          </span>
          <SearchableSelect
            id={`sync-run-mapping-${providerId}`}
            aria-label={t("inventory.sync.mappingConfiguration")}
            items={mappingItems}
            value={mappingRef}
            onValueChange={onMappingRefChange}
            placeholder={t("inventory.sync.selectMappingConfiguration")}
            searchPlaceholder={t("inventory.sync.searchMappingConfiguration")}
            emptyText={t("inventory.sync.noMappingConfigurations")}
          />
        </div>
        <Button
          size="sm"
          className="h-9 w-fit gap-1.5 text-sm"
          disabled={!selectedMappingRunnable || running}
          onClick={() => { if (selectedMappingRunnable) onRunSync(mappingRef); }}
        >
          {running ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Play className="h-3.5 w-3.5" />}
          {t("inventory.sync.syncNow")}
        </Button>
        <MappingRefHint
          hasRunnableMapping={hasRunnableMapping}
          hasSelectedMapping={hasSelectedMapping}
          mappingRef={mappingRef}
        />
        <LatestJobPanel
          pending={latestJobPending}
          error={latestJobError}
          job={latestJob}
          stats={latestJobStats}
        />
      </div>
    );
  }

  return (
    <FormSection title={t("inventory.sync.runSync")} description={t("inventory.sync.runSyncDescription")} divider>
      {content}
    </FormSection>
  );
};

/* ── Add Provider Editor ── */
const SyncProviderEditor = ({ open, onOpenChange, onAdd }: {
  open: boolean; onOpenChange: (v: boolean) => void;
  onAdd: (providerId: string, name: string, config: Record<string, string>) => void;
}) => {
  const { t } = useTranslation();
  const providerDefs = useProviders();
  const [step, setStep] = useState<"select" | "configure">("select");
  const [selectedProvider, setSelectedProvider] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [config, setConfig] = useState<Record<string, string>>({});
  const [showSecrets, setShowSecrets] = useState<Record<string, boolean>>({});

  const def = providerDefs.find(d => d.id === selectedProvider);
  const reset = () => { setStep("select"); setSelectedProvider(null); setName(""); setConfig({}); };

  return (
    <EntityEditorHost
      open={open}
      onOpenChange={v => { if (!v) { reset(); } onOpenChange(v); }}
      title={<span className="flex items-center gap-2">
        <Plug className="h-4 w-4 text-primary" />
        {step === "select" ? t("inventory.sync.chooseProvider") : t("inventory.sync.configure", { name: def?.name ?? "" })}
      </span>}
      description={t("inventory.sync.addProviderDescription")}
      footer={
        <>
          {step === "configure" && (
            <Button variant="ghost" size="sm" className="h-9 text-sm" onClick={() => setStep("select")}>← {t("back")}</Button>
          )}
          <div className="flex-1" />
          <Button data-editor-cancel variant="outline" size="sm" className="h-9 text-sm" onClick={() => { reset(); onOpenChange(false); }}>{t("cancel")}</Button>
          {step === "configure" && (
            <Button size="sm" className="h-9 text-sm gap-1" disabled={!name.trim()}
              onClick={() => { onAdd(selectedProvider!, name, config); reset(); }}>
              <Plus className="h-3 w-3" /> {t("inventory.sync.addProvider")}
            </Button>
          )}
        </>
      }
    >
      <div className="min-w-0">
      {step === "select" ? (
          <div className="grid gap-2">
            {providerDefs.map(p => (
              <Button variant="ghost" type="button" key={p.id} onClick={() => { setSelectedProvider(p.id); setStep("configure"); setName(`${p.name} ${t("inventory.sync.instanceSuffix")}`); }}
                className="h-auto w-full justify-start gap-3 whitespace-normal rounded-lg border border-border p-3 text-left transition-colors hover:border-primary hover:bg-accent group">
                <div className="size-10 shrink-0 rounded-lg surface-inset flex items-center justify-center">
                  <p.icon className={cn("h-5 w-5", p.color)} />
                </div>
                <div className="flex-1">
                  <div className="text-sm font-medium text-foreground">{p.name}</div>
                  <div className="text-xs text-muted-foreground">{p.description}</div>
                </div>
                <ChevronRight className="h-4 w-4 text-muted-foreground group-hover:text-foreground transition-colors" />
              </Button>
            ))}
          </div>
        ) : def ? (
          <div className="space-y-3">
            <div>
              <FieldLabel htmlFor="sync-provider-new-name" className="text-sm font-medium mb-1 block" required>{t("inventory.sync.displayName")}</FieldLabel>
              <Input id="sync-provider-new-name" className="h-9 text-sm border-border" value={name} onChange={e => setName(e.target.value)} autoComplete="off" />
            </div>
            {def.fields.map(f => {
              const inputId = `sync-provider-new-${f.key}`;
              return (
                <div key={f.key}>
                  <FieldLabel htmlFor={inputId} className="text-sm font-medium mb-1 flex items-center gap-1" required={f.required}>
                    {f.label}
                  </FieldLabel>
                  {f.secret ? (
                    <div className="flex gap-1">
                      <Input id={inputId} type={showSecrets[f.key] ? "text" : "password"} className="h-9 text-sm flex-1 border-border"
                        value={config[f.key] ?? ""} onChange={e => setConfig(p => ({ ...p, [f.key]: e.target.value }))} placeholder={f.placeholder} autoComplete="new-password" />
                      <Button
                        variant="outline"
                        size="icon"
                        className="size-9"
                        aria-label={showSecrets[f.key] ? t("inventory.sync.hideSecret") : t("inventory.sync.showSecret")}
                        aria-pressed={!!showSecrets[f.key]}
                        onClick={() => setShowSecrets(p => ({ ...p, [f.key]: !p[f.key] }))}
                      >
                        {showSecrets[f.key] ? <EyeOff className="h-3 w-3" /> : <Eye className="h-3 w-3" />}
                      </Button>
                    </div>
                  ) : (
                    <Input id={inputId} type={f.inputType ?? "text"} className="h-9 text-sm border-border" value={config[f.key] ?? ""}
                      onChange={e => setConfig(p => ({ ...p, [f.key]: e.target.value }))} placeholder={f.placeholder} autoComplete="off" />
                  )}
                </div>
              );
            })}
          </div>
        ) : null}
      </div>
    </EntityEditorHost>
  );
};

/* ── Shared ── */
// The only provider state the API returns is `is_enabled`. There is no
// transport status until the backend last-job API exists, so the inspector
// shows the truthful configured state instead of a fabricated one.
const EnabledBadge = ({ enabled }: { enabled: boolean }) => {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-1.5">
      <span
        aria-hidden="true"
        className={cn("h-1.5 w-1.5 rounded-full", enabled ? "bg-status-positive" : "bg-muted-foreground/50")}
      />
      <span className="text-xs text-muted-foreground">{enabled ? t("enabled") : t("disabled")}</span>
    </div>
  );
};

export default SyncPage;
