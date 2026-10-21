import type { PlaybookTargetSet } from "@/features/playbooks/api";

export function targetSetLabel(targetSet?: PlaybookTargetSet) {
  if (!targetSet) return "—";
  return `${targetSet.name} · ${targetSet.mode}`;
}

export function formatDateTime(value?: string | null) {
  if (!value) return "—";
  return new Date(value).toLocaleString();
}

export function asJsonObject(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return value as Record<string, unknown>;
}

export function jsonObjectSize(value: unknown): number {
  return Object.keys(asJsonObject(value)).length;
}
