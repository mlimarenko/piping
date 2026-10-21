import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";
import { Users, UserCheck, Key, Award } from "lucide-react";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import { usePermissions } from "@/shared/auth/permissionsContext";
import AccessUsers from "./AccessUsers";
import AccessGroups from "./AccessGroups";
import AccessRoles from "./AccessRoles";
import AccessGrants from "./AccessGrants";

type Section = "users" | "groups" | "roles" | "grants";
const SECTIONS: readonly Section[] = ["users", "groups", "roles", "grants"];

interface AccessDirectoryProps {
  /** Section resolved from a legacy ?tab=roles/groups/grants by the workspace;
      used as a fallback when the URL carries no explicit ?section=. */
  initialSection?: Section;
}

/**
 * The "Directory" tab: identity vocabulary (Users, Groups), reusable bundle
 * names (Roles), and role activation (Grants) — separated from the rules so the
 * Access Rules view stays calm. Renders the existing components verbatim (same
 * tableIds, CRUD, capability gating). The active section is derived from the URL
 * (?section=) so deep-links, the sidebar, and the back button stay in sync;
 * sections the caller can't read are hidden (mirrors the workspace tab gating).
 */
const AccessDirectory = ({ initialSection = "users" }: AccessDirectoryProps) => {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const [searchParams, setSearchParams] = useSearchParams();

  const items = [
    { value: "users" as const, label: t("accessControl.tabs.users"), icon: Users, visible: can("READ", "USER") },
    { value: "groups" as const, label: t("accessControl.tabs.groups"), icon: UserCheck, visible: can("READ", "GROUP") },
    { value: "roles" as const, label: t("accessControl.tabs.roles"), icon: Key, visible: can("READ", "ROLE") },
    { value: "grants" as const, label: t("accessControl.tabs.grants"), icon: Award, visible: can("READ", "ROLE") },
  ].filter((x) => x.visible);

  // URL is the source of truth; fall back to the workspace-resolved legacy
  // section, then the first section the caller can actually read.
  const rawSection = searchParams.get("section");
  const requested: Section = SECTIONS.includes(rawSection as Section) ? (rawSection as Section) : initialSection;
  const section: Section = items.some((x) => x.value === requested) ? requested : (items[0]?.value ?? "users");

  const handleChange = (value: string) => {
    const next: Section = SECTIONS.includes(value as Section) ? (value as Section) : "users";
    setSearchParams({ tab: "directory", section: next });
  };

  return (
    <Tabs value={section} onValueChange={handleChange} className="flex min-w-0 flex-col gap-4">
      {/* Lighter, secondary strip so the inner level reads as nested, not a peer
          of the top Access Rules / Directory / SSO tabs. */}
      <SectionTabs
        items={items.map((x) => ({ value: x.value, label: x.label, icon: x.icon }))}
        variant="secondary"
      />
      <TabsContent value="users" className="mt-0"><AccessUsers /></TabsContent>
      <TabsContent value="groups" className="mt-0"><AccessGroups /></TabsContent>
      <TabsContent value="roles" className="mt-0"><AccessRoles /></TabsContent>
      <TabsContent value="grants" className="mt-0"><AccessGrants /></TabsContent>
    </Tabs>
  );
};

export default AccessDirectory;
