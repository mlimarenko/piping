import { createContext, useContext } from "react";
import type { AccessAction, AccessResourceType, CurrentUser } from "@/features/auth/api";

/**
 * Client-side authorization, seeded from `/auth/me` (`CurrentUser.capabilities`
 * + `roles`). The single primitive the SPA uses to gate navigation, routes, and
 * controls so a user without a permission never sees a section or action that
 * will only 403 on the server. UX/clarity layer only — the server stays the
 * source of truth and re-checks every request.
 *
 * Capabilities are type-level: `can("UPDATE", "INVENTORY")` means "may update
 * some inventory object", not authority over a specific id. Per-object decisions
 * stay server-side.
 *
 * Non-component exports live here (separate from the `.tsx` components) so the
 * provider file satisfies `react-refresh/only-export-components`.
 */
export interface PermissionsValue {
  roles: string[];
  /** True for a full administrator (can manage policies — the strongest signal,
   *  independent of the seed role name). */
  isAdmin: boolean;
  can: (action: AccessAction, resourceType: AccessResourceType) => boolean;
  /** True if ANY of the actions is allowed on the resource type. */
  canAny: (resourceType: AccessResourceType, actions: AccessAction[]) => boolean;
  hasRole: (role: string) => boolean;
}

export const PermissionsContext = createContext<PermissionsValue | null>(null);

export function buildPermissions(user: CurrentUser | undefined): PermissionsValue {
  const roles = user?.roles ?? [];
  const grid = new Set((user?.capabilities ?? []).map((c) => `${c.action}:${c.resourceType}`));
  const can = (action: AccessAction, resourceType: AccessResourceType) =>
    grid.has(`${action}:${resourceType}`);
  return {
    roles,
    can,
    canAny: (resourceType, actions) => actions.some((a) => can(a, resourceType)),
    hasRole: (role) => roles.includes(role),
    // A real admin can manage the policy engine itself. Derive from capability,
    // not the role name, so renamed/custom admin roles still resolve correctly.
    isAdmin: can("MANAGE_POLICIES", "POLICY"),
  };
}

export const usePermissions = (): PermissionsValue => {
  const ctx = useContext(PermissionsContext);
  if (!ctx) throw new Error("usePermissions must be used within a PermissionsProvider");
  return ctx;
};
