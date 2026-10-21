import { ApiError } from "@/shared/api/client";
import type { InventoryEntityKey } from "@/shared/api/types";
import { isInventoryEntityKey } from "@/features/inventory/resourceLinks";

export const DELETE_BLOCKED_REASON = "DELETE_BLOCKED";

export interface DeleteBlockedExample {
  id?: string;
  label: string;
}

export interface DeleteBlockedGroup {
  table: string;
  entityKey?: InventoryEntityKey;
  count: number;
  examples: DeleteBlockedExample[];
}

export interface DeleteBlockedDetails {
  totalCount: number;
  blockers: DeleteBlockedGroup[];
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function parseExample(value: unknown): DeleteBlockedExample | null {
  const record = asRecord(value);
  if (!record || typeof record.label !== "string") return null;
  return {
    id: typeof record.id === "string" ? record.id : undefined,
    label: record.label,
  };
}

function parseGroup(value: unknown): DeleteBlockedGroup | null {
  const record = asRecord(value);
  if (!record || typeof record.table !== "string" || typeof record.count !== "number") {
    return null;
  }
  const rawEntityKey = record.entity_key ?? record.entityKey;
  const entityKey =
    typeof rawEntityKey === "string" && isInventoryEntityKey(rawEntityKey)
      ? rawEntityKey
      : undefined;
  const rawExamples = Array.isArray(record.examples) ? record.examples : [];
  const examples = rawExamples
    .map(parseExample)
    .filter((example): example is DeleteBlockedExample => example !== null);
  return {
    table: record.table,
    entityKey,
    count: record.count,
    examples,
  };
}

export function parseDeleteBlockedDetails(error: unknown): DeleteBlockedDetails | null {
  if (!(error instanceof ApiError)) return null;
  if (error.reasonCode !== DELETE_BLOCKED_REASON && error.code !== "DELETE_BLOCKED") {
    return null;
  }
  const details = asRecord(error.details);
  const totalCount = details?.total_count ?? details?.totalCount;
  if (!details || typeof totalCount !== "number" || !Array.isArray(details.blockers)) {
    return null;
  }
  const blockers = details.blockers
    .map(parseGroup)
    .filter((group): group is DeleteBlockedGroup => group !== null);
  return {
    totalCount,
    blockers,
  };
}

export function fallbackBlockerLabel(table: string): string {
  const stripped = table.replace(/^(dcim|ipam|tenancy|virtualization|circuits|vpn|inventory)_/, "");
  return stripped
    .split("_")
    .filter(Boolean)
    .map((segment) => segment.charAt(0).toUpperCase() + segment.slice(1))
    .join(" ");
}
