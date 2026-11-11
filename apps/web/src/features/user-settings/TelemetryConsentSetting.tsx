import { useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import SurfaceCard, {
  SurfaceCardContent,
  SurfaceCardHeader,
} from "@/shared/ui-shared/SurfaceCard";
import StatusChip from "@/shared/ui-shared/StatusChip";
import { Button } from "@/shared/ui-primitives/button";
import {
  setTelemetryConsent,
  subscribeTelemetryConsent,
  telemetryConsent,
  telemetryState,
} from "@/shared/lib/telemetry/consent";
import { startTelemetryIfAllowed } from "@/shared/lib/telemetry";

/**
 * Revocable telemetry choice in Settings. Mirrors the first-run banner and is
 * the only place a user can inspect or withdraw the stored decision.
 */
const TelemetryConsentSetting = () => {
  const { t } = useTranslation();
  const consent = useSyncExternalStore(
    subscribeTelemetryConsent,
    telemetryState,
    () => "pending" as const,
  );
  const choice = useSyncExternalStore(
    subscribeTelemetryConsent,
    telemetryConsent,
    () => "unset" as const,
  );

  const statusKey =
    consent === "granted"
      ? "telemetry.statusGranted"
      : consent === "denied" || consent === "disabled"
        ? "telemetry.statusDenied"
        : "telemetry.statusUnset";

  return (
    <SurfaceCard>
      <SurfaceCardHeader
        title={t("telemetry.settingsTitle")}
        description={t("telemetry.settingsDescription")}
      />
      <SurfaceCardContent>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <StatusChip
            status={consent === "granted" ? "positive" : "neutral"}
            label={t(statusKey)}
          />
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant={consent === "granted" ? "outline" : "default"}
              disabled={consent === "disabled" || consent === "pending"}
              onClick={() => {
                setTelemetryConsent("granted");
                startTelemetryIfAllowed();
              }}
            >
              {t("telemetry.grant")}
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={consent !== "granted" && choice !== "granted"}
              onClick={() => setTelemetryConsent("denied")}
            >
              {t("telemetry.deny")}
            </Button>
          </div>
        </div>
      </SurfaceCardContent>
    </SurfaceCard>
  );
};

export default TelemetryConsentSetting;
