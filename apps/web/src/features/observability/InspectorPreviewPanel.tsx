import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Eye } from "lucide-react";

import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import EmptyState from "@/shared/ui-shared/EmptyState";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FormField from "@/shared/ui-shared/FormField";
import { CONNECTION_TARGETS_QUERY_KEY, listConnectionTargets } from "@/features/connections/api";
import HostMetricsPreviewSection from "./HostMetricsPreviewSection";

/**
 * Right-hand preview for Screen ②: pick a connection target and see the latest
 * inspector values that the current sources + metric-set assignments resolve to.
 */
const InspectorPreviewPanel = () => {
  const { t } = useTranslation();
  const [hostId, setHostId] = useState("");

  const hostsQuery = useQuery({
    queryKey: CONNECTION_TARGETS_QUERY_KEY,
    queryFn: () => listConnectionTargets(),
    staleTime: 60_000,
  });
  const hosts = useMemo(() => hostsQuery.data?.items ?? [], [hostsQuery.data]);
  const selectedHost = hosts.find((h) => h.id === hostId);

  return (
    <SurfaceCard className="flex min-h-0 flex-col p-4">
      <div className="mb-3">
        <h3 className="text-sm font-semibold text-foreground">
          {t("observability.inspector.previewTitle", { defaultValue: "Inspector preview" })}
        </h3>
        <p className="text-xs text-muted-foreground">
          {t("observability.inspector.previewHint", {
            defaultValue: "Latest values for a connection target using the current configuration.",
          })}
        </p>
      </div>

      <FormField htmlFor="inspector-preview-target" label={t("observability.scope.targetLabel", { defaultValue: "Target" })} className="mb-4">
        <EntitySelect
          id="inspector-preview-target"
          value={hostId}
          onValueChange={setHostId}
          placeholder={
            hostsQuery.isPending
              ? t("loading", { defaultValue: "Loading…" })
              : t("observability.inspector.pickHost", { defaultValue: "Pick a connection target…" })
          }
          items={hosts.map((host) => ({ id: host.id, label: host.name }))}
        />
      </FormField>

      <div className="min-h-0 flex-1 overflow-auto">
        {selectedHost ? (
          <HostMetricsPreviewSection host={{ id: selectedHost.id, name: selectedHost.name }} />
        ) : (
          <EmptyState
            icon={Eye}
            title={t("observability.inspector.noHost", { defaultValue: "No connection target selected" })}
            description={t("observability.inspector.noHostHint", {
              defaultValue: "Choose a connection target to preview its inspector metrics.",
            })}
          />
        )}
      </div>
    </SurfaceCard>
  );
};

export default InspectorPreviewPanel;
