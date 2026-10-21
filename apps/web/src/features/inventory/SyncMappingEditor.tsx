import { useState, useCallback, useRef, type Dispatch, type SetStateAction } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { ApiError } from "@/shared/api/client";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useLocation, useNavigate, useParams } from "react-router";
import {
  AlertTriangle,
  Info,
  Layers,
  Plus,
} from "lucide-react";
import { notify } from "@/shared/lib/notifications";

import PageLayout from "@/shared/ui-shared/PageLayout";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import DefinitionGrid from "@/shared/ui-shared/DefinitionGrid";
import FormSection from "@/shared/ui-shared/FormSection";
import FormField from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Skeleton } from "@/shared/ui-primitives/skeleton";
import { Switch } from "@/shared/ui-primitives/switch";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import { cn } from "@/shared/lib/utils";
import type { MappingConfiguration, MappingTarget, EntityMapping, FieldMapping } from "@/shared/api/types";
import { listProviders } from "@/features/inventory/sync-api";
import {
  getMappingConfiguration,
  createMappingConfiguration,
  updateMappingConfiguration,
  getLocalInventoryResource,
} from "@/features/inventory/sync-mapping-api";

import FieldTreeRail from "@/features/inventory/components/sync-mapping/FieldTreeRail";
import ExpressionBuilder from "@/features/inventory/components/sync-mapping/ExpressionBuilder";
import PreviewDiff from "@/features/inventory/components/sync-mapping/PreviewDiff";
import ConflictDialog from "@/features/inventory/components/sync-mapping/ConflictDialog";
import AddEntityDialog from "@/features/inventory/components/sync-mapping/AddEntityDialog";

type EditorMode = "create" | "edit" | "readonly";

interface SelectedField {
  entityId: string;
  fieldId: string;
}

interface WorkbenchSelection {
  selectedField: SelectedField | null;
  setSelectedField: Dispatch<SetStateAction<SelectedField | null>>;
  selectedEntityId: string | null;
  setSelectedEntityId: Dispatch<SetStateAction<string | null>>;
  activeTab: "expression" | "preview" | "settings";
  setActiveTab: Dispatch<SetStateAction<"expression" | "preview" | "settings">>;
}

function makeDraftConfiguration(providerId: string, name: string): MappingConfiguration {
  const now = new Date().toISOString();
  const logicalId = crypto.randomUUID();
  return {
    id: `draft-${logicalId}`,
    logical_id: logicalId,
    name,
    version: 0,
    is_latest: true,
    provider_id: providerId,
    // A new configuration defaults to the supported shape: the typed local
    // inventory target with NetBox-wins resolution, pulling from the provider.
    target: { kind: "local_inventory" },
    status: "active",
    direction: "pull",
    conflict_mode: "netbox_wins",
    entity_mappings: [],
    created_at: now,
    in_flight_job_count: 0,
  };
}

function deriveTargetField(sourceField: string): string {
  const parts = sourceField.split(".").map((part) => part.trim()).filter(Boolean);
  return parts.at(-1) ?? sourceField;
}

function makeFieldMapping(sourceField: string, targetField: string): FieldMapping {
  return {
    id: `fm-${crypto.randomUUID()}`,
    source_field: sourceField,
    target_field: targetField,
    enabled: true,
    forward_transform: {
      operator: "identity",
      args: { field: sourceField },
    },
    reverse_transform: null,
  };
}

/** Select wire value for a mapping target: `local_inventory` or `provider:<uuid>`. */
function targetToSelectValue(target: MappingTarget): string {
  return target.kind === "local_inventory" ? "local_inventory" : `provider:${target.provider_id}`;
}

function selectValueToTarget(value: string): MappingTarget {
  return value.startsWith("provider:")
    ? { kind: "provider", provider_id: value.slice("provider:".length) }
    : { kind: "local_inventory" };
}

const SyncMappingEditor = () => {
  const { id, mode, providerId } = useParams<{ id?: string; mode?: string; providerId?: string }>();
  const { can } = usePermissions();
  // Workbench selection survives saving a new version; the authoring draft does not.
  const [selectedField, setSelectedField] = useState<SelectedField | null>(null);
  const [selectedEntityId, setSelectedEntityId] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<"expression" | "preview" | "settings">("expression");
  // Authoring state belongs to one route identity and capability scope. Leaving
  // it discards the draft without invalidating the saved configuration cache.
  const scope = JSON.stringify([id, mode, providerId, can("UPDATE", "INVENTORY")]);
  return <SyncMappingEditorState key={scope} selection={{ selectedField, setSelectedField, selectedEntityId, setSelectedEntityId, activeTab, setActiveTab }} />;
};

