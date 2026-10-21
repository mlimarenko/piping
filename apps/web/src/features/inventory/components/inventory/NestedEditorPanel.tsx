import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { Loader2, Save } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import NestedEditorSection, {
  type PrimarySelection,
} from "@/shared/nested-editor/NestedEditorSection";
import {
  useNestedEditorTree,
  type LocalId,
  type NestedEditorTree,
} from "@/shared/nested-editor/useNestedEditorTree";
import {
  descriptorsFromSpecs,
  type RelationSpec,
} from "@/shared/nested-editor/descriptorFromSpec";
import EntityCreateEditor from "@/shared/forms/EntityCreateEditor";
import type { EntityPickerOption } from "@/shared/forms/EntityPicker";
import {
  listAllResources,
  createResource,
  INVENTORY_MAX_PAGE_SIZE,
} from "@/features/inventory/api";
import { inventoryResourceOption } from "@/features/inventory/resourceOptions";
import { relatedEntitiesForEntity } from "@/shared/forms/entityRelatedEntities";
import { relationshipsByTab } from "@/features/inventory/components/inventory/inventoryTypes";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import { notify } from "@/shared/lib/notifications";

/**
 * Host-agnostic container for the unified nested-CRUD editor. It owns the
 * headless tree, the related-entity option loaders the config row forms need,
 * inline-create of related entities (the picker "+"), the Save action, and eager
 * loading of the root relation levels. Both hosts (the inventory
 * `ScrollableDialog` and the connections `sheet` drawer) mount this with the same
 * `relationshipsByTab` specs, so behavior is identical.
 *
 * The inventory dialog mounts it with `hideSave` and drives `save()` through the
 * imperative handle from its single footer button (one Save for the whole form);
 * the connections drawer is standalone and renders its own Save.
 */
export interface NestedEditorPanelProps {
  /** The root inventory entity (e.g. `Devices`, `Virtual Machines`). */
  entity: InventoryEntityKey;
  /** The root row's real id. */
  rootRealId: string;
  /** When true, every edit/add/delete affordance is hidden (view-only). */
  readOnly?: boolean;
  /** Notified after a save completes (any outcome) so the host can refresh. */
  onSaved?: () => void;
  /** Hide the built-in Save button (the host drives `save()` via the ref). */
  hideSave?: boolean;
  /** Optional "mark primary IP" wiring, applied to the IP relation level only. */
  primarySelection?: PrimarySelection;
}

/** Imperative handle so a host (the inventory dialog) can fold the nested save
 *  into its own single Save action. */
export interface NestedEditorPanelHandle {
  save: () => Promise<void>;
}

/** Collect every related entity the config row forms across these specs may
 *  need options for (FK pickers other than the implied parent FK). */
function relatedEntitiesForSpecs(specs: RelationSpec[]): InventoryEntityKey[] {
  const set = new Set<InventoryEntityKey>();
  const walk = (spec: RelationSpec) => {
    if (spec.rowForm.kind === "config") {
      for (const e of relatedEntitiesForEntity(spec.rowForm.entity)) set.add(e);
    }
    if (spec.child) walk(spec.child);
  };
  specs.forEach(walk);
  return [...set];
}

