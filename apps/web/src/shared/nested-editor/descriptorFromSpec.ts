import {
  createResource,
  updateResource,
  deleteResource,
  listChildResourcesByRouteQuery,
  INVENTORY_MAX_PAGE_SIZE,
} from "@/features/inventory/api";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import { tabConfig } from "@/features/inventory/components/inventory/inventoryTypes";
import {
  buildEntityDraft,
  draftToResource,
  validateEntityDraft,
  type EntityFormValues,
} from "@/shared/forms/entityForm";
import type { AnyRelationDescriptor, RelationDescriptor } from "./useNestedEditorTree";

/**
 * Pure data declaration of one master-detail relation level, declared beside the
 * inventory field config (`relationshipsByTab`). It names the child entity, the
 * server list-filter param keyed off the parent, the FK field the child create
 * needs, and how each row is edited. `descriptorFromSpec` compiles it into the
 * runtime {@link RelationDescriptor} the headless core consumes — both hosts
 * (inventory dialog + connections drawer) compile the SAME spec, so behavior is
 * identical regardless of where it is mounted.
 */
export interface RelationSpec {
  /** Stable relation key (used as the node specKey and i18n suffix). */
  key: string;
  /** The inventory entity edited at this level. */
  childEntity: InventoryEntityKey;
  /** i18n key for the section title. */
  title: string;
  /** lucide icon name string (resolved to a component by the section view). */
  icon: string;
  /** Backend list-filter param that scopes children to one parent real id. */
  listFilterKey: "device_id" | "assigned_object_id" | "virtual_machine_id";
  /** Backend list route for the child entity (kebab/underscore as deployed). */
  listRoute: string;
  /** FK field on the child Resource (`idKey`) that must carry the parent id. */
  parentFkField: keyof Resource;
  /** Row-form strategy. `config` reuses the inventory field grid; `custom` lets
   *  a caller provide its own draft/payload/validate (none needed yet). */
  rowForm: RelationRowForm;
  /** Page size for child fetches (clamped to the server cap). */
  pageLimit?: number;
  /** The single nested child relation, if this level has one. */
  child?: RelationSpec;
}

export type RelationRowForm =
  | { kind: "config"; entity: InventoryEntityKey }
  | {
      kind: "custom";
      buildDraft: (row?: Resource) => Record<string, string>;
      toDraft: (row: Resource) => Record<string, string>;
      toPayload: (draft: Record<string, string>, parentRealId?: string) => Record<string, unknown>;
      validate: (draft: Record<string, string>) => Record<string, string>;
    };

/** Translate function shape the validators need (subset of i18next `t`). */
export type TFunc = (key: string, opts?: Record<string, unknown>) => string;

function singularFor(entity: InventoryEntityKey): string {
  return tabConfig.find((c) => c.key === entity)?.singular ?? entity;
}

/**
 * Compile a {@link RelationSpec} (and its nested child, recursively) into a
 * runtime {@link RelationDescriptor}. All temp-id→real-FK injection is
 * centralized in `toPayload`: it stamps the parent real id onto the spec's
 * `parentFkField` (a camelCase `Resource` idKey such as `deviceId` /
 * `interfaceId` / `virtualMachineId`) and then hands the assembled `Resource`
 * to `createResource`/`updateResource`, which run the canonical
 * `toBackendPayload` (camel→snake) — so the FK is converted exactly once.
 */
export function descriptorFromSpec(spec: RelationSpec, t: TFunc): RelationDescriptor<Resource> {
  const pageLimit = Math.min(spec.pageLimit ?? INVENTORY_MAX_PAGE_SIZE, INVENTORY_MAX_PAGE_SIZE);
  const singular = singularFor(spec.childEntity);

  const buildDraft = (row?: Resource): Record<string, string> => {
    if (spec.rowForm.kind === "custom") return spec.rowForm.buildDraft(row);
    return buildEntityDraft(spec.rowForm.entity, row ?? null);
  };
  const toDraft = (row: Resource): Record<string, string> => {
    if (spec.rowForm.kind === "custom") return spec.rowForm.toDraft(row);
    return buildEntityDraft(spec.rowForm.entity, row);
  };
  const validate = (draft: Record<string, string>): Record<string, string> => {
    if (spec.rowForm.kind === "custom") return spec.rowForm.validate(draft);
    const errs = validateEntityDraft(spec.rowForm.entity, draft, t);
    // The parent FK is satisfied by nesting (injected at save time), so a
    // missing-FK error must not block the row — drop it.
    delete errs[spec.parentFkField];
    return errs;
  };

  /** Build the `Resource` write body from a draft, injecting the parent FK. */
  const toPayload = (
    draft: Record<string, string>,
    parentRealId?: string,
  ): Record<string, unknown> => {
    if (spec.rowForm.kind === "custom") {
      return spec.rowForm.toPayload(draft, parentRealId);
    }
    const merged: EntityFormValues = { ...draft };
    if (parentRealId) merged[spec.parentFkField] = parentRealId;
    // `draftToResource` mirrors the create/edit dialog mapping (status, nameless
    // label synthesis, FK idKeys). The result is a UI `Resource`; the api layer
    // runs `toBackendPayload` to coerce it to the snake_case wire shape.
    const resource = draftToResource(spec.childEntity, merged, singular, null);
    // Strip the synthetic id so create/update don't treat the draft id as real.
    const { id: _id, ...body } = resource;
    void _id;
    return body;
  };

  return {
    key: spec.key,
    title: spec.title,
    parentFkField: spec.parentFkField,
    pageLimit,
    listByParent: async (parentRealId, page) => {
      const result = await listChildResourcesByRouteQuery(spec.childEntity, spec.listRoute as never, {
        [spec.listFilterKey]: parentRealId,
        limit: page.limit,
        offset: page.offset,
      });
      return { rows: result.rows, total: result.total, loaded: result.offset + result.rows.length };
    },
    create: async (payload) => {
      const created = await createResource(
        spec.childEntity,
        payload as unknown as Omit<Resource, "id">,
      );
      return { id: created.id, raw: created };
    },
    update: async (realId, patch) =>
      updateResource(spec.childEntity, realId, patch),
    remove: async (realId) => {
      await deleteResource(spec.childEntity, realId);
    },
    buildDraft,
    toDraft,
    toPayload,
    validate,
    // The core treats descriptors opaquely (it only ever feeds a child its own
    // rows), so the generic-parameter variance between `RelationDescriptor<Resource>`
    // and the field's `RelationDescriptor<{ id: string }>` default is safe to widen.
    childRelation: spec.child
      ? (descriptorFromSpec(spec.child, t) as unknown as RelationDescriptor)
      : undefined,
  };
}

/** Compile a list of root relation specs (one root entity's levels). The result
 *  is widened to {@link AnyRelationDescriptor} at the seam — the headless core
 *  treats descriptors opaquely (it never narrows `TChild` across levels). */
export function descriptorsFromSpecs(
  specs: RelationSpec[],
  t: TFunc,
): AnyRelationDescriptor[] {
  return specs.map((s) => descriptorFromSpec(s, t) as unknown as AnyRelationDescriptor);
}
