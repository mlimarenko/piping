import {
  inventoryEntityHasStatus,
  type InventoryEntityKey,
  type ObjectRefProjection,
} from "@/shared/api/types";
import {
  fieldsByTab,
  type Resource,
} from "@/features/inventory/components/inventory/inventoryTypes";
import { isValidHex } from "@/shared/forms/colorUtils";

/** Form values are keyed on the FK-UUID column (`idKey`) for related fields. */
export type EntityFormValues = Record<string, string>;
export type EntityResourceUpdate = Pick<Resource, "id"> & Partial<Resource>;

/** Suffix for the companion draft key that holds an `objectRef` field's chosen
 *  `app_label.model` token (the target UUID lives under the field's own key). */
export const objectRefTypeKey = (stateKey: string): string => `${stateKey}__type`;

/** Entities with no `name` column — the name field is dropped and a label is
 * synthesized on save (the backend ignores `name` for them).
 *
 * This MUST stay in sync with the read-side "nameless on the wire" label
 * derivation in `features/inventory/api.ts` (the `mapResource` name fallback):
 * those entities render their display label from a non-name column
 * (address / prefix / role / cid) or from a composite/`id` identity. If an
 * entity is nameless on read but absent here, the edit form shows a phantom
 * required "Name" field whose value the backend silently drops on save — the
 * operator edits it, sees "saved", reopens, and finds the old derived value. */
export const NAMELESS_ENTITIES: InventoryEntityKey[] = [
  "Contact Assignments",
  "Aggregates",
  "ASNs",
  "IP Ranges",
  "Modules",
  // IPAM: the CIDR/address is the identity (`prefix` field), `name` is unused.
  "IP Addresses",
  "Prefixes",
  // VPN/circuit terminations: identified by role / side / circuit, no `name`.
  "Tunnel Terminations",
  "Circuit Terminations",
  // Rack reservation: identified by rack + units + user, no `name`.
  "Rack Reservations",
  // MAC: identified by the `macAddress` field (now required), no `name` column.
  "MAC Addresses",
];

export const entityHasName = (entity: InventoryEntityKey): boolean =>
  !NAMELESS_ENTITIES.includes(entity);

/** Builds an empty (or editing-seeded) draft for one entity's field-grid. */
export const buildEntityDraft = (
  entity: InventoryEntityKey,
  editing?: Resource | null,
): EntityFormValues => {
  const hasStatus = inventoryEntityHasStatus(entity);
  const fields = fieldsByTab[entity] || [];
  const vals: EntityFormValues = { name: "" };
  if (hasStatus) vals.status = "";
  fields.forEach((f) => {
    if (f.type === "nested-children") return;
    const stateKey = (f.idKey ?? f.key) as string;
    vals[stateKey] = "";
    if (f.type === "icon") vals[(f.iconDataKey ?? "iconDataUrl") as string] = "";
    if (f.type === "objectRef") vals[objectRefTypeKey(stateKey)] = "";
  });
  if (editing) {
    vals.name = editing.name;
    if (hasStatus) vals.status = editing.status ?? "";
    fields.forEach((f) => {
      if (f.type === "nested-children") return;
      const stateKey = (f.idKey ?? f.key) as string;
      if (f.type === "objectRef") {
        // The objectRef projection is a nested object, not a string column:
        // seed the target UUID under the field key and the token under the
        // companion `__type` key.
        const ref = editing[stateKey as keyof Resource] as ObjectRefProjection | null | undefined;
        vals[stateKey] = ref?.objectId ?? "";
        vals[objectRefTypeKey(stateKey)] = ref?.objectType ?? "";
        return;
      }
      vals[stateKey] = (editing[stateKey as keyof Resource] as string) || "";
      if (f.type === "identity-multiple" || f.type === "related-multiple") {
        vals[stateKey] = (editing[stateKey as keyof Resource] as string[] | undefined)?.join(",") ?? "";
      }
      if (f.type === "icon") {
        const dataKey = (f.iconDataKey ?? "iconDataUrl") as string;
        vals[dataKey] = (editing[dataKey as keyof Resource] as string) || "";
      }
    });
  }
  return vals;
};

/** Maps a validated draft into the `Resource` shape the inventory API consumes.
 * Mirrors the historical ResourceFormDialog save logic: nameless entities
 * synthesize a label, status is attached when present, and every non-empty
 * field value is copied under its state key (the FK `idKey` for related). */
