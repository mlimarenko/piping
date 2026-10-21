import type { ResourceStatus } from "@/shared/api/types";

export const UNAVAILABLE = "—";

export const hostStatusMap: Record<
  ResourceStatus,
  "positive" | "info" | "warning" | "negative" | "neutral"
> = {
  active: "positive",
  planned: "info",
  staged: "warning",
  decommissioning: "negative",
  offline: "neutral",
};

export function hostValue(value?: string | null): string {
  const trimmed = value?.trim();
  return trimmed ? trimmed : UNAVAILABLE;
}

export function hostSubtitle(parts: Array<string | null | undefined>): string {
  const present = parts.map((part) => part?.trim()).filter(Boolean) as string[];
  return present.length > 0 ? present.join(" · ") : UNAVAILABLE;
}
