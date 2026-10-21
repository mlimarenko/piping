import { useState } from "react";
import { useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { Activity, Radio, Rocket, Settings, Telescope } from "lucide-react";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { RequireCapability } from "@/shared/auth/permissions";
import { Button } from "@/shared/ui-primitives/button";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageLayout from "@/shared/ui-shared/PageLayout";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import ServiceDiscoveryScreen from "./ServiceDiscoveryScreen";
import InspectorMetricsScreen from "./InspectorMetricsScreen";
import QuickStartWizard from "./QuickStartWizard";
import SdPreviewPanel from "./SdPreviewPanel";
import SdLabelSettings from "./SdLabelSettings";

export default function ObservabilityPage() {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const [params, setParams] = useSearchParams();
  const tab = params.get("tab");
  const active = tab === "collector" || tab === "discovery-settings" || tab === "inspector-metrics" ? tab : "assignments";
  const [quickStartOpen, setQuickStartOpen] = useState(false);
  return <PageLayout width={!can("READ", "MONITORING") || active === "collector" || active === "discovery-settings" ? "standard" : "full"} fill className="flex min-w-0 flex-col gap-4">
    <RequireCapability check={p => p.can("READ", "MONITORING")}>
      <PageChrome icon={Telescope} title={t("observability.title")} description={t("observability.subtitle")}
        actions={active === "inspector-metrics" && can("CREATE", "MONITORING") ? <Button onClick={() => setQuickStartOpen(true)}><Rocket className="size-4" />{t("observability.quickStart.title")}</Button> : undefined} />
      <SectionLinks wrap activeValue={active} label={t("observability.title")} items={[
        { value: "assignments", to: "/observability", label: t("observability.workflow.collection", { defaultValue: "Metric collection" }), icon: Radio },
        { value: "collector", to: "/observability?tab=collector", label: t("observability.workflow.collector", { defaultValue: "Connect collector" }), icon: Radio },
        { value: "discovery-settings", to: "/observability?tab=discovery-settings", label: t("observability.workflow.settings", { defaultValue: "Discovery settings" }), icon: Settings },
        { value: "inspector-metrics", to: "/observability?tab=inspector-metrics", label: t("observability.workflow.inspector", { defaultValue: "Inspector metrics" }), icon: Activity },
      ]} />
      {active === "assignments" ? <ServiceDiscoveryScreen /> : active === "collector" ? <div className="flex min-w-0 flex-col gap-4"><p className="text-sm text-muted-foreground">{t("observability.workflow.collectorHint", { defaultValue: "Add this endpoint to your collector's http_sd_configs. Load a sample to inspect discovery output, not scrape health." })}</p><SdPreviewPanel /></div> : active === "discovery-settings" ? <SdLabelSettings /> : <InspectorMetricsScreen />}
      {active === "inspector-metrics" && <QuickStartWizard open={quickStartOpen} onOpenChange={setQuickStartOpen} onCompleted={() => setParams({ tab: "inspector-metrics" })} />}
    </RequireCapability>
  </PageLayout>;
}