export const draftToResource = (
  entity: InventoryEntityKey,
  draft: EntityFormValues,
  singular: string,
  editing?: Resource | null,
): Resource => {
  const hasStatus = inventoryEntityHasStatus(entity);
  const hasName = entityHasName(entity);
  const fields = fieldsByTab[entity] || [];
  const resource: Resource = {
    id: editing?.id || String(Date.now()),
    name: hasName ? (draft.name ?? "").trim() : (editing?.name ?? singular),
    type: editing?.type || entity.toLowerCase(),
    customFields: editing?.customFields,
  };
  if (hasStatus && draft.status) resource.status = draft.status as Resource["status"];
  // `Resource` is a local UI type with statically-known columns; the field-grid
  // writes dynamic per-field keys, so these are structural index assignments on
  // the local type — not a backend-contract bypass.
  fields.forEach((f) => {
    if (f.type === "nested-children") return;
    const stateKey = (f.idKey ?? f.key) as string;
    const val = draft[stateKey]?.trim();
    if (f.type === "identity-multiple" || f.type === "related-multiple") {
      (resource as unknown as Record<string, unknown>)[stateKey] = val ? val.split(",") : [];
      return;
    }
    if (!val && f.emptyValue !== undefined) {
      (resource as unknown as Record<string, unknown>)[stateKey] = f.emptyValue;
      return;
    }
    if (f.type === "icon") {
      const dataKey = (f.iconDataKey ?? "iconDataUrl") as string;
      (resource as unknown as Record<string, unknown>)[stateKey] = val ?? "";
      (resource as unknown as Record<string, unknown>)[dataKey] = draft[dataKey]?.trim() ?? "";
      return;
    }
    if (f.type === "objectRef") {
      // Reassemble the nested ObjectRef projection from the two draft keys.
      // `label` is write-only-absent (the backend rejects it); set null. Only
      // emit when both halves are present so the write builder can omit/clear.
      const token = draft[objectRefTypeKey(stateKey)]?.trim();
      if (token && val) {
        (resource as unknown as Record<string, unknown>)[stateKey] = {
          objectType: token,
          objectId: val,
          label: null,
        } satisfies ObjectRefProjection;
      }
      return;
    }
    if (val) (resource as unknown as Record<string, unknown>)[stateKey] = val;
  });
  return resource;
};

/** Native PATCH sends operator changes, never stale read snapshots. */
export const draftToResourceUpdate = (
  entity: InventoryEntityKey,
  draft: EntityFormValues,
  singular: string,
  editing: Resource,
  initialDraft: EntityFormValues,
): EntityResourceUpdate => {
  const resource = draftToResource(entity, draft, singular, editing);
  if (entity === "Virtual Machines") {
    // These nullable PUT fields retain omission, unlike the other VM scalars.
    for (const key of ["platformId", "virtualMachineTypeId", "vcpus", "memory"] as const) {
      if (draft[key] === initialDraft[key]) delete resource[key];
    }
    return resource;
  }
  if (entity !== "Owners" && entity !== "Owner Groups" && entity !== "Virtual Machine Types") return resource;
  const changed = Object.entries(resource).filter(
    ([key]) => key in initialDraft && draft[key] !== initialDraft[key],
  );
  return { id: editing.id, ...Object.fromEntries(changed) };
};

/** Validates one entity's draft. Returns a key→message error map. */
export const validateEntityDraft = (
  entity: InventoryEntityKey,
  draft: EntityFormValues,
  t: (key: string, opts?: Record<string, unknown>) => string,
): Record<string, string> => {
  const errs: Record<string, string> = {};
  const fields = fieldsByTab[entity] || [];
  if (entityHasName(entity)) {
    if (!draft.name?.trim()) errs.name = t("accessControl.groups.errors.nameRequired");
    else if (draft.name.trim().length > 100) errs.name = t("inventory.resources.form.nameTooLong");
  }
  fields.forEach((f) => {
    if (f.type === "nested-children") return;
    const stateKey = (f.idKey ?? f.key) as string;
    const value = draft[stateKey]?.trim() ?? "";
    if (f.maxLength && value.length > f.maxLength) {
      errs[stateKey] = t("inventory.resources.form.fieldTooLong", { max: f.maxLength });
      return;
    }
    if (f.type === "color" && value && !isValidHex(value)) {
      errs[stateKey] = t("inventory.resources.form.colorField.invalid");
      return;
    }
    if (!f.required) return;
    // An objectRef is satisfied only when BOTH the type token and the target
    // UUID are chosen; a partial selection must not pass.
    const missing =
      f.type === "objectRef"
        ? !draft[objectRefTypeKey(stateKey)]?.trim() || !draft[stateKey]?.trim()
        : !value;
    if (missing) {
      errs[stateKey] = `${t(f.label, { defaultValue: f.label })}: ${t("required")}`;
    }
  });
  return errs;
};
