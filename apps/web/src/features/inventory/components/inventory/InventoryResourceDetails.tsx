import { useMemo, type ReactNode } from "react";
import { Link } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { ResourceWithRaw } from "@/features/inventory/api";
import type { InventoryEntityKey, Resource } from "@/shared/api/types";
import { UUID_RE } from "@/shared/lib/ids";
import IdentitySelection from "@/features/access-control/components/access/IdentitySelection";
import { inventoryResourceDetailHref } from "@/features/inventory/resourceLinks";
import { fieldsByTab } from "./inventoryTypes";
import { formatInventoryValue, humanizeKey, snakeKey } from "./inventoryRawFields";
import InventoryRelatedValues from "./InventoryRelatedValues";
import EntityOptionLabel from "@/shared/ui-shared/EntityOptionLabel";
import EntityRef from "@/shared/ui-shared/EntityRef";
import {
  inventoryRelationKey,
  useInventoryRelationLabels,
  type InventoryRelationRef,
} from "@/features/inventory/relationLabels";
import { listContentTypes } from "@/features/inventory/custom-fields-api";

// These maps have definition-level visibility and secret policy, never raw JSON
// fallback. Nested relation payloads must not bypass that policy either.
const CUSTOM_CARRIERS = new Set(["custom_fields", "custom_field_data", "role_custom_fields", "customFields"]);
function publicValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(publicValue);
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value)
    .filter(([key]) => !CUSTOM_CARRIERS.has(key)).map(([key, item]) => [key, publicValue(item)]));
  return value;
}