const SyncMappingEditorState = ({ selection }: { selection: WorkbenchSelection }) => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const qc = useQueryClient();
  const {
    id,
    mode: modeParam,
    providerId: routeProviderId,
  } = useParams<{ id?: string; mode?: string; providerId?: string }>();
  const mode: EditorMode =
    modeParam === "readonly" ? "readonly" : modeParam === "create" || !id ? "create" : "edit";

  // View-but-not-edit: a user can open the editor read-only but only sees the
  // save/persist action when they hold UPDATE on inventory; navigation/preview stay.
  const { can } = usePermissions();
  const canUpdate = can("UPDATE", "INVENTORY");
  const isReadonly = mode === "readonly" || !canUpdate;

  const { selectedField, setSelectedField, selectedEntityId, setSelectedEntityId, activeTab, setActiveTab } = selection;
  const [initialDraft] = useState<MappingConfiguration | null>(() =>
    mode === "create"
      ? makeDraftConfiguration(
          "",
          t("sync.mapping.defaultConfigName", "New mapping configuration"),
        )
       : null,
  );
  const [draft, setDraft] = useState<MappingConfiguration | null>(initialDraft);
  const saveInFlight = useRef(false);
  const [authoringGeneration, setAuthoringGeneration] = useState(0);
  const authoringGenerationRef = useRef(0);
  // Save and discard end an authoring generation. Retained results from that
  // lifetime cannot recreate a draft after the save settles or history returns.
  const invalidateAuthoring = () => {
    const nextGeneration = authoringGenerationRef.current + 1;
    authoringGenerationRef.current = nextGeneration;
    setAuthoringGeneration(nextGeneration);
  };
  const changeDraft = useCallback((update: SetStateAction<MappingConfiguration | null>) => {
    if (saveInFlight.current || authoringGeneration !== authoringGenerationRef.current) return false;
    setDraft(update);
    return true;
  }, [authoringGeneration]);
  const [conflictDialogOpen, setConflictDialogOpen] = useState(false);
  const [conflictData, setConflictData] = useState<{
    newVersion: number;
    savedBy?: string;
    savedAt?: string;
    newId?: string;
  } | null>(null);
  const [addEntityOpen, setAddEntityOpen] = useState(false);

  // ── Fetch existing config ─────────────────────────────────

  const providersQuery = useQuery({
    queryKey: ["sync", "providers", "mapping-editor"],
    queryFn: () => listProviders({ limit: 100, offset: 0, order: "asc" }),
    enabled: mode === "create",
  });

  const configQuery = useQuery({
    queryKey: ["sync", "mapping-configuration", id],
    queryFn: () => getMappingConfiguration(id!),
    enabled: !!id && mode !== "create",
    select: (data) => data,
  });

  const defaultProviderId = routeProviderId ?? providersQuery.data?.items[0]?.id ?? "";

  // Target choices: the typed local inventory (the only supported runnable
  // target) plus any provider as an explicit remote target. Never implicit.
  const targetOptions = [
    { value: "local_inventory", label: t("sync.mapping.targetLocalInventory", "Local inventory") },
    ...(providersQuery.data?.items ?? []).map((provider) => ({
      value: `provider:${provider.id}`,
      label: `${t("sync.mapping.targetProvider", "Provider")}: ${provider.name}`,
    })),
  ];

  // Initialize draft from server data when loaded
  const draftWithDefaults =
    mode === "create" && draft
      ? { ...draft, provider_id: draft.provider_id || defaultProviderId }
      : draft;
  const config = isReadonly ? configQuery.data ?? null : draftWithDefaults ?? configQuery.data ?? null;

  // The source/target field suggestions for the selected entity come from the
  // canonical local-inventory catalogue, not the remote provider registry.
  const selectedSourceEntity = config?.entity_mappings.find(
    (em) => em.id === selectedEntityId,
  )?.source_entity;
  const catalogResourceQuery = useQuery({
    queryKey: ["sync", "local-inventory-resource", selectedSourceEntity],
    queryFn: () => getLocalInventoryResource(selectedSourceEntity!),
    enabled: !!selectedSourceEntity && config?.target?.kind === "local_inventory",
  });
  const catalogFieldSuggestions = (catalogResourceQuery.data?.fields ?? []).map((f) => f.name);

  // ── Save mutation ─────────────────────────────────────────

  const saveMutation = useApiMutation({
    mutationFn: async (payload: MappingConfiguration) => {
      if (mode === "create") {
        return createMappingConfiguration({
          logical_id: payload.logical_id,
          name: payload.name,
          provider_id: payload.provider_id,
          target: payload.target,
          direction: payload.direction,
          conflict_mode: payload.conflict_mode,
          entity_mappings: payload.entity_mappings,
        });
      }
      return updateMappingConfiguration(id!, {
        name: payload.name,
        direction: payload.direction,
        conflict_mode: payload.conflict_mode,
        entity_mappings: payload.entity_mappings,
      });
    },
    onSuccess: (saved) => {
      qc.setQueryData(["sync", "mapping-configuration", saved.id], saved);
      setDraft(null);
      void qc.invalidateQueries({ queryKey: ["sync", "mapping-configurations"] });
      notify("success",
        t("sync.mapping.savedAsVersion", "Saved as v{{version}}", {
          version: saved.version,
        }),
      );
      void navigate(`/inventory/sync/mappings/${saved.id}/edit`);
    },
    onError: (err: unknown) => {
      const apiErr = err as { status?: number; body?: unknown };
      if (apiErr.status === 409) {
        const body = apiErr.body as {
          latest_version?: number;
          latest_id?: string;
          saved_by?: string;
          saved_at?: string;
        };
        setConflictData({
          newVersion: body.latest_version ?? 0,
          newId: body.latest_id,
          savedBy: body.saved_by,
          savedAt: body.saved_at,
        });
        setConflictDialogOpen(true);
      } else {
        notify("error", t("sync.mapping.saveFailed", "Save failed"));
      }
    },
    onSettled: () => { saveInFlight.current = false; },
  });

  // ── Draft mutation helpers ────────────────────────────────

  const updateFieldMapping = useCallback(
    (entityId: string, fieldId: string, patch: Partial<FieldMapping>) => {
      changeDraft((prev) => {
        const base = prev ?? configQuery.data ?? null;
        if (!base) return prev;
        return {
          ...base,
          entity_mappings: base.entity_mappings.map((em) =>
            em.id === entityId
              ? {
                  ...em,
                  field_mappings: em.field_mappings.map((fm) =>
                    fm.id === fieldId ? { ...fm, ...patch } : fm,
                  ),
                }
              : em,
          ),
        };
      });
    },
    [changeDraft, configQuery.data],
  );

  const updateEntityMapping = useCallback(
    (entityId: string, patch: Partial<EntityMapping>) => {
      changeDraft((prev) => {
        const base = prev ?? configQuery.data ?? null;
        if (!base) return prev;
        return {
          ...base,
          entity_mappings: base.entity_mappings.map((em) =>
            em.id === entityId ? { ...em, ...patch } : em,
          ),
        };
      });
    },
    [changeDraft, configQuery.data],
  );

  const addEntityMapping = useCallback((em: EntityMapping) => {
    if (saveInFlight.current) return false;
    const base = draft ?? configQuery.data ?? makeDraftConfiguration(
      defaultProviderId,
      t("sync.mapping.defaultConfigName", "New mapping configuration"),
    );
    if (base.entity_mappings.some((existing) => existing.source_entity === em.source_entity)) {
      notify("warning",
        t("sync.mapping.entityAlreadyExists", "Entity {{entity}} is already mapped.", {
          entity: em.source_entity,
        }),
      );
      return false;
    }
    if (!changeDraft({
      ...base,
      entity_mappings: [...base.entity_mappings, em],
    })) return false;
    setSelectedField(null);
    setSelectedEntityId(em.id);
    return true;
  }, [changeDraft, configQuery.data, defaultProviderId, draft, t, setSelectedField, setSelectedEntityId]);

  const selectField = useCallback((field: SelectedField) => {
    setSelectedEntityId(null);
    setSelectedField(field);
  }, [setSelectedEntityId, setSelectedField]);

  const selectEntity = useCallback((entityId: string) => {
    setSelectedField(null);
    setSelectedEntityId(entityId);
  }, [setSelectedEntityId, setSelectedField]);

  const addFieldMapping = useCallback(
    (entityId: string, field: FieldMapping) => {
      if (saveInFlight.current) return false;
      const base = draft ?? configQuery.data ?? null;
      if (!base) return false;

      const entity = base.entity_mappings.find((em) => em.id === entityId);
      if (!entity) return false;

      const duplicate = entity.field_mappings.some(
        (fm) =>
          fm.source_field.trim().toLowerCase() === field.source_field.trim().toLowerCase() ||
          fm.target_field.trim().toLowerCase() === field.target_field.trim().toLowerCase(),
      );
      if (duplicate) {
        notify("warning",
          t("sync.mapping.fieldAlreadyExists", "This source or target field is already mapped."),
        );
        return false;
      }

      if (!changeDraft({
        ...base,
        entity_mappings: base.entity_mappings.map((em) =>
          em.id === entityId
            ? { ...em, field_mappings: [...em.field_mappings, field] }
            : em,
        ),
      })) return false;
      setSelectedEntityId(null);
      setSelectedField({ entityId, fieldId: field.id });
      setActiveTab("expression");
      notify("success",
        t("sync.mapping.fieldAdded", "Field {{field}} added.", {
          field: field.source_field,
        }),
      );
      return true;
    },
    [changeDraft, configQuery.data, draft, t, setSelectedEntityId, setSelectedField, setActiveTab],
  );

  // ── Loading ───────────────────────────────────────────────

  if (mode !== "create" && configQuery.isLoading) {
    return (
      <EntityEditorPage scope="routed" width="standard"
        title={t("sync.mapping.editorTitle", "Mapping Editor")}
        onBack={() => navigate(-1)}
        backLabel={t("sync.mapping.backToList", "Back to list")}
        footer={null}
      >
        <div className="flex min-h-[32rem] flex-col gap-4 md:flex-row">
          <SurfaceCard className="w-full shrink-0 p-3 space-y-2 md:w-[280px]">
            {[0, 1, 2].map((i) => (
              <Skeleton key={i} className="h-9 w-full" />
            ))}
          </SurfaceCard>
          <SurfaceCard className="flex-1 p-4 space-y-3">
            <Skeleton className="h-9 w-48" />
            <Skeleton className="h-32 w-full" />
            <Skeleton className="h-32 w-full" />
          </SurfaceCard>
        </div>
      </EntityEditorPage>
    );
  }

  // ── Error ─────────────────────────────────────────────────

  const missingConfig = mode !== "create" && (
    (configQuery.error instanceof ApiError && configQuery.error.status === 404) ||
    (configQuery.isSuccess && !config)
  );
  if (mode !== "create" && (configQuery.isError || missingConfig)) {
    return (
      <EntityEditorPage scope="routed" width="standard"
        title={t("sync.mapping.editorTitle", "Mapping Editor")}
        onBack={() => navigate(-1)}
        backLabel={t("sync.mapping.backToList", "Back to list")}
        footer={null}
      >
        <SurfaceCard className="flex w-full flex-col items-center justify-center py-16 gap-4">
          <AlertTriangle className="size-9 text-status-negative" />
          <p role="alert" className="mx-auto max-w-xs text-sm text-muted-foreground text-balance">
            {missingConfig ? t("notFound") : t("sync.mapping.loadConfigError", "Failed to load configuration.")}
          </p>
          <div className="flex gap-2">
            <Button
              variant="outline"
              size="sm"
              className="h-9 text-sm"
              onClick={() => configQuery.refetch()}
            >
              {t("tryAgain", "Try again")}
            </Button>
          </div>
        </SurfaceCard>
      </EntityEditorPage>
    );
  }

  const editingLocked = isReadonly || saveMutation.isPending;
  const currentConfig = config ?? configQuery.data ?? null;
  const selectedEntityMapping = currentConfig?.entity_mappings.find(
    (em) => em.id === selectedField?.entityId,
  );
  const selectedEntityOnly = selectedEntityId
    ? currentConfig?.entity_mappings.find((em) => em.id === selectedEntityId) ?? null
    : null;
  const selectedFieldMapping = selectedEntityMapping?.field_mappings.find(
    (fm) => fm.id === selectedField?.fieldId,
  );
  const providers = providersQuery.data?.items ?? [];
  const entityCount = currentConfig?.entity_mappings.length ?? 0;
  const fieldCount =
    currentConfig?.entity_mappings.reduce(
      (sum, entity) => sum + entity.field_mappings.length,
      0,
    ) ?? 0;
  const saveDisabled =
    saveMutation.isPending ||
    !currentConfig ||
    !currentConfig.name.trim() ||
    !currentConfig.provider_id;

  const identityFields = [
    { label: t("sync.mapping.providerLabel", "Provider"), value: currentConfig?.provider_id ?? "" },
    { label: t("sync.mapping.targetLabel", "Target"), value: currentConfig?.target.kind === "provider" ? `${t("sync.mapping.targetProvider", "Provider")}: ${currentConfig.target.provider_id}` : t("sync.mapping.targetLocalInventory", "Local inventory") },
  ];
  const directionOptions = [
    { value: "pull", label: t("sync.mapping.directionPull", "Pull") },
    { value: "push", label: t("sync.mapping.directionPush", "Push") },
    { value: "bidirectional", label: t("sync.mapping.directionBidirectional", "Bidirectional") },
  ] satisfies Array<{ value: MappingConfiguration["direction"]; label: string }>;
  const conflictOptions = [
    { value: "netbox_wins", label: t("sync.mapping.conflictRemoteWins", "Remote wins") },
    { value: "piping_wins", label: t("sync.mapping.conflictLocalWins", "Local wins") },
    { value: "last_write_wins", label: t("sync.mapping.conflictNewest", "Newest") },
    { value: "field_level", label: t("sync.mapping.conflictManual", "Manual") },
  ] satisfies Array<{ value: MappingConfiguration["conflict_mode"]; label: string }>;

  const updateDraft = (patch: Partial<MappingConfiguration>) => {
    changeDraft((prev) => {
      const base = prev ?? configQuery.data;
      if (!base) return prev;
      return { ...base, ...patch };
    });
  };

  const saveConfiguration = () => {
    if (saveInFlight.current || saveDisabled || isReadonly || !canUpdate || !currentConfig) return;
    saveInFlight.current = true;
    invalidateAuthoring();
    setAddEntityOpen(false);
    saveMutation.mutate(currentConfig);
  };

  const body = (
    <div className="flex min-w-0 flex-col gap-6">
      {/* Read-only banner */}
      {isReadonly && currentConfig?.is_latest === false && (
        <SurfaceCard className="px-4 py-2 flex items-center gap-2 border-status-warning/30 bg-status-warning/5">
          <AlertTriangle className="h-3.5 w-3.5 text-status-warning shrink-0" />
          <span className="text-xs text-status-warning">
            {t(
              "sync.mapping.viewingArchived",
              "Viewing v{{version}} — archived. Editing creates a new version.",
              { version: currentConfig?.version },
            )}
          </span>
        </SurfaceCard>
      )}

      {/* In-flight jobs banner */}
      {currentConfig && currentConfig.in_flight_job_count > 0 && (
        <SurfaceCard className="px-4 py-2 flex items-center gap-2 border-status-info/20 bg-status-info/5">
          <Info className="h-3.5 w-3.5 text-status-info shrink-0" />
          <span className="text-xs text-muted-foreground">
            {t(
              "sync.mapping.inFlightJobs",
              "{{count}} sync jobs are running on v{{version}}. Saving creates a new version; in-flight jobs continue.",
              {
                count: currentConfig.in_flight_job_count,
                version: currentConfig.version,
              },
            )}
          </span>
        </SurfaceCard>
      )}

      {!isReadonly && <FormSection title={t("sync.mapping.stepConfig", "Configuration")}>
        {mode !== "create" && <div className="pb-4"><DefinitionGrid fields={identityFields} /></div>}
        <fieldset disabled={editingLocked} className="grid min-w-0 gap-3 lg:grid-cols-[minmax(14rem,2fr)_minmax(12rem,1.3fr)_minmax(10rem,1fr)_minmax(10rem,1fr)]">
          <FormField
            htmlFor="sync-mapping-config-name"
            label={t("sync.mapping.configNameLabel", "Configuration name")}
            required

          >
            <Input
              id="sync-mapping-config-name"
              className="h-9 border-input/30 text-sm"
              value={currentConfig?.name ?? ""}
              disabled={editingLocked}
              placeholder={t("sync.mapping.configNamePlaceholder", "NetBox to inventory")}
              onChange={(event) => updateDraft({ name: event.target.value })}
            />
          </FormField>
          {mode === "create" && <FormField
            htmlFor="sync-mapping-provider"
            label={t("sync.mapping.providerLabel", "Provider")}
            required

          >
            <SearchableSelect
              id="sync-mapping-provider"
              items={providers.map((provider) => ({
                id: provider.id,
                label: provider.name,
                searchText: provider.providerId,
              }))}
              value={currentConfig?.provider_id ?? ""}
              disabled={editingLocked || mode !== "create" || providersQuery.isLoading}
              onValueChange={(value) => updateDraft({ provider_id: value })}
              placeholder={t("sync.mapping.providerPlaceholder", "Choose provider")}
              searchPlaceholder={t("search")}
              isLoading={providersQuery.isLoading}
              triggerClassName="border-input/30"
              panelPlacement="overlay"
            />
          </FormField>}
          <FormSelect<MappingConfiguration["direction"]>
            id="sync-mapping-direction"
            label={t("sync.mapping.directionLabel", "Direction")}

              value={currentConfig?.direction ?? "pull"}
              disabled={editingLocked}
              onValueChange={(value) =>
                updateDraft({ direction: value })
              }
            triggerClassName="border-input/30"
            options={directionOptions}
          />
          <FormSelect<MappingConfiguration["conflict_mode"]>
            id="sync-mapping-conflict-mode"
            label={t("sync.mapping.conflictModeLabel", "Conflict mode")}

              value={currentConfig?.conflict_mode ?? "netbox_wins"}
              disabled={editingLocked}
              onValueChange={(value) =>
                updateDraft({ conflict_mode: value })
              }
            triggerClassName="border-input/30"
            options={conflictOptions}
          />
          {mode === "create" && <FormSelect
            id="sync-mapping-target"
            label={t("sync.mapping.targetLabel", "Target")}

            value={targetToSelectValue(currentConfig?.target ?? { kind: "local_inventory" })}
            disabled={editingLocked || mode !== "create"}
            onValueChange={(value) => updateDraft({ target: selectValueToTarget(value) })}
            triggerClassName="border-input/30"
            options={targetOptions}
          />}
        </fieldset>
        {!currentConfig?.provider_id && (
          <div className="mt-2 flex items-center gap-2 rounded-md border border-status-warning/30 bg-status-warning/5 px-2 py-1.5 text-xs text-status-warning">
            <AlertTriangle className="h-3.5 w-3.5" />
            {t("sync.mapping.providerRequiredHint", "Choose a provider before saving this configuration.")}
          </div>
        )}
      </FormSection>}

      <FormSection title={t("sync.mapping.stepFields", "Fields")} description={`${t("sync.mapping.entityCount", "{{count}} entities", { count: entityCount })} · ${t("sync.mapping.fieldCount", "{{count}} fields", { count: fieldCount })}`} divider>

      {/* Main layout: left rail + right pane */}
      <div className="flex min-h-[32rem] flex-col gap-4 md:flex-row">
        {/* The field tree is a bounded workbench surface, not the page scroll owner. */}
        <aside className="flex w-full shrink-0 flex-col md:max-h-[40rem] md:w-[280px] md:border-r md:border-border">
          <FieldTreeRail
            entityMappings={currentConfig?.entity_mappings ?? []}
            selectedField={selectedField}
            selectedEntityId={selectedEntityId}
            onSelectField={selectField}
            onSelectEntity={selectEntity}
            onAddEntity={() => setAddEntityOpen(true)}
            onAddField={selectEntity}
            readonly={editingLocked}
          />
        </aside>

        <section className="flex min-h-[28rem] min-w-0 flex-1 flex-col">
          {selectedField && selectedEntityMapping && selectedFieldMapping ? (
            <Tabs
              value={activeTab}
              onValueChange={(v) =>
                setActiveTab(v as "expression" | "preview" | "settings")
              }
              className="flex min-w-0 flex-col"
            >
              {/* Sticky tab strip */}
              <div className="px-4 pt-3 pb-0 shrink-0 sticky top-0 z-10 bg-card border-b border-border">
                <SectionTabs
                  items={[
                    { value: "expression", label: t("sync.mapping.tabExpression", "Expression") },
                    { value: "preview", label: t("sync.mapping.tabPreview", "Preview") },
                    { value: "settings", label: t("sync.mapping.tabSettings", "Settings") },
                  ] satisfies Array<{ value: typeof activeTab; label: string }>}
                  className="surface-chip h-9 w-auto justify-start gap-0"
                  triggerClassName="h-9 px-3"
                />
              </div>

              <div className="min-w-0">
                <TabsContent value="expression" className="p-4 mt-0">
                  <fieldset disabled={editingLocked} className="min-w-0">
                  <ExpressionBuilder
                    entityMapping={selectedEntityMapping}
                    fieldMapping={selectedFieldMapping}
                    readonly={editingLocked}
                    onUpdate={(patch) =>
                      updateFieldMapping(
                        selectedField.entityId,
                        selectedField.fieldId,
                        patch,
                      )
                    }
                  />
                  </fieldset>
                </TabsContent>

                <TabsContent value="preview" className="p-4 mt-0">
                  <PreviewDiff
                    entityMapping={selectedEntityMapping}
                    fieldMapping={selectedFieldMapping}
                    providerId={currentConfig?.provider_id ?? ""}
                  />
                </TabsContent>

                <TabsContent value="settings" className="p-4 mt-0">
                  <SettingsTab
                    entityMapping={selectedEntityMapping}
                    readonly={editingLocked}
                    onUpdateEntity={(patch) =>
                      updateEntityMapping(selectedField.entityId, patch)
                    }
                  />
                </TabsContent>
              </div>
            </Tabs>
          ) : (
            <div className="flex-1 flex w-full items-center justify-center">
              {selectedEntityOnly ? (
                <EntityFieldStart
                  entityMapping={selectedEntityOnly}
                  fieldCount={selectedEntityOnly.field_mappings.length}
                  readonly={editingLocked}
                  fieldSuggestions={catalogFieldSuggestions}
                  onAddAnotherEntity={() => setAddEntityOpen(true)}
                  onAddField={(sourceField, targetField) =>
                    addFieldMapping(
                      selectedEntityOnly.id,
                      makeFieldMapping(sourceField, targetField),
                    )
                  }
                />
              ) : (
                <div className="mx-auto max-w-md space-y-3 text-center">
                  <div className="mx-auto flex h-10 w-10 items-center justify-center rounded-lg bg-primary/10">
                    <Layers className="h-5 w-5 text-primary" />
                  </div>
                  <div>
                    <h3 className="text-sm font-semibold text-foreground">
                      {t("sync.mapping.startTitle", "Start with an entity")}
                    </h3>
                    <p className="mt-1 text-xs text-muted-foreground text-balance">
                      {t(
                        "sync.mapping.selectFieldHint",
                        "Select a field from the left, or add an entity to begin.",
                      )}
                    </p>
                  </div>
                  {!editingLocked && (
                    <Button
                      size="sm"
                      className="h-9 gap-1.5 text-sm"
                      onClick={() => setAddEntityOpen(true)}
                    >
                      <Plus className="h-3.5 w-3.5" />
                      {t("sync.mapping.addEntity", "Add entity")}
                    </Button>
                  )}
                </div>
              )}
            </div>
          )}
        </section>
      </div>

      </FormSection>
      {/* Add entity dialog */}
      <AddEntityDialog
        open={addEntityOpen && !editingLocked}
        onOpenChange={setAddEntityOpen}
        providerId={currentConfig?.provider_id ?? ""}
        target={currentConfig?.target ?? { kind: "local_inventory" }}
        onAdded={addEntityMapping}
      />

      {/* Version conflict dialog */}
      {conflictData && (
        <ConflictDialog
          open={conflictDialogOpen}
          onOpenChange={setConflictDialogOpen}
          newVersion={conflictData.newVersion}
          savedBy={conflictData.savedBy}
          savedAt={conflictData.savedAt}
          newId={conflictData.newId}
          onContinueEditing={() => {
            setConflictDialogOpen(false);
          }}
        />
      )}
    </div>
  );

  if (isReadonly && currentConfig) {
    return (
      <PageLayout width="standard">
      <EntityDetail
        icon={Layers}
        title={`${currentConfig.name} v${currentConfig.version}`}
        backHref={`${currentConfig.provider_id ? `/inventory/sync/${encodeURIComponent(currentConfig.provider_id)}/mappings` : "/inventory/sync/mappings"}${location.search}${location.hash}`}
        backLabel={t("sync.mapping.backToList", "Back to list")}
        fields={[
          ...identityFields,
          { label: t("sync.mapping.directionLabel", "Direction"), value: directionOptions.find(option => option.value === currentConfig.direction)?.label },
          { label: t("sync.mapping.conflictModeLabel", "Conflict mode"), value: conflictOptions.find(option => option.value === currentConfig.conflict_mode)?.label },
        ]}
        actions={[{
          key: "edit",
          label: !currentConfig.is_latest ? t("sync.mapping.editLatest", "Edit latest") : t("edit"),
          href: `/inventory/sync/mappings/${currentConfig.id}/edit${location.search}${location.hash}`,
          allowed: canUpdate,
        }]}
      >
        <div className="pt-6">{body}</div>
      </EntityDetail>
      </PageLayout>
    );
  }

  return (
    <EntityEditorPage scope="routed"
      width="standard"
      title={mode === "create" ? t("sync.mapping.newConfig", "New Configuration") : currentConfig ? `${currentConfig.name} v${currentConfig.version}` : t("sync.mapping.newConfig", "New Configuration")}
      onBack={() => {
        invalidateAuthoring();
        setAddEntityOpen(false);
        setDraft(initialDraft);
        void navigate(-1);
      }}
      backLabel={t("sync.mapping.backToList", "Back to list")}
      onSave={!isReadonly && canUpdate ? saveConfiguration : undefined}
      saveDisabled={saveDisabled}
      saving={saveMutation.isPending}
      saveLabel={mode === "create" ? t("save", "Save") : t("sync.mapping.saveAsVersion", "Save as v{{version}}", { version: (currentConfig?.version ?? 0) + 1 })}
      cancelLabel={t("cancel")}
      dirty={!isReadonly && !!draft && JSON.stringify(draft) !== JSON.stringify(mode === "create" ? initialDraft : configQuery.data)}
      dirtyMessage={t("editor.unsavedChanges")}
      bodyClassName="flex flex-col gap-6"
    >
      {body}
    </EntityEditorPage>
  );
};


