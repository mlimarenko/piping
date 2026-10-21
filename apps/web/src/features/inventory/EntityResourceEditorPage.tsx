import { useCallback, useMemo, useRef, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import PageLayout from "@/shared/ui-shared/PageLayout";
import InventoryResourceDetailPage from "./InventoryResourceDetailPage";
import { Button } from "@/shared/ui-primitives/button";
import FormSection from "@/shared/ui-shared/FormSection";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import {
  DEPTH_GUARD_THRESHOLD,
  useEntityCreateStack,
  type EntityCreateFrame,
} from "@/shared/forms/entityCreateStack";
import {
  buildEntityDraft,
  draftToResource,
  draftToResourceUpdate,
  validateEntityDraft,
  type EntityFormValues,
} from "@/shared/forms/entityForm";
import type { EntityPickerOption } from "@/shared/forms/EntityPicker";
import { relatedEntitiesForEntity } from "@/shared/forms/entityRelatedEntities";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { CONNECTION_TARGETS_QUERY_KEY } from "@/features/connections/api";
import { notify } from "@/shared/lib/notifications";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import {
  createResource,
  customFieldEntityKind,
  getResource,
  listAllResources,
  updateResource,
} from "@/features/inventory/api";
import { inventoryResourceOption } from "@/features/inventory/resourceOptions";
import {
  isInventoryEntityKey,
  INVENTORY_RESOURCES_PATH,
} from "@/features/inventory/resourceLinks";
import EntityCustomFieldsSection, {
  type EntityCustomFieldsHandle,
} from "@/features/inventory/components/custom-fields/EntityCustomFieldsSection";
import { CustomFieldValidationError } from "@/features/inventory/components/custom-fields/customFieldErrors";
import EntityLocalContextSection from "@/features/inventory/components/config-contexts/EntityLocalContextSection";
import {
  LocalContextValidationError,
  type EntityLocalContextHandle,
} from "@/features/inventory/components/config-contexts/localContextErrors";
import RenderedConfigContextPanel from "@/features/inventory/components/config-contexts/RenderedConfigContextPanel";
import type { ConfigContextEntityKind } from "@/features/inventory/config-contexts-api";
import NestedEditorPanel, {
  type NestedEditorPanelHandle,
} from "@/features/inventory/components/inventory/NestedEditorPanel";
import {
  inventoryEntityIsReadOnly,
  relationshipsByTab,
  tabConfig,
} from "@/features/inventory/components/inventory/inventoryTypes";

/** The rendered-config-context endpoint kind for a config-context-bearing
 *  inventory entity (Devices / Virtual Machines), or `null` for the rest. */
function configContextEntityKind(entity: InventoryEntityKey): ConfigContextEntityKind | null {
  if (entity === "Devices") return "device";
  if (entity === "Virtual Machines") return "virtual-machine";
  return null;
}

const PRIMARY_IP_DISABLED_KEYS = new Set(["primaryIp"]);

const singularOf = (entity: InventoryEntityKey): string =>
  tabConfig.find((cfg) => cfg.key === entity)?.singular || entity;

/** Primary-IP selection belongs to nested relationships in the writable form. */
function disabledDraftKeys(
  hasNestedSpecs: boolean,
): Set<string> | undefined {
  if (hasNestedSpecs) return PRIMARY_IP_DISABLED_KEYS;
  return undefined;
}

/** Any nested create frame below the root differs from its pristine draft. */
function hasDirtyNestedFrame(stack: EntityCreateFrame[]): boolean {
  return stack.length > 1 && stack.slice(1).some(frameIsDirty);
}

/** The edit form is dirty when the root draft changed, an auxiliary input was
 *  touched, or a nested create frame holds unsaved input. */
function editDraftIsDirty(
  draft: EntityFormValues,
  initialDraft: EntityFormValues,
  auxTouched: boolean,
  nestedDirty: boolean,
): boolean {
  return JSON.stringify(draft) !== JSON.stringify(initialDraft) || auxTouched || nestedDirty;
}

/** A create frame is dirty when its draft differs from that entity's pristine
 *  draft. Used for both the routed create stack and the edit editor's inline
 *  frames, so an untouched nested frame never registers as unsaved. */
const frameIsDirty = (frame: { entity: InventoryEntityKey; draft: EntityFormValues }): boolean =>
  JSON.stringify(frame.draft) !== JSON.stringify(buildEntityDraft(frame.entity));

interface OptionLoaders {
  loadOptions: (entity: InventoryEntityKey) => EntityPickerOption[];
  isOptionsLoading: (entity: InventoryEntityKey) => boolean;
}

/** Per-related-entity FK option lists for the pickers (shared option cache
 *  prefix so inline-create invalidations refresh them). */
function useEntityOptions(entities: InventoryEntityKey[]): OptionLoaders {
  const optionQueries = useQueries({
    queries: entities.map((optionEntity) => ({
      queryKey: ["inventory", "resources", optionEntity, "options"],
      queryFn: () => listAllResources(optionEntity),
      staleTime: 30_000,
    })),
  });

  return useMemo(() => {
    const optionsByEntity = new Map<InventoryEntityKey, EntityPickerOption[]>();
    const loadingByEntity = new Map<InventoryEntityKey, boolean>();
    entities.forEach((optionEntity, idx) => {
      const rows = optionQueries[idx]?.data ?? [];
      optionsByEntity.set(optionEntity, rows.map((r) => inventoryResourceOption(optionEntity, r)));
      loadingByEntity.set(optionEntity, optionQueries[idx]?.isPending ?? false);
    });
    return {
      loadOptions: (optionEntity: InventoryEntityKey) => optionsByEntity.get(optionEntity) ?? [],
      isOptionsLoading: (optionEntity: InventoryEntityKey) => loadingByEntity.get(optionEntity) ?? false,
    };
  }, [entities, optionQueries]);
}

/** Create a child frame inline, seed the parent draft and pop. Returns the
 *  persisted resource id or null on failure. */
async function persistNewChild(
  entity: InventoryEntityKey,
  draft: EntityFormValues,
  singular: string,
  t: (key: string, opts?: Record<string, unknown>) => string,
  qc: ReturnType<typeof useQueryClient>,
): Promise<string | null> {
  const resource = draftToResource(entity, draft, singular);
  const { id: _omit, ...payload } = resource;
  try {
    const created = await createResource(entity, payload);
    await qc.invalidateQueries({ queryKey: ["inventory", "resources", entity] });
    return created.id;
  } catch (err) {
    notify("error", err instanceof Error ? err.message : t("unknownError"));
    return null;
  }
}

const INLINE_CREATE_DEPTH_WARNING = "inventory.resources.form.depthWarning";

/** The canonical full-page create editor. Mounted fresh per entity (keyed by the
 *  route wrapper), so its draft and create stack need no reset effect. */
const EntityCreateEditor = ({
  entity,
  canCreate,
  backLabel,
  onBack,
}: {
  entity: InventoryEntityKey;
  canCreate: boolean;
  backLabel: string;
  onBack: () => void;
}) => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const singular = t(`inventory.resources.entities.${entity}.one`, { defaultValue: singularOf(entity) });
  const { stack, top, depth, push, pop, setField, setErrors, resolve } =
    useEntityCreateStack(entity);
  const [submitting, setSubmitting] = useState(false);

  const optionEntities = useMemo(() => {
    const set = new Set<InventoryEntityKey>();
    for (const frame of stack) {
      for (const related of relatedEntitiesForEntity(frame.entity)) set.add(related);
    }
    return Array.from(set);
  }, [stack]);
  const { loadOptions, isOptionsLoading } = useEntityOptions(optionEntities);

  const topSingular = t(`inventory.resources.entities.${top.entity}.one`, {
    defaultValue: singularOf(top.entity),
  });
  // Dirty when any frame differs from its pristine draft: the root's unsaved
  // fields stay relevant for navigation/beforeunload even while a blank nested
  // frame is active, and an untouched nested frame is not dirty on its own.
  const dirty = useMemo(() => stack.some(frameIsDirty), [stack]);

  const requestBack = () => {
    if (depth > 1) pop();
    else onBack();
  };

  const submit = async () => {
    if (!canCreate) return;
    const errs = validateEntityDraft(top.entity, top.draft, t);
    if (Object.keys(errs).length > 0) {
      setErrors(errs);
      return;
    }
    if (depth > 1) {
      setSubmitting(true);
      const createdId = await persistNewChild(top.entity, top.draft, topSingular, t, qc);
      setSubmitting(false);
      if (createdId) resolve(createdId);
      return;
    }
    setSubmitting(true);
    const resource = draftToResource(entity, top.draft, singular);
    const { id: _omit, ...payload } = resource;
    try {
      const created = await createResource(entity, payload);
      await qc.invalidateQueries({ queryKey: ["inventory", "resources", entity] });
      if (entity === "Devices" || entity === "Virtual Machines") {
        await qc.invalidateQueries({ queryKey: CONNECTION_TARGETS_QUERY_KEY });
      }
      notify("success", t("inventory.resources.created", { name: created.name }));
      onBack();
    } catch (err) {
      notify("error", err instanceof Error ? err.message : t("unknownError"));
    } finally {
      setSubmitting(false);
    }
  };

  return (
      <EntityEditorPage scope="routed"
        title={t("inventory.resources.form.createTitle", { entity: topSingular })}
        description={t(`inventory.resources.hints.entities.${top.entity}`)}
        breadcrumbs={[{ key: singular, label: singular, onClick: onBack }]}
        backLabel={depth > 1 ? t("back") : backLabel}
        onBack={requestBack}
        dirty={dirty}
        saveLabel={t("create")}
        onSave={canCreate ? () => void submit() : undefined}
        saving={submitting}
        breadcrumbLabel={t("inventory.resources.title")}
        dirtyMessage={t("entityEditor.unsavedChanges")}
        onCancel={requestBack}
        cancelLabel={t("cancel")}
      >
        {depth >= DEPTH_GUARD_THRESHOLD && (
          <p className="mb-3 rounded-md border border-status-warning/40 bg-status-warning/10 px-2 py-1.5 text-xs text-status-warning">
            {t(INLINE_CREATE_DEPTH_WARNING)}
          </p>
        )}
        <EntityFormBody
          entity={top.entity}
          draft={top.draft}
          errors={top.errors}
          onField={setField}
          loadOptions={loadOptions}
          isOptionsLoading={isOptionsLoading}
          onRequestCreate={push}
        />
      </EntityEditorPage>
  );
};

