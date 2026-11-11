import { useState } from "react";
import { useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { Activity, Radio, Radar, Rocket, Settings, Telescope } from "lucide-react";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { RequireCapability } from "@/shared/auth/permissions";
import { Button } from "@/shared/ui-primitives/button";
import PageChrome from "@/shared/ui-shared/PageChrome";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import CoverageScreen from "./CoverageScreen";
import ServiceDiscoveryScreen from "./ServiceDiscoveryScreen";
import InspectorMetricsScreen from "./InspectorMetricsScreen";
import QuickStartWizard from "./QuickStartWizard";
import SdPreviewPanel from "./SdPreviewPanel";
import SdLabelSettings from "./SdLabelSettings";
import ProfilesScreen from "./ProfilesScreen";

export default function ObservabilityPage() {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const [params, setParams] = useSearchParams();
  const tab = params.get("tab");
  const active = tab === "profiles" || tab === "collector" || tab === "coverage" || tab === "discovery-settings" || tab === "inspector-metrics" ? tab : "assignments";
  const [quickStartOpen, setQuickStartOpen] = useState(false);
  return <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-4">
    <RequireCapability check={p => p.can("READ", "MONITORING")}>
      <PageChrome icon={Telescope} title={t("observability.title")} description={t("observability.subtitle")}
        actions={active === "inspector-metrics" && can("CREATE", "MONITORING") ? <Button onClick={() => setQuickStartOpen(true)}><Rocket className="size-4" />{t("observability.quickStart.title")}</Button> : undefined} />
      <SectionLinks activeValue={active} label={t("observability.title")} items={[
        { value: "assignments", to: "/observability", label: t("observability.workflow.collection", { defaultValue: "Metric collection" }), icon: Radio },
        { value: "profiles", to: "/observability?tab=profiles", label: t("observability.profiles.title"), icon: Radio },
        { value: "coverage", to: "/observability?tab=coverage", label: t("observability.workflow.coverage", { defaultValue: "Coverage" }), icon: Radar },
        { value: "collector", to: "/observability?tab=collector", label: t("observability.workflow.collector", { defaultValue: "Connect collector" }), icon: Radio },
        { value: "discovery-settings", to: "/observability?tab=discovery-settings", label: t("observability.workflow.settings", { defaultValue: "Discovery settings" }), icon: Settings },
        { value: "inspector-metrics", to: "/observability?tab=inspector-metrics", label: t("observability.workflow.inspector", { defaultValue: "Inspector metrics" }), icon: Activity },
      ]} />
      {active === "profiles" ? <ProfilesScreen /> : active === "assignments" ? <ServiceDiscoveryScreen /> : active === "coverage" ? <CoverageScreen /> : active === "collector" ? <div className="flex min-w-0 flex-col gap-4"><p className="text-sm text-muted-foreground">{t("observability.workflow.collectorHint", { defaultValue: "Add this endpoint to your collector's http_sd_configs. Load a sample to inspect discovery output, not scrape health." })}</p><SdPreviewPanel /></div> : active === "discovery-settings" ? <SdLabelSettings /> : <InspectorMetricsScreen />}
      {active === "inspector-metrics" && <QuickStartWizard open={quickStartOpen} onOpenChange={setQuickStartOpen} onCompleted={() => setParams({ tab: "inspector-metrics" })} />}
    </RequireCapability>
  </div>;
}