// ── Empty entity / quick field creation ──────────────────────────────────────

interface EntityFieldStartProps {
  entityMapping: EntityMapping;
  fieldCount: number;
  readonly: boolean;
  /** Canonical field names for this entity from the local-inventory catalogue. */
  fieldSuggestions: string[];
  onAddAnotherEntity: () => void;
  onAddField: (sourceField: string, targetField: string) => boolean;
}

const EntityFieldStart = ({
  entityMapping,
  fieldCount,
  readonly,
  fieldSuggestions,
  onAddAnotherEntity,
  onAddField,
}: EntityFieldStartProps) => {
  const { t } = useTranslation();
  const [sourceField, setSourceField] = useState("");
  const [targetField, setTargetField] = useState("");

  const normalizedSource = sourceField.trim();
  const normalizedTarget = targetField.trim() || deriveTargetField(normalizedSource);
  const canAdd = normalizedSource.length > 0 && !readonly;

  const submit = () => {
    if (!canAdd) return;
    if (onAddField(normalizedSource, normalizedTarget)) {
      setSourceField("");
      setTargetField("");
    }
  };

  return (
    <div className="mx-auto w-full max-w-2xl space-y-4">
      <div className="flex items-start gap-3">
        <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-primary/10">
          <Layers className="h-5 w-5 text-primary" />
        </div>
        <div className="min-w-0 flex-1">
          <h3 className="text-sm font-semibold text-foreground">
            {t("sync.mapping.addFieldsForEntity", "Add fields for {{entity}}", {
              entity: entityMapping.source_entity,
            })}
          </h3>
          <p className="mt-1 text-xs text-muted-foreground text-balance">
            {fieldCount === 0
              ? t(
                  "sync.mapping.addFirstFieldHint",
                  "Add the first source field and choose where it should be written in Piping.",
                )
              : t(
                  "sync.mapping.addMoreFieldsHint",
                  "Add another field, or select an existing field on the left to tune its transform.",
                )}
          </p>
        </div>
      </div>

      <div className="rounded-lg border border-border bg-background p-3">
        <div className="grid gap-3 md:grid-cols-[1fr_1fr_auto] md:items-end">
          <FormField
            htmlFor="sync-mapping-source-field"
            label={t("sync.mapping.sourceFieldLabel", "Source field")}
            required

          >
            <Input
              id="sync-mapping-source-field"
              className="h-9 border-input/30 font-mono text-sm"
              value={sourceField}
              disabled={readonly}
              list="sync-mapping-field-suggestions"
              placeholder={t("sync.mapping.sourceFieldPlaceholder", "name or site.name")}
              onChange={(event) => {
                const next = event.target.value;
                const previousDerived = deriveTargetField(sourceField.trim());
                setSourceField(next);
                if (!targetField.trim() || targetField.trim() === previousDerived) {
                  setTargetField(deriveTargetField(next.trim()));
                }
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  submit();
                }
              }}
            />
          </FormField>
          <FormField
            htmlFor="sync-mapping-target-field"
            label={t("sync.mapping.targetFieldPathLabel", "Target field")}
            required

          >
            <Input
              id="sync-mapping-target-field"
              className="h-9 border-input/30 font-mono text-sm"
              value={targetField}
              disabled={readonly}
              list="sync-mapping-field-suggestions"
              placeholder={t("sync.mapping.targetFieldPathPlaceholder", "name")}
              onChange={(event) => setTargetField(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  submit();
                }
              }}
            />
          </FormField>
          <Button
            size="sm"
            className="h-9 gap-1.5 text-sm"
            disabled={!canAdd}
            onClick={submit}
          >
            <Plus className="h-3.5 w-3.5" />
            {t("sync.mapping.addField", "Add field")}
          </Button>
        </div>
        <p className="mt-2 text-xs text-muted-foreground">
          {t(
            "sync.mapping.fieldQuickHint",
            "Tip: use dotted paths for nested data, for example site.name or primary_ip.address.",
          )}
        </p>
        <datalist id="sync-mapping-field-suggestions">
          {fieldSuggestions.map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
      </div>

      {!readonly && (
        <Button
          size="sm"
          variant="outline"
          className="h-9 gap-1.5 text-sm"
          onClick={onAddAnotherEntity}
        >
          <Plus className="h-3.5 w-3.5" />
          {t("sync.mapping.addAnotherEntity", "Add another entity")}
        </Button>
      )}
    </div>
  );
};

