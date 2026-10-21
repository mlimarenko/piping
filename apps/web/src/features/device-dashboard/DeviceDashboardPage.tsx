import { useQuery } from "@tanstack/react-query";
import { useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { MonitorCog } from "lucide-react";

import { CctvConsole } from "@/features/device-dashboard/CctvConsole";
import { connectionFailureFromApiError } from "@/features/connections/connectionFailure";
import {
  CONNECTION_TARGETS_QUERY_KEY,
  getConnectionTarget,
} from "@/features/connections/api";
import ConnectionFailureOverlay from "@/features/connections/components/connections/ConnectionFailureOverlay";
import { retryOn5xx } from "@/shared/api/hooks";
import { Skeleton } from "@/shared/ui-primitives/skeleton";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageLayout from "@/shared/ui-shared/PageLayout";

const DeviceDashboardSkeleton = () => (
  <div className="flex h-full min-h-0 flex-col gap-4" aria-busy="true">
    <div className="flex items-center gap-3">
      <Skeleton className="h-9 w-9 shrink-0" />
      <div className="space-y-2">
        <Skeleton className="h-5 w-48" />
        <Skeleton className="h-3 w-72" />
      </div>
    </div>
    <Skeleton className="min-h-0 flex-1" />
  </div>
);

/**
 * Standalone route (`/connections/devices/:targetId`) wrapper around the
 * {@link CctvConsole} device mini-admin. It serves both a CCTV recorder (opened
 * from the `CCTV` connect button) and a Linux host (opened from the neutral
 * `Accounts` button); the capabilities response decides which tabs appear. The
 * primary way to reach the console is the connections workspace connect panel,
 * which opens it as a session tab; this route stays for deep-links and direct
 * navigation.
 */
const DeviceDashboardPage = () => {
  const { t } = useTranslation();
  const { targetId } = useParams<{ targetId: string }>();
  const targetQuery = useQuery({
    queryKey: [...CONNECTION_TARGETS_QUERY_KEY, targetId],
    queryFn: () => getConnectionTarget(targetId ?? ""),
    enabled: Boolean(targetId),
    retry: retryOn5xx,
  });

  if (targetQuery.isPending) {
    return <PageLayout width="full" fill><DeviceDashboardSkeleton /></PageLayout>;
  }

  if (targetQuery.isError || !targetQuery.data) {
    return (
      <PageLayout width="full" fill className="flex flex-col gap-4">
        <PageChrome
          title={t("deviceDashboard.title", "Device dashboard")}
          icon={MonitorCog}
          description={t(
            "deviceDashboard.description",
            "Management view of the selected device.",
          )}
        />
        <div className="relative min-h-48 flex-1">
          <ConnectionFailureOverlay
            failure={connectionFailureFromApiError(targetQuery.error)}
            onRetry={() => {
              targetQuery.refetch().catch(() => {
                // The overlay stays and the button can be pressed again; a
                // rejected refetch needs no separate report.
              });
            }}
          />
        </div>
      </PageLayout>
    );
  }

  const target = targetQuery.data;
  const targetKind =
    target.targetKind === "device"
      ? t("connections.targetKind.device", "Device")
      : t("connections.targetKind.virtualMachine", "Virtual machine");
  const targetAddress = target.primaryIp ?? "—";
  const targetFamily = target.family ?? "—";

  return (
    <PageLayout width="full" fill className="flex flex-col gap-4">
      <PageChrome
        title={target.name}
        icon={MonitorCog}
        description={`${targetAddress} · ${targetKind}`}
        badge={targetFamily}
      />
      <CctvConsole targetId={targetId ?? ""} className="min-h-0 flex-1" />
    </PageLayout>
  );
};

export default DeviceDashboardPage;
