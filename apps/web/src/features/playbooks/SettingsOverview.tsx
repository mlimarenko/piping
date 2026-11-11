import { useTranslation } from "react-i18next";
import { Link } from "react-router";
import SurfaceCard, { SurfaceCardContent, SurfaceCardHeader } from "@/shared/ui-shared/SurfaceCard";
import { Button } from "@/shared/ui-primitives/button";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type { AccessResourceType } from "@/features/auth/api";
import { PLAYBOOK_TAB_ROUTES, type PlaybookSettingsTab } from "./playbookNavigation";

/** One navigation entry per settings surface; this page owns no lists and no counters. */
const SURFACES: ReadonlyArray<{
  tab: PlaybookSettingsTab;
  resource: AccessResourceType;
  titleKey: string;
  purposeKey: string;
}> = [
  { tab: "execution-environments", resource: "PLAYBOOK_EXECUTION_ENVIRONMENT", titleKey: "playbooks.page.executionEnvironments.title", purposeKey: "playbooks.page.settingsOverview.surfaces.executionEnvironments" },
  { tab: "target-sets", resource: "PLAYBOOK_TARGET_SET", titleKey: "playbooks.page.targetSets.title", purposeKey: "playbooks.page.settingsOverview.surfaces.targetSets" },
  { tab: "assignments", resource: "PLAYBOOK_ASSIGNMENT", titleKey: "playbooks.page.assignments.title", purposeKey: "playbooks.page.settingsOverview.surfaces.assignments" },
];

export function SettingsOverview() {
  const { t } = useTranslation();
  const { can } = usePermissions();
  return <section aria-label={t("playbooks.page.settings")} className="grid min-w-0 gap-4 md:grid-cols-2">
    {SURFACES.map(surface => {
      const title = t(surface.titleKey);
      return <SurfaceCard key={surface.tab}>
        <SurfaceCardHeader title={title} description={t(surface.purposeKey)} />
        <SurfaceCardContent>
          {can("READ", surface.resource)
            ? <Button asChild variant="outline" size="sm"><Link to={PLAYBOOK_TAB_ROUTES[surface.tab]} aria-label={t("playbooks.page.settingsOverview.openSurface", { name: title })}>{t("playbooks.page.settingsOverview.open")}</Link></Button>
            : <p className="text-sm text-muted-foreground">{t("playbooks.page.settingsOverview.noAccess")}</p>}
        </SurfaceCardContent>
      </SurfaceCard>;
    })}
  </section>;
}