const NestedEditorPanel = forwardRef<NestedEditorPanelHandle, NestedEditorPanelProps>(
  function NestedEditorPanel(
    { entity, rootRealId, readOnly = false, onSaved, hideSave = false, primarySelection },
    ref,
  ) {
    const { t } = useTranslation();
    const qc = useQueryClient();
    const specs = useMemo<RelationSpec[]>(() => relationshipsByTab[entity] ?? [], [entity]);

    const descriptors = useMemo(() => descriptorsFromSpecs(specs, t), [specs, t]);
    const tree: NestedEditorTree = useNestedEditorTree({
      rootRealId,
      relations: descriptors,
      maxPageSize: INVENTORY_MAX_PAGE_SIZE,
    });
    // `loadRootRelation` is one of the tree's stable useCallback members, so it
    // can be the effect dependency instead of the whole freshly-built tree.
    const { loadRootRelation } = tree;

    // Eagerly load each top-level relation once the panel mounts for this root.
    useEffect(() => {
      if (!rootRealId) return;
      for (const desc of descriptors) {
        loadRootRelation(desc.key);
      }
    }, [rootRealId, descriptors, loadRootRelation]);

    // Related-entity option lists for the config row pickers. One query per
    // related entity, cached and shared with the rest of the app.
    const relatedEntities = useMemo(() => relatedEntitiesForSpecs(specs), [specs]);
    const optionQueries = useQueries({
      queries: relatedEntities.map((relEntity) => ({
        queryKey: ["inventory", "resources", relEntity, "nested-options"],
        queryFn: () => listAllResources(relEntity),
        staleTime: 30_000,
      })),
    });
    const optionsByEntity = useMemo(() => {
      const map = new Map<InventoryEntityKey, EntityPickerOption[]>();
      relatedEntities.forEach((relEntity, i) => {
        const rows = optionQueries[i]?.data ?? [];
        map.set(relEntity, rows.map((r) => inventoryResourceOption(relEntity, r)));
      });
      return map;
    }, [relatedEntities, optionQueries]);
    const loadingByEntity = useMemo(() => {
      const map = new Map<InventoryEntityKey, boolean>();
      relatedEntities.forEach((relEntity, i) => {
        map.set(relEntity, optionQueries[i]?.isPending ?? false);
      });
      return map;
    }, [relatedEntities, optionQueries]);

    const loadOptions = (relEntity: InventoryEntityKey): EntityPickerOption[] =>
      optionsByEntity.get(relEntity) ?? [];
    const isOptionsLoading = (relEntity: InventoryEntityKey): boolean =>
      loadingByEntity.get(relEntity) ?? false;

    // Inline-create of a related entity from a row picker's "+". We remember which
    // node + field requested it, create through the same API the inventory page
    // uses, then write the new id back into that draft so it is auto-selected.
    const [createReq, setCreateReq] = useState<{
      entity: InventoryEntityKey;
      fieldKey: string;
      nodeLocalId: LocalId;
    } | null>(null);

    const createEntity = async (
      createKind: InventoryEntityKey,
      resource: Resource,
    ): Promise<Resource | null> => {
      try {
        const { id: _omit, ...payload } = resource;
        const created = await createResource(createKind, payload);
        void qc.invalidateQueries({ queryKey: ["inventory", "resources", createKind] });
        void qc.invalidateQueries({ queryKey: ["inventory", "resources", createKind, "nested-options"] });
        return created;
      } catch (err) {
        notify("error", err instanceof Error ? err.message : t("unknownError"));
        return null;
      }
    };

    const handleRequestCreate = (
      createKind: InventoryEntityKey,
      fieldKey: string,
      nodeLocalId: LocalId,
    ) => setCreateReq({ entity: createKind, fieldKey, nodeLocalId });

    const handleRootCreated = (resource: Resource) => {
      if (createReq) tree.patchDraft(createReq.nodeLocalId, createReq.fieldKey, resource.id);
      setCreateReq(null);
    };

    const [saving, setSaving] = useState(false);

    const runSave = useCallback(async () => {
      const result = await tree.save();
      const touched = new Set<InventoryEntityKey>();
      specs.forEach((spec) => {
        touched.add(spec.childEntity);
        if (spec.child) touched.add(spec.child.childEntity);
      });
      for (const e of touched) {
        void qc.invalidateQueries({ queryKey: ["inventory", "resources", e] });
      }
      if (!result.ok) {
        notify("error", t("nestedEditor.partialFailure", { count: result.failed.length }));
      }
      onSaved?.();
    }, [tree, specs, qc, t, onSaved]);

    // Host-driven save (inventory dialog): run only if there is nested work, and
    // never toast a separate success — the dialog owns the overall save feedback.
    useImperativeHandle(
      ref,
      () => ({
        save: async () => {
          if (!tree.isDirty) return;
          await runSave();
        },
      }),
      // runSave closes over the current tree; rebuild the handle when either changes.
      [tree, runSave],
    );

    const handleSaveClick = async () => {
      if (saving) return;
      setSaving(true);
      try {
        await runSave();
        if (tree.isDirty) return; // a partial failure leaves dirty nodes
        notify("success", t("nestedEditor.saved"));
      } finally {
        setSaving(false);
      }
    };

    if (specs.length === 0) return null;

    return (
      <div className="space-y-4">
        {descriptors.map((desc, i) => {
          const spec = specs[i];
          if (!spec) return null;
          return (
            <NestedEditorSection
              key={desc.key}
              spec={spec}
              tree={tree}
              readOnly={readOnly}
              loadOptions={loadOptions}
              isOptionsLoading={isOptionsLoading}
              onRequestCreate={handleRequestCreate}
              primarySelection={primarySelection}
            />
          );
        })}

        {!readOnly && !hideSave && (
          <div className="flex items-center justify-end gap-2 border-t border-border pt-3">
            <Button
              type="button"
              size="sm"
              onClick={() => void handleSaveClick()}
              disabled={saving || !tree.isDirty || tree.hasInvalid}
            >
              {saving ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Save className="h-3.5 w-3.5" />}
              {t("save")}
            </Button>
          </div>
        )}

        {createReq && (
          <EntityCreateEditor
            open
            onOpenChange={(open) => { if (!open) setCreateReq(null); }}
            rootEntity={createReq.entity}
            createEntity={createEntity}
            loadOptions={loadOptions}
            isOptionsLoading={isOptionsLoading}
            onRootCreated={handleRootCreated}
          />
        )}
      </div>
    );
  },
);

export default NestedEditorPanel;
