import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";
import { Users, UserCheck, Key, Award } from "lucide-react";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
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
  const [searchParams] = useSearchParams();

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

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <SectionLinks
        label={t("accessControl.tabs.directory")}
        activeValue={section}
        items={items.map((x) => ({ value: x.value, label: x.label, icon: x.icon, to: { search: `?${new URLSearchParams({ tab: "directory", section: x.value })}` } }))}
      />
      {section === "users" && <AccessUsers />}
      {section === "groups" && <AccessGroups />}
      {section === "roles" && <AccessRoles />}
      {section === "grants" && <AccessGrants />}
    </div>
  );
};

export default AccessDirectory;
