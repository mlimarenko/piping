import { ApiError } from "@/shared/api/client";
import type { InventoryEntityKey } from "@/shared/api/types";

type Translate = (key: string, opts?: Record<string, unknown>) => string;

/** Backend reason codes with their own explanation; everything else is mapped by status. */
const REASON_KEYS: Record<string, string> = {
  MODULE_BAY_INCOMPATIBLE: "inventory.resources.saveErrors.moduleBayIncompatible",
  MODULE_BAY_OCCUPIED: "inventory.resources.saveErrors.moduleBayOccupied",
  MODULE_BAY_DISABLED: "inventory.resources.saveErrors.moduleBayDisabled",
};

function keyForStatus(status: number): string {
  if (status === 409) return "inventory.resources.saveErrors.conflict";
  if (status === 400 || status === 422) return "inventory.resources.saveErrors.validation";
  if (status === 403) return "inventory.resources.saveErrors.forbidden";
  if (status === 404) return "inventory.resources.saveErrors.notFound";
  return "unknownError";
}

/**
 * The one inventory write-failure message. Backend messages can carry SQL or
 * internal identifiers, so an `ApiError` is shown only as localized copy chosen
 * from its reason code or HTTP status. Local errors (validation thrown while
 * building a payload) already carry their own text.
 */
export function inventoryErrorMessage(t: Translate, error: unknown, entity?: InventoryEntityKey): string {
  if (error instanceof ApiError) {
    if (entity?.startsWith("Cooling ") && (error.status === 400 || error.status === 422)) {
      return t("inventory.resources.saveErrors.coolingValidation");
    }
    const reason = (error.reasonCode ?? error.code ?? "").toUpperCase();
    return t(REASON_KEYS[reason] ?? keyForStatus(error.status));
  }
  return error instanceof Error ? error.message : t("unknownError");
}
