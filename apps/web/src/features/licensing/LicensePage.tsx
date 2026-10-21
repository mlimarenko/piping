import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageLayout from "@/shared/ui-shared/PageLayout";
import SurfaceCard, { SurfaceCardHeader, SurfaceCardContent } from "@/shared/ui-shared/SurfaceCard";
import FormSection from "@/shared/ui-shared/FormSection";
import StatusChip from "@/shared/ui-shared/StatusChip";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Button } from "@/shared/ui-primitives/button";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/shared/ui-primitives/collapsible";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import { RefreshCw, Shield, Zap, Loader2, AlertTriangle } from "lucide-react";
import { Skeleton } from "@/shared/ui-primitives/skeleton";
import { activateLicense, getLicense, getLicenseHealth, refreshLicense } from "@/features/licensing/api";
import type { ApiError } from "@/shared/api/client";
import type { LicenseInfo } from "@/shared/api/types";
import { notify } from "@/shared/lib/notifications";

const LICENSE_DATE_FMT = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

function formatLicenseDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  try {
    const d = new Date(iso);
    if (!isFinite(d.getTime())) return "—";
    return LICENSE_DATE_FMT.format(d);
  } catch {
    return "—";
  }
}

const STATUS_STYLES: Record<LicenseInfo["status"], "positive" | "info" | "warning" | "negative" | "neutral"> = {
  active: "positive",
  expired: "negative",
  grace: "warning",
  inactive: "neutral",
  trial: "info",
  invalid: "negative",
  missing: "neutral",
  suspended: "negative",
};

