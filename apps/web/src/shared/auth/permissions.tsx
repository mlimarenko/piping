import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { ShieldAlert } from "lucide-react";
import PageChrome from "@/shared/ui-shared/PageChrome";
import type { CurrentUser } from "@/features/auth/api";
import {
  PermissionsContext,
  buildPermissions,
  usePermissions,
  type PermissionsValue,
} from "./permissionsContext";

export const PermissionsProvider = ({
  user,
  children,
}: {
  user: CurrentUser | undefined;
  children: React.ReactNode;
}) => {
  const value = useMemo(() => buildPermissions(user), [user]);
  return <PermissionsContext.Provider value={value}>{children}</PermissionsContext.Provider>;
};

/** Inline "you don't have access" panel for guarded routes. */
export const AccessDenied = ({ visible = true }: { visible?: boolean }) => {
  const { t } = useTranslation();
  return (
    <>
    <PageChrome visible={visible} title={t("accessDenied.title", "You don't have access to this section")} icon={ShieldAlert} />
    <div className="flex min-h-[50vh] w-full flex-col items-center justify-center gap-3 text-center">
      <div className="surface rounded-xl p-4 shadow-md">
        <ShieldAlert className="h-8 w-8 text-muted-foreground" />
      </div>
      <p className="text-sm font-medium text-foreground">
        {t("accessDenied.title", "You don't have access to this section")}
      </p>
      <p className="mx-auto max-w-sm text-xs text-muted-foreground text-balance">
        {t(
          "accessDenied.body",
          "Your role doesn't grant the permissions required here. Contact an administrator if you need access.",
        )}
      </p>
    </div>
    </>
  );
};

/**
 * Route/section guard. Renders children only when `check(permissions)` holds,
 * otherwise an {@link AccessDenied} panel (no redirect — avoids loops and keeps
 * the shell/nav in place).
 */
export const RequireCapability = ({
  check,
  children,
  visible = true,
}: {
  check: (permissions: PermissionsValue) => boolean;
  children: React.ReactNode;
  /** Retained off-route workspaces must not publish denial chrome. */
  visible?: boolean;
}) => {
  const permissions = usePermissions();
  return check(permissions) ? <>{children}</> : <AccessDenied visible={visible} />;
};
