import type { ReactNode } from "react";
import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";

/**
 * Allow-all permissions for component tests.
 *
 * Components gated with `usePermissions()` throw outside a `PermissionsProvider`.
 * Tests that exercise behaviour unrelated to RBAC wrap their render tree in
 * {@link AllPermissions} so every control is visible (matching the pre-gating
 * behaviour the existing assertions were written against). For tests that
 * specifically assert denial, build a narrower value instead.
 */
const ALLOW_ALL_PERMISSIONS: PermissionsValue = {
  roles: ["admin"],
  isAdmin: true,
  can: () => true,
  canAny: () => true,
  hasRole: () => true,
};

export const AllPermissions = ({ children }: { children: ReactNode }) => (
  <PermissionsContext.Provider value={ALLOW_ALL_PERMISSIONS}>{children}</PermissionsContext.Provider>
);
