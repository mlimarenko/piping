import { Shield, FileCheck, Users, Lock } from "lucide-react";
import { useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import PageChrome from "@/shared/ui-shared/PageChrome";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import AccessRules from "@/features/access-control/components/access/AccessRules";
import AccessDirectory from "@/features/access-control/components/access/AccessDirectory";
import AccessSSO from "@/features/access-control/components/access/AccessSSO";
import { AccessDataProvider } from "@/features/access-control/components/access/AccessDataContext";
import { usePermissions } from "@/shared/auth/permissionsContext";

type TopTab = "access" | "directory" | "sso";
type Section = "users" | "groups" | "roles" | "grants";

const SECTIONS: readonly Section[] = ["users", "groups", "roles", "grants"];

interface Resolved {
  tab: TopTab;
  initialSection: Section;
}

/**
 * Resolve the surface from the canonical query params — exactly the values the
 * app itself produces (sidebar links, Roles → "View rules"):
 *   ?tab=access|directory|sso, ?section=<directory section>.
 * Anything else falls back to the Access Rules tab.
 */
function resolveTab(rawTab: string | null, rawSection: string | null): Resolved {
  if (rawTab === "sso") {
    return { tab: "sso", initialSection: "users" };
  }
  if (rawTab === "directory") {
    const section = SECTIONS.find((s) => s === rawSection) ?? "users";
    return { tab: "directory", initialSection: section };
  }
  return { tab: "access", initialSection: "users" };
}

const AccessControlWorkspace = () => {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const [searchParams] = useSearchParams();
  const resolved = resolveTab(
    searchParams.get("tab"),
    searchParams.get("section"),
  );

  // Only surface tabs the caller can read (mirrors the sidebar gating), so a
  // partial-capability admin never lands on a tab that 403s on every fetch.
  const tabConfig = [
    { key: "access" as const, label: t("accessControl.tabs.access"), icon: FileCheck, visible: can("READ", "POLICY") },
    { key: "directory" as const, label: t("accessControl.tabs.directory"), icon: Users, visible: can("READ", "USER") || can("READ", "GROUP") || can("READ", "ROLE") },
    { key: "sso" as const, label: t("accessControl.tabs.sso"), icon: Lock, visible: can("READ", "KEYCLOAK") },
  ].filter((x) => x.visible);

  const activeTab = tabConfig.some((x) => x.key === resolved.tab)
    ? resolved.tab
    : (tabConfig[0]?.key ?? resolved.tab);

  return (
    <AccessDataProvider>
      <div className="flex min-w-0 flex-col gap-4">
        <PageChrome
          title={t("accessControl.title")}
          icon={Shield}
          description={t("accessControl.description")}
        />

        <div className="flex min-w-0 flex-col gap-4">
          <SectionLinks label={t("accessControl.title")} activeValue={activeTab} items={tabConfig.map((x) => ({ value: x.key, label: x.label, icon: x.icon, to: { search: `?${new URLSearchParams(x.key === "directory" ? { tab: x.key, section: resolved.initialSection } : { tab: x.key })}` } }))} />

          {activeTab === "access" && <AccessRules />}
          {activeTab === "directory" && <AccessDirectory initialSection={resolved.initialSection} />}
          {activeTab === "sso" && <AccessSSO />}
        </div>
      </div>
    </AccessDataProvider>
  );
};

export default AccessControlWorkspace;
