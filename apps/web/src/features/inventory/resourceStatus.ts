/**
 * User-facing labels and visual tones for inventory resource statuses. These
 * are intentionally separate from session/user/job statuses because the same
 * enum value needs inventory-specific wording in cards, inspectors, filters,
 * and playbook target selectors.
 *
 * This file is the single owner of the `ResourceStatus` → tone/variant/label
 * mapping. `inventoryResourceStatusTone` feeds `StatusChip` (resource list),
 * `inventoryResourceStatusBadgeVariant` feeds `Badge` (map inspector), and
 * `inventoryResourceStatusLabel` feeds every surface that prints the status.
 */
import type { BadgeProps } from "@/shared/ui-primitives/badge";
import type { ResourceStatus } from "@/shared/api/types";

type Translate = (key: string, options?: Record<string, unknown>) => string;

export function inventoryResourceStatusLabel(
  t: Translate,
  status?: string | null,
): string {
  if (!status) return "";
  return t(`inventory.resources.status.${status}`, {
    defaultValue: t(`statuses.${status}`, { defaultValue: status }),
  });
}

/** StatusChip tone vocabulary, matching `StatusChipProps["status"]`. */
export type InventoryStatusTone = "info" | "positive" | "warning" | "negative" | "neutral";

/**
 * `ResourceStatus` → `StatusChip` tone. The extra `failed`/`reserved`/`available`
 * branches are defensive: `Resource.status` is typed to the five-value
 * `ResourceStatus` union, so they are unreachable for the resource list today.
 */
export function inventoryResourceStatusTone(status?: string | null): InventoryStatusTone {
  if (status === "active") return "positive";
  if (status === "planned" || status === "staged") return "info";
  if (status === "decommissioning" || status === "failed" || status === "offline") return "negative";
  if (status === "reserved" || status === "available") return "neutral";
  return "warning";
}

/**
 * `ResourceStatus` → `Badge` variant for the map inspector. Exhaustive over the
 * `ResourceStatus` union; callers guard on a truthy status before rendering.
 */
const RESOURCE_STATUS_BADGE_VARIANTS: Record<ResourceStatus, BadgeProps["variant"]> = {
  active: "status-active",
  planned: "status-planned",
  staged: "status-staging",
  decommissioning: "status-decommissioning",
  offline: "status-offline",
};

export function inventoryResourceStatusBadgeVariant(
  status: ResourceStatus,
): BadgeProps["variant"] {
  return RESOURCE_STATUS_BADGE_VARIANTS[status];
}