export function inventoryDetailText(value: unknown): ReactNode {
  const formatted = formatInventoryValue(publicValue(value));
  return <span className="whitespace-pre-wrap break-words">{formatted.isEmpty ? "—" : formatted.value}</span>;
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

/** Raw FK keys whose target is not part of this entity's curated form. */
const RAW_RELATION_ENTITIES: Readonly<Record<string, InventoryEntityKey>> = {
  primary_ip4_id: "IP Addresses",
  primary_ip6_id: "IP Addresses",
  cluster_id: "Clusters",
  virtual_chassis_id: "Virtual Chassis",
};

function rawRelationEntities(entity: InventoryEntityKey): Map<string, InventoryEntityKey> {
  const map = new Map<string, InventoryEntityKey>(Object.entries(RAW_RELATION_ENTITIES));
  for (const field of fieldsByTab[entity] ?? []) {
    if (field.type === "related" && field.idKey && field.relatedTab) {
      map.set(snakeKey(field.idKey), field.relatedTab);
    }
  }
  return map;
}

function relationContent(label: string | undefined, saved: unknown, id: string | undefined): ReactNode {
  if (label) return inventoryDetailText(label);
  if (!formatInventoryValue(saved).isEmpty) return inventoryDetailText(saved);
  return <EntityRef id={id} />;
}

/** Full saved-field projection, not the compact inspector's omit set. */
export function useInventoryDetailFields(entity: InventoryEntityKey, { resource, raw }: ResourceWithRaw) {
  const { t } = useTranslation();
  const consumed = new Set([...CUSTOM_CARRIERS, "local_context_data", "localContextData", "name"]);
  const fields: { label: string; value: ReactNode }[] = [{ label: "ID", value: resource.id }];
  consumed.add("id");

  const rawEntities = useMemo(() => rawRelationEntities(entity), [entity]);
  const relationRefs = useMemo<InventoryRelationRef[]>(() => {
    const refs: InventoryRelationRef[] = [];
    for (const field of fieldsByTab[entity] ?? []) {
      if (field.type === "related" && field.relatedTab && field.idKey) {
        const id = resource[field.idKey] ?? raw[snakeKey(field.idKey)];
        if (typeof id === "string") refs.push({ entity: field.relatedTab, id });
      } else if (field.type === "objectRef") {
        const ref = record(resource[field.key] ?? raw[snakeKey(field.key)]);
        const id = ref?.objectId ?? ref?.object_id;
        const token = ref?.objectType ?? ref?.object_type;
        const related = field.objectTypes?.find(option => option.token === token)?.relatedTab;
        if (typeof id === "string" && related) refs.push({ entity: related, id });
      } else if (field.type === "related-multiple" && field.relatedTab) {
        const saved = resource[field.key] ?? raw[snakeKey(field.key)];
        if (Array.isArray(saved)) {
          for (const item of saved) {
            const id = typeof item === "string" ? item : record(item)?.id;
            if (typeof id === "string") refs.push({ entity: field.relatedTab, id });
          }
        }
      }
    }
    for (const [key, value] of Object.entries(raw)) {
      if (!key.endsWith("_id") || typeof value !== "string" || !UUID_RE.test(value)) continue;
      const related = rawEntities.get(key);
      if (related) refs.push({ entity: related, id: value });
    }
    return refs;
  }, [entity, rawEntities, resource, raw]);
  const relationLabels = useInventoryRelationLabels(relationRefs);
  const hasScopeField = (fieldsByTab[entity] ?? []).some(field => field.type === "content-type-scope");
  const contentTypes = useQuery({
    queryKey: ["inventory-content-types"],
    queryFn: listContentTypes,
    enabled: hasScopeField,
    staleTime: 5 * 60 * 1000,
  });
  const tokenById = useMemo(
    () => new Map((contentTypes.data ?? []).map(type => [type.id, type.token])),
    [contentTypes.data],
  );

  for (const field of fieldsByTab[entity] ?? []) {
    if (field.type === "nested-children") continue; // Declared child lists are a separate parent-scoped owner.
    const key = snakeKey(field.key);
    const idKey = field.idKey ? snakeKey(field.idKey) : undefined;
    const saved = resource[field.key] ?? raw[key];
    const label = t(field.label, { defaultValue: field.label });
    let value: ReactNode = inventoryDetailText(saved);
    if (field.type === "color" && typeof saved === "string") {
      value = <EntityOptionLabel label={saved} color={saved} />;
    } else if (field.type === "identity-multiple" && field.identityKind) {
      value = <IdentitySelection kind={field.identityKind} label={label} value={Array.isArray(saved) ? saved.filter((id): id is string => typeof id === "string") : []} />;
    } else if (field.type === "related") {
      const id = field.idKey ? resource[field.idKey] ?? raw[idKey!] : undefined;
      const href = typeof id === "string" ? inventoryResourceDetailHref(field.relatedTab, id) : null;
      const resolved = typeof id === "string" && field.relatedTab
        ? relationLabels.get(inventoryRelationKey(field.relatedTab, id))
        : undefined;
      const content = relationContent(resolved, saved, typeof id === "string" ? id : undefined);
      value = href ? <Link to={href} className="break-words text-primary underline">{content}</Link> : content;
    } else if (field.type === "objectRef") {
      const ref = record(saved);
      const id = ref?.objectId ?? ref?.object_id;
      const token = ref?.objectType ?? ref?.object_type;
      const target = field.objectTypes?.find(option => option.token === token);
      const related = target?.relatedTab;
      const href = typeof id === "string" ? target?.detailHref?.(id) ?? inventoryResourceDetailHref(related, id) : null;
      const resolved = typeof id === "string" && related
        ? relationLabels.get(inventoryRelationKey(related, id))
        : undefined;
      const display = resolved ?? ref?.label;
      const identity = ref
        ? <>{relationContent(typeof display === "string" ? display : undefined, undefined, typeof id === "string" ? id : undefined)}
          {typeof token === "string" ? ` (${token})` : ""}</>
        : "—";
      value = href ? <Link to={href} className="break-words text-primary underline">{identity}</Link> : identity;
    } else if (field.type === "content-type-scope" && Array.isArray(saved)) {
      value = saved.length === 0
        ? <span>{t("inventory.resources.fields.tagScopeUnrestricted")}</span>
        : <span className="break-words">{saved.map((item, index) => {
          const token = typeof item === "string" ? tokenById.get(item) : undefined;
          return <span key={index}>
            {token ?? (typeof item === "string" ? <EntityRef id={item} /> : String(item))}
            {index < saved.length - 1 ? ", " : ""}
          </span>;
        })}</span>;
    } else if (field.type === "related-multiple" && Array.isArray(saved)) {
      value = field.relatedTab && saved.every(item => typeof item === "string")
        ? <InventoryRelatedValues entity={field.relatedTab} ids={saved} />
        : <span className="flex flex-wrap gap-2">{saved.map((item, index) => {
        const ref = record(item);
        const id: unknown = ref?.id ?? item;
        const href = typeof id === "string" ? inventoryResourceDetailHref(field.relatedTab, id) : null;
        const resolved = typeof id === "string" && field.relatedTab
          ? relationLabels.get(inventoryRelationKey(field.relatedTab, id))
          : undefined;
        const content = relationContent(typeof resolved === "string" ? resolved : (typeof ref?.name === "string" ? ref.name : undefined),
          undefined, typeof id === "string" ? id : undefined);
        return href ? <Link key={index} to={href} className="break-words text-primary underline">{content}</Link> : <span key={index}>{content}</span>;
      })}</span>;
    }
    fields.push({ label, value });
    consumed.add(key);
    consumed.add(field.key);
    if (field.type === "related") consumed.add(`${key}_name`);
    if (idKey) consumed.add(idKey);
    if (field.idKey) consumed.add(field.idKey);
  }
  // Response-only schema, zero/false and long values are retained. A relation
  // UUID is resolved to its label; an unknown FK target is omitted rather than
  // printed as an opaque identifier.
  for (const [key, value] of Object.entries(raw)) {
    if (consumed.has(key)) continue;
    if (key === "tags" && Array.isArray(value) && value.every(item => typeof item === "string")) {
      fields.push({
        label: t("inventory.resources.fields.tags", { defaultValue: "Tags" }),
        value: <InventoryRelatedValues entity="Tags" ids={value} />,
      });
      continue;
    }
    if (key.endsWith("_id") && typeof value === "string" && UUID_RE.test(value)) {
      const related = rawEntities.get(key);
      if (!related) {
        // A response-only FK with no known tab still belongs to the complete
        // field set; keep it visible as an explicit unknown reference, never a
        // bare UUID.
        fields.push({ label: humanizeKey(key), value: <EntityRef id={value} /> });
        continue;
      }
      const resolved = relationLabels.get(inventoryRelationKey(related, value));
      const href = inventoryResourceDetailHref(related, value);
      const content = resolved ? inventoryDetailText(resolved) : <EntityRef id={value} />;
      fields.push({
        label: humanizeKey(key),
        value: href ? <Link to={href} className="break-words text-primary underline">{content}</Link> : content,
      });
      continue;
    }
    fields.push({ label: humanizeKey(key), value: inventoryDetailText(value) });
  }
  return fields;
}