const LicensePage = () => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [activationToken, setActivationToken] = useState("");

  // View-but-not-edit: a READ-only role sees license state but no activate/refresh
  // affordance — both are UPDATE mutations on LICENSE.
  const { can } = usePermissions();
  const canUpdate = can("UPDATE", "LICENSE");

  const licenseQuery = useQuery({
    queryKey: ["license", "state"],
    queryFn: getLicense,
  });
  const healthQuery = useQuery({
    queryKey: ["license", "health"],
    queryFn: getLicenseHealth,
  });

  const activateMutation = useApiMutation({
    mutationFn: activateLicense,
    onSuccess: () => {
      setActivationToken("");
      void qc.invalidateQueries({ queryKey: ["license"] });
      notify("success", t("licensePage.licenseActivated"));
    },
    onError: (err: ApiError) => {
      notify("error", t("licensePage.activationFailed"), { description: err.message });
    },
  });

  const refreshMutation = useApiMutation({
    mutationFn: refreshLicense,
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["license"] });
      notify("success", t("licensePage.licenseRefreshed"));
    },
    onError: (err: ApiError) => {
      void qc.invalidateQueries({ queryKey: ["license"] });
      notify("error", t("licensePage.refreshFailed"), { description: err.message });
    },
  });

  const info = licenseQuery.data;
  const status = info?.status;
  const hasKnownState = Boolean(status && status !== "missing" && Object.hasOwn(STATUS_STYLES, status));
  const isCommunity = hasKnownState && status === "inactive";
  const statusStyle = status ? STATUS_STYLES[status] ?? "neutral" : "neutral";
  const hasEntitlement = hasKnownState && !isCommunity;
  const seatsValue = info ? `${info.seats.used} / ${info.seats.total}` : "—";

  return (
    <PageLayout width="form" className="flex flex-col gap-6">
      <PageChrome
        title={t("licensePage.title")}
        description={t("licensePage.description")}
      />

       <SurfaceCard className="p-4 md:p-6">
        <div className="flex flex-col gap-6">
         <FormSection>
           <SurfaceCardHeader title={<span className="flex items-center gap-2"><Shield className="size-4 text-primary" aria-hidden="true" />{t("licensePage.status")}</span>} />
           <SurfaceCardContent className="flex flex-col gap-2.5">
            {licenseQuery.isPending ? (
              <div className="space-y-3">
                <div className="flex items-center gap-3">
                  <Skeleton className="w-10 h-10 rounded-lg" />
                  <div className="space-y-1.5">
                    <Skeleton className="h-4 w-32" />
                    <Skeleton className="h-3 w-20" />
                   </div>
                </div>
                {[0, 1, 2, 3, 4].map(i => <Skeleton key={i} className="h-4 w-full" />)}
              </div>
            ) : licenseQuery.error ? (
              <div role="alert">
                <EmptyState
                  icon={AlertTriangle}
                  title={t("licensePage.loadFailed")}
                  actionLabel={t("retry")}
                  actionIcon={RefreshCw}
                  onAction={() => { void licenseQuery.refetch(); }}
                />
              </div>
             ) : !hasKnownState ? (
               <div role="alert">
                 <EmptyState icon={Shield} title={t("licensePage.noLicenseData")} />
               </div>
             ) : (
              <>
                <div className="flex items-center gap-3 mb-3">
                  <div className="w-10 h-10 rounded-lg flex items-center justify-center bg-muted">
                      <Zap className={`h-5 w-5 ${status === "active" ? "text-status-positive" : "text-muted-foreground"}`} />
                  </div>
                  <div>
                    <p className="text-sm font-semibold text-foreground">
                      {hasEntitlement
                        ? t("licensePage.licenseLabel", { status: t(`licensePage.statuses.${status}`, status ?? "") })
                        : t("licensePage.freeLicenseLabel")}
                    </p>
                    <p className="text-xs text-muted-foreground">
                      {status === "suspended" ? t("licensePage.suspendedHint") : hasEntitlement ? t("licensePage.activeLicenseHint") : t("licensePage.freeLicenseHint")}
                    </p>
                  </div>
                </div>
                {status === "suspended" && (
                  <p role="alert" className="text-sm text-destructive">
                    {t(`licensePage.issuerReasons.${healthQuery.data?.reasonCode}`, { defaultValue: t("licensePage.issuerDenied") })}
                  </p>
                )}
                <InfoRow
                  label={t("licensePage.entitlement")}
                  value={<StatusChip status={isCommunity ? "positive" : statusStyle} label={isCommunity ? t("licensePage.communityAvailable") : t(`licensePage.statuses.${status}`, status ?? "")} />}
                />
                <InfoRow
                  label={hasEntitlement ? t("licensePage.seats") : t("licensePage.freeSeats")}
                  value={seatsValue}
                />
                {hasEntitlement && (
                  <>
                    <InfoRow label={t("licensePage.validFrom")} value={formatLicenseDate(info?.validFrom)} />
                    <InfoRow label={t("licensePage.validUntil")} value={formatLicenseDate(info?.validUntil)} />
                    <InfoRow label={t("licensePage.lastSyncError")} value={info?.lastSyncError || "—"} />
                  </>
                )}
                {hasEntitlement && canUpdate && (
                  <Button
                    variant="outline"
                    disabled={refreshMutation.isPending || activateMutation.isPending}
                    aria-busy={refreshMutation.isPending}
                    onClick={() => refreshMutation.mutate()}
                  >
                    {refreshMutation.isPending ? (
                      <Loader2 className="size-4 animate-spin" />
                    ) : (
                      <>
                        <RefreshCw className="size-4" />
                      </>
                    )}
                    {t("refresh")}
                  </Button>
                )}
              </>
             )}
           </SurfaceCardContent>
         </FormSection>

        {canUpdate && (
           <Collapsible>
             <CollapsibleTrigger asChild>
               <Button variant="outline">{t("licensePage.moreSeats")}</Button>
             </CollapsibleTrigger>
             <CollapsibleContent className="pt-4">
           <FormSection>
             <SurfaceCardHeader title={<span className="flex items-center gap-2"><Zap className="size-4 text-primary" aria-hidden="true" />{t("licensePage.activation")}</span>} />
             <SurfaceCardContent className="flex flex-col gap-3">
              <p className="text-sm leading-relaxed text-muted-foreground text-balance">
                {t("licensePage.activationHelp")}
              </p>
              <div>
                <FieldLabel htmlFor="license-activation-token" className="mb-2 block" required>
                  {t("licensePage.licenseKey")}
                </FieldLabel>
                <Textarea
                  id="license-activation-token"
                  disabled={activateMutation.isPending || refreshMutation.isPending}
                  autoComplete="off"
                  spellCheck={false}
                  className="min-h-24 resize-y font-mono"
                  placeholder={t("licensePage.enterToken")}
                  value={activationToken}
                  onChange={(e) => setActivationToken(e.target.value)}
                />
               </div>
              <div className="flex gap-2">
                <Button
                  disabled={!activationToken.trim() || activateMutation.isPending || refreshMutation.isPending}
                  aria-busy={activateMutation.isPending}
                  onClick={() =>
                    activateMutation.mutate({
                      activationToken: activationToken.trim(),
                    })
                  }
                >
                  {activateMutation.isPending && <Loader2 className="size-4 animate-spin" />}
                  {t("licensePage.activate")}
                </Button>
              </div>
             </SurfaceCardContent>
             </FormSection>
             </CollapsibleContent>
           </Collapsible>
        )}

       </div>
       </SurfaceCard>
    </PageLayout>
  );
};

const InfoRow = ({ label, value }: { label: string; value: React.ReactNode }) => (
  <div className="flex flex-wrap justify-between items-center gap-2 text-sm">
    <span className="text-muted-foreground">{label}</span>
    <span className="max-w-full break-words text-foreground">{value}</span>
  </div>
);

export default LicensePage;