/** The canonical full-page edit editor. Mounted only once the resource has
 *  loaded and keyed by its id, so the draft seeds from an initializer. */
const EntityEditEditor = ({
  entity,
  editing,
  canUpdate,
  backLabel,
  onBack,
}: {
  entity: InventoryEntityKey;
  editing: Resource;
  canUpdate: boolean;
  backLabel: string;
  onBack: () => void;
}) => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const singular = t(`inventory.resources.entities.${entity}.one`, { defaultValue: singularOf(entity) });
  const [initialDraft] = useState<EntityFormValues>(() => buildEntityDraft(entity, editing));
  const [draft, setDraft] = useState<EntityFormValues>(initialDraft);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [submitting, setSubmitting] = useState(false);
  // Custom fields / local context / nested editor own their inputs; a captured
  // change marks the whole editor dirty so the leave guard covers them too.
  const [auxTouched, setAuxTouched] = useState(false);
  const customFieldsHandle = useRef<EntityCustomFieldsHandle | null>(null);
  const localContextHandle = useRef<EntityLocalContextHandle | null>(null);
  const nestedRef = useRef<NestedEditorPanelHandle | null>(null);
  const [primaryIp4Id, setPrimaryIp4Id] = useState(editing.primaryIp4Id ?? "");
  const [primaryIp6Id, setPrimaryIp6Id] = useState(editing.primaryIp6Id ?? "");
  const { stack, top, depth, push, pop, setField, setErrors: setChildErrors, resolve } =
    useEntityCreateStack(entity);
  const creatingInline = depth > 1;

  const nestedSpecs = useMemo(() => relationshipsByTab[entity] ?? [], [entity]);
  const cfEntityKind = customFieldEntityKind(entity);
  const ccEntityKind = configContextEntityKind(entity);

  const optionEntities = useMemo(() => {
    const set = new Set<InventoryEntityKey>(relatedEntitiesForEntity(entity));
    for (const frame of stack) {
      for (const related of relatedEntitiesForEntity(frame.entity)) set.add(related);
    }
    return Array.from(set);
  }, [entity, stack]);
  const { loadOptions, isOptionsLoading } = useEntityOptions(optionEntities);

  const topSingular = t(`inventory.resources.entities.${top.entity}.one`, {
    defaultValue: singularOf(top.entity),
  });
  // Root and nested drafts both count: a dirty root must not be masked while a
  // blank inline-create frame is active, and an untouched nested frame is not
  // dirty. The edit form's root draft is the seeded `draft` state, not the
  // create stack's root frame.
  const nestedDirty = hasDirtyNestedFrame(stack);
  const dirty = editDraftIsDirty(draft, initialDraft, auxTouched, nestedDirty);

  const requestBack = () => {
    if (creatingInline) pop();
    else onBack();
  };

  const submitInline = useCallback(async () => {
    const errs = validateEntityDraft(top.entity, top.draft, t);
    if (Object.keys(errs).length > 0) {
      setChildErrors(errs);
      return;
    }
    setSubmitting(true);
    const createdId = await persistNewChild(top.entity, top.draft, topSingular, t, qc);
    setSubmitting(false);
    if (!createdId) return;
    // Seed the parent's FK field with the new id, then return to the edit form.
    const returnToFieldKey = top.returnToFieldKey;
    setDraft((prev) => ({ ...prev, [returnToFieldKey]: createdId }));
    resolve(createdId);
  }, [qc, resolve, setChildErrors, setDraft, setSubmitting, t, top.draft, top.entity, top.returnToFieldKey, topSingular]);

  const submitEdit = useCallback(async () => {
    if (!canUpdate) return;
    const errs = validateEntityDraft(entity, draft, t);
    setErrors(errs);
    if (Object.keys(errs).length > 0) return;
    const resource = draftToResourceUpdate(entity, draft, singular, editing, initialDraft);
    if (nestedSpecs.length > 0) {
      resource.primaryIp4Id = primaryIp4Id || undefined;
      resource.primaryIp6Id = primaryIp6Id || undefined;
    }
    try {
      if (cfEntityKind) resource.customFields = customFieldsHandle.current?.buildPayload();
      if (ccEntityKind) resource.localContextData = localContextHandle.current?.buildPayload();
    } catch (err) {
      if (err instanceof CustomFieldValidationError) {
        notify("error", t("customFields.entity.invalidJsonField", { field: err.fieldLabel }));
        return;
      }
      if (err instanceof LocalContextValidationError) {
        notify("error", t("configContexts.local.invalidJson"));
        return;
      }
      throw err;
    }
    setSubmitting(true);
    try {
      const { id: resourceId, ...payload } = resource;
      const updated = await updateResource(entity, resourceId, payload);
      await qc.invalidateQueries({ queryKey: ["inventory", "resources", entity] });
      qc.removeQueries({ queryKey: ["inventory", "resources", entity, "edit-detail", resourceId] });
      if (ccEntityKind) {
        await qc.invalidateQueries({ queryKey: ["inventory", "config-context", ccEntityKind, resourceId] });
      }
      await nestedRef.current?.save();
      if (entity === "Devices" || entity === "Virtual Machines") {
        await qc.invalidateQueries({ queryKey: CONNECTION_TARGETS_QUERY_KEY });
      }
      notify("success", t("inventory.resources.updated", { name: updated.name }));
      onBack();
    } catch (err) {
      notify("error", err instanceof Error ? err.message : t("unknownError"));
    } finally {
      setSubmitting(false);
    }
  }, [canUpdate, ccEntityKind, cfEntityKind, draft, editing, entity, initialDraft, nestedSpecs, onBack, primaryIp4Id, primaryIp6Id, qc, setErrors, setSubmitting, singular, t]);

  // The save handler picks the inline-create or root-edit submit path; both
  // submit functions are memoised so this selection stays stable.
  const onSaveHandler = useMemo<(() => void) | undefined>(() => {
    if (creatingInline) return () => void submitInline();
    if (canUpdate) return () => void submitEdit();
    return undefined;
  }, [canUpdate, creatingInline, submitEdit, submitInline]);

  const disabledKeys = disabledDraftKeys(nestedSpecs.length > 0);

  const customFieldsMap =
    editing.customFields && typeof editing.customFields === "object"
      ? editing.customFields
      : undefined;

  return (
      <EntityEditorPage scope="routed"
        title={
          creatingInline
            ? t("inventory.resources.form.createTitle", { entity: topSingular })
            : editing.name || singular
        }
        description={t(`inventory.resources.hints.entities.${top.entity}`)}
        breadcrumbs={[{ key: singular, label: singular, onClick: onBack }]}
        backLabel={creatingInline ? t("back") : backLabel}
        onBack={requestBack}
        dirty={dirty}
        saveLabel={creatingInline ? t("create") : t("saveChanges")}
        onSave={onSaveHandler}
        saving={submitting}
        breadcrumbLabel={t("inventory.resources.title")}
        dirtyMessage={t("entityEditor.unsavedChanges")}
        onCancel={requestBack}
        cancelLabel={t("cancel")}
      >
        {creatingInline ? (
          <EntityFormBody
            entity={top.entity}
            draft={top.draft}
            errors={top.errors}
            onField={setField}
            loadOptions={loadOptions}
            isOptionsLoading={isOptionsLoading}
            onRequestCreate={push}
          />
        ) : (
          <div className="flex flex-col gap-6" onChangeCapture={() => setAuxTouched(true)}>
            <EntityFormBody
              entity={entity}
              draft={draft}
              errors={errors}
              onField={(key, value) => {
                setDraft((prev) => ({ ...prev, [key]: value }));
                if (errors[key]) setErrors((prev) => ({ ...prev, [key]: "" }));
              }}
              loadOptions={loadOptions}
              isOptionsLoading={isOptionsLoading}
              onRequestCreate={push}
              disabledKeys={disabledKeys}
              renderNested={
                nestedSpecs.length > 0
                  ? () => (
                      <div className="border-t border-border pt-4">
                        <NestedEditorPanel
                          key={editing.id}
                          ref={nestedRef}
                          entity={entity}
                          rootRealId={editing.id}
                          readOnly={!canUpdate}
                          hideSave
                          primarySelection={{
                            current4: primaryIp4Id,
                            current6: primaryIp6Id,
                            onSelect: (ipRealId, isV6) =>
                              isV6 ? setPrimaryIp6Id(ipRealId) : setPrimaryIp4Id(ipRealId),
                          }}
                        />
                      </div>
                    )
                  : undefined
              }
            />

            {cfEntityKind && (
              <FormSection divider title={t("customFields.entity.section")} description={t("customFields.entity.sectionHint")}>
                <EntityCustomFieldsSection
                  entityKind={cfEntityKind}
                  entityId={editing.id}
                  customFields={customFieldsMap}
                  onRegister={(handle) => { customFieldsHandle.current = handle; }}
                />
              </FormSection>
            )}

            {ccEntityKind && (
              <>
                <FormSection divider>
                  <EntityLocalContextSection
                    localContextData={editing.localContextData}
                    onRegister={(handle) => { localContextHandle.current = handle; }}
                  />
                </FormSection>
                <FormSection divider>
                  <RenderedConfigContextPanel kind={ccEntityKind} entityId={editing.id} />
                </FormSection>
              </>
            )}
          </div>
        )}
      </EntityEditorPage>
  );
};


