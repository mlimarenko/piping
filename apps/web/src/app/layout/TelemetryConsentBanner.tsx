import { useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { BarChart3 } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import {
  setTelemetryConsent,
  subscribeTelemetryConsent,
  telemetryState,
} from "@/shared/lib/telemetry/consent";
import { startTelemetryIfAllowed } from "@/shared/lib/telemetry";

/**
 * First-run consent for non-essential usage data.
 *
 * Non-modal on purpose: it does not take focus, does not block the workspace and
 * stays readable at a constrained width. It disappears after an explicit choice;
 * Settings owns later changes.
 */
const TelemetryConsentBanner = () => {
  const { t } = useTranslation();
  const consent = useSyncExternalStore(
    subscribeTelemetryConsent,
    telemetryState,
    () => "pending" as const,
  );

  if (consent !== "unset") return null;

  const allow = () => {
    setTelemetryConsent("granted");
    startTelemetryIfAllowed();
  };
  const decline = () => setTelemetryConsent("denied");

  return (
    <section
      role="region"
      aria-label={t("telemetry.bannerTitle")}
      className="fixed inset-x-4 bottom-4 z-50 rounded-lg border border-border bg-card p-4 text-card-foreground shadow-lg sm:inset-x-auto sm:right-4 sm:w-96"
    >
      <div className="flex items-start gap-3">
        <BarChart3 className="mt-0.5 size-5 shrink-0 text-primary" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold">{t("telemetry.bannerTitle")}</h2>
          <p className="mt-1 text-sm text-muted-foreground">{t("telemetry.bannerBody")}</p>
          <div className="mt-3 flex flex-wrap gap-2">
            <Button size="sm" onClick={allow}>
              {t("telemetry.allow")}
            </Button>
            <Button size="sm" variant="outline" onClick={decline}>
              {t("telemetry.decline")}
            </Button>
          </div>
        </div>
      </div>
    </section>
  );
};

export default TelemetryConsentBanner;
