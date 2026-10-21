import type { ReactNode } from "react";
import { Link } from "react-router";
import { useTranslation } from "react-i18next";
import type { ResourceWithRaw } from "@/features/inventory/api";
import type { InventoryEntityKey } from "@/shared/api/types";
import IdentitySelection from "@/features/access-control/components/access/IdentitySelection";
import { inventoryResourceDetailHref } from "@/features/inventory/resourceLinks";
import { fieldsByTab } from "./inventoryTypes";
import { formatInventoryValue, humanizeKey, snakeKey } from "./inventoryRawFields";

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

/** Full saved-field projection, not the compact inspector's omit set. */
export function useInventoryDetailFields(entity: InventoryEntityKey, { resource, raw }: ResourceWithRaw) {
  const { t } = useTranslation();
  const consumed = new Set([...CUSTOM_CARRIERS, "local_context_data", "localContextData", "name"]);
  const fields: { label: string; value: ReactNode }[] = [{ label: "ID", value: resource.id }];
  consumed.add("id");
  for (const field of fieldsByTab[entity] ?? []) {
    if (field.type === "nested-children") continue; // Declared child lists are a separate parent-scoped owner.
    const key = snakeKey(field.key);
    const idKey = field.idKey ? snakeKey(field.idKey) : undefined;
    const saved = resource[field.key] ?? raw[key];
    const label = t(field.label, { defaultValue: field.label });
    let value: ReactNode = inventoryDetailText(saved);
    if (field.type === "identity-multiple" && field.identityKind) {
      value = <IdentitySelection kind={field.identityKind} label={label} value={Array.isArray(saved) ? saved.filter((id): id is string => typeof id === "string") : []} />;
    } else if (field.type === "related") {
      const id = field.idKey ? resource[field.idKey] ?? raw[idKey!] : undefined;
      const href = typeof id === "string" ? inventoryResourceDetailHref(field.relatedTab, id) : null;
      const identity = formatInventoryValue(saved).isEmpty ? id : saved;
      value = href ? <Link to={href} className="break-words text-primary underline">{inventoryDetailText(identity)}</Link> : inventoryDetailText(identity);
    } else if (field.type === "objectRef") {
      const ref = record(saved);
      const id = ref?.objectId ?? ref?.object_id;
      const token = ref?.objectType ?? ref?.object_type;
      const related = field.objectTypes?.find(option => option.token === token)?.relatedTab;
      const href = typeof id === "string" ? inventoryResourceDetailHref(related, id) : null;
      const identity = ref ? `${formatInventoryValue(ref.label ?? id).value || "—"} (${typeof token === "string" ? token : "—"})` : "—";
      value = href ? <Link to={href} className="break-words text-primary underline">{identity}</Link> : identity;
    } else if (field.type === "related-multiple" && Array.isArray(saved)) {
      value = <span className="flex flex-wrap gap-2">{saved.map((item, index) => {
        const ref = record(item);
        const id: unknown = ref?.id ?? item;
        const href = typeof id === "string" ? inventoryResourceDetailHref(field.relatedTab, id) : null;
        return href ? <Link key={index} to={href} className="break-words text-primary underline">{inventoryDetailText(ref?.name ?? id)}</Link> : <span key={index}>{inventoryDetailText(item)}</span>;
      })}</span>;
    }
    fields.push({ label, value });
    consumed.add(key);
    consumed.add(field.key);
    if (idKey) consumed.add(idKey);
    if (field.idKey) consumed.add(field.idKey);
  }
  // Response-only schema, zero/false, unresolved IDs and long values are retained.
  for (const [key, value] of Object.entries(raw)) {
    if (!consumed.has(key)) fields.push({ label: humanizeKey(key), value: inventoryDetailText(value) });
  }
  return fields;
}