/**
 * Route entry point. Validates the `:entity` param and keys the editor by the
 * entity/id pair so a param change remounts it with fresh draft/stack state
 * (no reset effects), then renders the create or edit editor once the resource
 * (edit mode) has loaded.
 */
const EntityResourceEditorPage = () => {
  const { entity: entityParam, id } = useParams<{ entity: string; id?: string }>();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const { t } = useTranslation();
  const { can } = usePermissions();

  const entity = isInventoryEntityKey(entityParam) ? entityParam : null;
  const returnTo = searchParams.get("return") || `${INVENTORY_RESOURCES_PATH}?entity=${entityParam ?? ""}`;
  const onBack = () => { void navigate(returnTo, { replace: true }); };

  const detailQuery = useQuery({
    queryKey: ["inventory", "resources", entity, "edit-detail", id],
    queryFn: () => getResource(entity as InventoryEntityKey, id as string),
    enabled: Boolean(id) && Boolean(entity),
    staleTime: 5_000,
  });

  const backLabel = t(returnTo === "/connections" ? "connections.catalogue.back" : "inventory.resources.form.backToList");
  const stateTitle = entity ? singularOf(entity) : t("inventory.resources.title");

  if (!entity) {
    return (
      <EntityEditorPage scope="routed" title={stateTitle} onBack={onBack} backLabel={backLabel} cancelLabel={t("cancel")}>
        <p role="alert">{t("inventory.resources.notFound")}</p>
      </EntityEditorPage>
    );
  }

  // Read-only entities have no backend write routes; the routed editor is still
  // reachable by direct URL, so withhold the write capability here too rather
  // than mounting a Save that would 403.
  const writableEntity = !inventoryEntityIsReadOnly(entity);

  if (!id) {
    return (
      <EntityCreateEditor
        key={`new:${entity}`}
        entity={entity}
        canCreate={can("CREATE", "INVENTORY") && writableEntity}
        backLabel={backLabel}
        onBack={onBack}
      />
    );
  }

  if (detailQuery.isPending || detailQuery.isError || !detailQuery.data) {
    return (
      <EntityEditorPage scope="routed" title={stateTitle}
        description={t(`inventory.resources.hints.entities.${entity}`)}
        breadcrumbs={[{ key: stateTitle, label: stateTitle, onClick: onBack }]}
        breadcrumbLabel={t("inventory.resources.title")}
        onBack={onBack} backLabel={backLabel} cancelLabel={t("cancel")}>
        {detailQuery.isPending ? (
          <output className="flex items-center gap-2"><Loader2 aria-hidden className="size-6 animate-spin text-muted-foreground" />{t("loading")}</output>
        ) : (
          <div className="flex flex-col items-start gap-4">
            <p role="alert">{t("inventory.resources.form.loadFailed")}</p>
            <Button variant="outline" onClick={() => void detailQuery.refetch()}>{t("retry")}</Button>
          </div>
        )}
      </EntityEditorPage>
    );
  }

  return (
    <EntityEditEditor
      key={`edit:${entity}:${detailQuery.data.id}`}
      entity={entity}
      editing={detailQuery.data}
      canUpdate={can("UPDATE", "INVENTORY") && writableEntity}
      backLabel={backLabel}
      onBack={onBack}
    />
  );
};

/** Existing /edit bookmarks without write authority use the same saved read
 * body as /:id, never disabled authoring/custom-context widgets. */
export default function EntityResourceRoute() {
  const { entity, id } = useParams();
  const { can } = usePermissions();
  if (id && (!can("UPDATE", "INVENTORY") || (isInventoryEntityKey(entity) && inventoryEntityIsReadOnly(entity)))) {
    return <PageLayout width="standard" inset fill><InventoryResourceDetailPage /></PageLayout>;
  }
  return <EntityResourceEditorPage />;
}