// ── Settings tab ──────────────────────────────────────────────────────────────

interface SettingsTabProps {
  entityMapping: EntityMapping;
  readonly: boolean;
  onUpdateEntity: (patch: Partial<EntityMapping>) => void;
}

const SettingsTab = ({ entityMapping, readonly, onUpdateEntity }: SettingsTabProps) => {
  const { t } = useTranslation();

  return (
    <div className="space-y-3">
      <div className="text-xs text-muted-foreground">
        {t("sync.mapping.entitySettingsHint", "Entity-level settings for")}:{" "}
        <span className="font-mono font-medium text-foreground">
          {entityMapping.source_entity}
        </span>
      </div>

      <SwitchRow
        label={t("sync.mapping.entityEnabled", "Entity sync enabled")}
        description={t(
          "sync.mapping.entityEnabledHint",
          "Toggle to include or exclude this entity from sync runs.",
        )}
        checked={entityMapping.enabled}
        disabled={readonly}
        onCheckedChange={(checked) => onUpdateEntity({ enabled: checked })}
      />

    </div>
  );
};

interface SwitchRowProps {
  label: string;
  description: string;
  checked: boolean;
  disabled: boolean;
  onCheckedChange: (checked: boolean) => void;
}

const SwitchRow = ({
  label,
  description,
  checked,
  disabled,
  onCheckedChange,
}: SwitchRowProps) => (
  <div className={cn("flex items-center gap-4 border-b border-border py-4")}>
    <div className="flex-1">
      <div className="text-sm font-medium text-foreground">{label}</div>
      <div className="text-xs text-muted-foreground">{description}</div>
    </div>
    <Switch
      checked={checked}
      disabled={disabled}
      onCheckedChange={onCheckedChange}
      aria-label={label}
    />
  </div>
);

export default SyncMappingEditor;
