import {
  inventoryEntityHasStatus,
  type InventoryEntityKey,
  type ObjectRefProjection,
} from "@/shared/api/types";
import {
  fieldsByTab,
  inventoryModelCapabilities,
  type Resource,
} from "@/features/inventory/components/inventory/inventoryTypes";
import { isValidHex } from "@/shared/forms/colorUtils";
import { parseServicePortMappings } from "@/features/inventory/servicePortMappings";
import { interfaceNetworkingFields, parseInterfaceNetworkingValue } from "@/features/inventory/interfaceNetworking";
import { parseL2VPNIdentifier } from "@/features/inventory/terminationGraph";
import { parseRearPortMappings } from "@/features/inventory/rearPortMappings";

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
  "Virtual Circuit Terminations", "L2VPN Terminations",
  "VLAN Translation Rules",
  "Circuit Group Assignments",
  "Wireless LANs",
  "Wireless Links",
  "FHRP Group Assignments",
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

export const entityRequiresName = (entity: InventoryEntityKey): boolean =>
  entityHasName(entity) && inventoryModelCapabilities(entity)?.nameRequired !== false;

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
  if ((entity === "Module Bays" || entity === "Module Bay Templates" || entity === "Device Bay Templates") && !editing) vals.enabled = "true";
  if (entity === "Interface Templates" && !editing) {
    vals.enabled = "true";
    vals.mgmtOnly = "false";
  }
  if ((entity === "Front Port Templates" || entity === "Rear Port Templates") && !editing) vals.positions = "1";
  if (entity === "Tags" && !editing) vals.weight = "0";
  if (entity === "Virtual Circuit Terminations" && !editing) vals.role = "peer";
  if (entity === "L2VPNs" && !editing) vals.status = "active";
  if (editing) {
    vals.name = editing.name;
    if (hasStatus) vals.status = editing.status ?? "";
    fields.forEach((f) => {
      if (f.type === "nested-children" || f.type === "managed-secret") return;
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
      if (f.type === "identity-multiple" || f.type === "related-multiple" || f.type === "content-type-scope" || f.type === "port-mappings") {
        vals[stateKey] = (editing[stateKey as keyof Resource] as string[] | undefined)?.join(f.type === "port-mappings" ? ", " : ",") ?? "";
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
    ...(entity === "FHRP Groups" || entity === "FHRP Group Assignments" ? {
      tags: editing?.tags, externalId: editing?.externalId, pipingSyncedAt: editing?.pipingSyncedAt,
    } : {}),
  };
  if (hasStatus && draft.status) resource.status = draft.status as Resource["status"];
  // `Resource` is a local UI type with statically-known columns; the field-grid
  // writes dynamic per-field keys, so these are structural index assignments on
  // the local type — not a backend-contract bypass.
  fields.forEach((f) => {
    if (f.type === "nested-children" || f.readOnly) return;
    const stateKey = (f.idKey ?? f.key) as string;
    const val = f.type === "password" ? draft[stateKey] : draft[stateKey]?.trim();
    if (f.type === "port-mappings") {
      const mappings = parseServicePortMappings(val ?? "");
      if (!mappings) throw new Error("Invalid service port mappings");
      resource.portMappings = mappings;
      return;
    }
    if (f.type === "managed-secret") {
      const intent = draft[`${stateKey}__intent`];
      if (intent === "clear") resource.authKeyIntent = { intent };
      else if ((intent === "set" || intent === "rotate") && val) {
        resource.authKeyIntent = { intent, value: draft[stateKey] };
      }
      return;
    }
    if (f.type === "identity-multiple" || f.type === "related-multiple" || f.type === "content-type-scope") {
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
      if (!token && !val) {
        // A cleared optional typed reference is an explicit null so the native
        // owner can atomically clear both id columns.
        if (f.emptyValue === null) (resource as unknown as Record<string, unknown>)[stateKey] = null;
        return;
      }
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

/** Native partial updates: only operator changes are sent, omitted members and fields are preserved. */
const PARTIAL_UPDATE_ENTITIES: InventoryEntityKey[] = [
  "Virtual Circuit Terminations", "L2VPNs", "L2VPN Terminations",
  "VLAN Translation Policies", "VLAN Translation Rules", "Interfaces", "VM Interfaces",
  "Circuit Group Assignments",
  "Module Types",
  "Module Type Profiles",
  "Module Bay Templates",
  "Console Port Templates", "Console Server Port Templates",
  "Power Port Templates", "Power Outlet Templates",
  "Interface Templates", "Front Port Templates", "Rear Port Templates",
  "Device Bay Templates", "Inventory Item Templates",
  "Cooling Sources", "Cooling Feeds", "Cooling Intakes", "Cooling Outflows",
  "Cooling Intake Templates", "Cooling Outflow Templates",
  "Wireless LAN Groups", "Wireless LANs", "Wireless Links",
  "Module Bay Types", "Module Bays", "Module Types", "Modules",
  "Tags",
];

/** Native PATCH sends operator changes, never stale read snapshots. */
export const draftToResourceUpdate = (
  entity: InventoryEntityKey,
  draft: EntityFormValues,
  singular: string,
  editing: Resource,
  initialDraft: EntityFormValues,
): EntityResourceUpdate => {
  const resource = draftToResource(entity, draft, singular, editing);
  // Imported dates are opaque until edited; unrelated saves must not rewrite them.
  if (["Devices", "Device Types", "Module Types", "Racks", "Rack Types"].includes(entity)) {
    for (const key of ["coolingMethod", "endOfLife", "coolingCapability", "coolingCapacity"] as const) {
      if (draft[key] === initialDraft[key]) delete resource[key];
    }
  }
  if (entity === "Interfaces") {
    for (const { key } of interfaceNetworkingFields) {
      if (draft[key] === initialDraft[key]) delete resource[key];
    }
    for (const key of ["wirelessLinkId", "wirelessLans"] as const) {
      if (draft[key] === initialDraft[key]) delete resource[key];
    }
    return resource;
  }
  if (entity === "Virtual Machines") {
    // These nullable PUT fields retain omission, unlike the other VM scalars.
    for (const key of ["platformId", "virtualMachineTypeId", "vcpus", "memory"] as const) {
      if (draft[key] === initialDraft[key]) delete resource[key];
    }
    return resource;
  }
  if (entity !== "Owners" && entity !== "Owner Groups" && entity !== "Virtual Machine Types" && !PARTIAL_UPDATE_ENTITIES.includes(entity)) return resource;
  const changed = Object.entries(resource).filter(
    ([key]) => key in initialDraft && (draft[key] !== initialDraft[key] ||
      (fieldsByTab[entity] || []).some(field => field.type === "objectRef" && (field.idKey ?? field.key) === key && draft[objectRefTypeKey(key)] !== initialDraft[objectRefTypeKey(key)])),
  );
  return { id: editing.id, ...Object.fromEntries(changed) };
};

const MODULAR_COMPONENT_TEMPLATE_ENTITIES: InventoryEntityKey[] = [
  "Module Bay Templates",
  "Console Port Templates", "Console Server Port Templates",
  "Power Port Templates", "Power Outlet Templates",
  "Interface Templates", "Front Port Templates", "Rear Port Templates",
];

const COMPONENT_TEMPLATE_NAME_ENTITIES: InventoryEntityKey[] = [
  ...MODULAR_COMPONENT_TEMPLATE_ENTITIES,
  "Module Bays",
  "Device Bay Templates", "Inventory Item Templates",
];

/** Validates one entity's draft. Returns a key→message error map. */
export const validateEntityDraft = (
  entity: InventoryEntityKey,
  draft: EntityFormValues,
  t: (key: string, opts?: Record<string, unknown>) => string,
): Record<string, string> => {
  const errs: Record<string, string> = {};
  if (entity === "L2VPNs" && draft.identifier) {
    try { parseL2VPNIdentifier(draft.identifier); }
    catch { errs.identifier = t("inventory.resources.form.l2vpnIdentifier"); }
  }
  const fields = fieldsByTab[entity] || [];
  if (MODULAR_COMPONENT_TEMPLATE_ENTITIES.includes(entity) && Boolean(draft.deviceTypeId) === Boolean(draft.moduleTypeId)) {
    errs.deviceTypeId = t("inventory.resources.form.hardwareTemplateParent");
  }
  if (entity === "Power Port Templates" && draft.maximumDraw && draft.allocatedDraw &&
    Number(draft.allocatedDraw) > Number(draft.maximumDraw)) {
    errs.allocatedDraw = t("inventory.resources.form.allocatedDrawExceedsMaximum");
  }
  if (entity === "Front Port Templates" && draft.rearPorts?.trim()) {
    try {
      const mappings = parseRearPortMappings(draft.rearPorts) ?? [];
      const frontPositions = Number(draft.positions);
      const duplicate = new Set<string>();
      for (const mapping of mappings) {
        if (Number.isInteger(frontPositions) && mapping.position > frontPositions) throw new Error("position");
        if (duplicate.has(String(mapping.position))) throw new Error("duplicate");
        duplicate.add(String(mapping.position));
      }
    } catch {
      errs.rearPorts = t("inventory.resources.form.rearPortMappingsInvalid");
    }
  }
  if (entity === "Module Type Profiles" && draft.schemaSummary?.trim()) {
    try {
      const schema: unknown = JSON.parse(draft.schemaSummary);
      if (schema !== null && (typeof schema !== "object" || Array.isArray(schema))) throw new Error("object required");
    } catch {
      errs.schemaSummary = t("inventory.resources.form.profileSchemaObject");
    }
  }
  if (entity === "Module Types" && draft.attributeDataSummary?.trim()) {
    try { JSON.parse(draft.attributeDataSummary); } catch {
      errs.attributeDataSummary = t("inventory.resources.form.attributeDataJson");
    }
  }
  if (COMPONENT_TEMPLATE_NAME_ENTITIES.includes(entity) && (draft.name?.trim().length ?? 0) > 64) {
    errs.name = t("inventory.resources.form.fieldTooLong", { max: 64 });
  }
  if (entity.startsWith("Cooling ")) {
    if (entity.includes("Template") && Boolean(draft.deviceTypeId) === Boolean(draft.moduleTypeId)) {
      errs.deviceTypeId = t("inventory.resources.form.hardwareTemplateParent");
    }
    if ((entity.includes("Intake") || entity.includes("Outflow")) && (draft.name?.trim().length ?? 0) > 64) {
      errs.name = t("inventory.resources.form.fieldTooLong", { max: 64 });
    }
    for (const [valueKey, unitKey] of [["diameter", "diameterUnit"], ["maxFlow", "maxFlowUnit"]] as const) {
      if (Boolean(draft[valueKey]) !== Boolean(draft[unitKey])) errs[unitKey] = t("inventory.resources.form.coolingUnitPair");
    }
  }
  if (entityHasName(entity)) {
    if (entityRequiresName(entity) && !draft.name?.trim()) errs.name = t("accessControl.groups.errors.nameRequired");
    else if ((draft.name?.trim().length ?? 0) > 100) errs.name = t("inventory.resources.form.nameTooLong");
  }
  if (entity === "Interfaces") {
    for (const { key } of interfaceNetworkingFields) {
      try {
        parseInterfaceNetworkingValue(key, draft[key]);
      } catch {
        errs[key] = t("inventory.resources.form.invalidNetworkingValue");
      }
    }
  }
  if (entity === "Wireless LANs" && Boolean(draft.scopeType) !== Boolean(draft.scopeId)) {
    errs.scopeId = t("inventory.resources.form.scopePairRequired");
  }
  if (entity === "Wireless Links") {
    if (draft.interfaceAId && draft.interfaceAId === draft.interfaceBId) errs.interfaceBId = t("inventory.resources.form.differentInterfaces");
    if (draft.distance && !draft.distanceUnit) errs.distanceUnit = t("inventory.resources.form.distanceUnitRequired");
  }
  fields.forEach((f) => {
    if (f.type === "nested-children") return;
    const stateKey = (f.idKey ?? f.key) as string;
    const value = draft[stateKey]?.trim() ?? "";
    if (f.type === "port-mappings" && !parseServicePortMappings(value)) {
      errs[stateKey] = t("inventory.resources.fields.portMappingsError");
      return;
    }
    if (entity === "Wireless Links" && stateKey === "distance" && value && (!Number.isFinite(Number(value)) || Number(value) < 0 || Number(value) > 999999.99)) {
      errs[stateKey] = t("inventory.resources.form.invalidDistance");
      return;
    }
    if (entity === "Wireless LANs" && stateKey === "scopeId" && value && !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)) {
      errs[stateKey] = t("inventory.resources.form.invalidUuid");
      return;
    }
    if (f.type === "managed-secret") {
      const intent = draft[`${stateKey}__intent`];
      if ((intent === "set" || intent === "rotate") && !value) errs[stateKey] = t("required");
      return;
    }
    if (f.maxLength && value.length > f.maxLength) {
      errs[stateKey] = t("inventory.resources.form.fieldTooLong", { max: f.maxLength });
      return;
    }
    if (f.type === "color" && value && !isValidHex(value)) {
      errs[stateKey] = t("inventory.resources.form.colorField.invalid");
      return;
    }
    // Networking has its own precision-aware parser above, including fractional
    // RF values. The generic integer rule must not replace that validation.
    const isNetworkingField = entity === "Interfaces" && interfaceNetworkingFields.some(({ key }) => key === stateKey);
    if (f.type === "number" && !isNetworkingField && value && (f.min !== undefined || f.max !== undefined)) {
      const number = Number(value);
      const decimal = f.numberMode === "decimal";
      if (!Number.isFinite(number) || (!decimal && !Number.isInteger(number)) || (decimal && !/^\d+(\.\d{1,2})?$/.test(value)) || (f.min !== undefined && number < f.min) || (f.max !== undefined && number > f.max)) {
        errs[stateKey] = t(decimal ? "inventory.resources.form.decimalRange" : "inventory.resources.form.integerRange", { min: f.min, max: f.max });
        return;
      }
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
