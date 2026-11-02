import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle } from "lucide-react";

import CustomMap, {
  type CustomMapPoint,
  type CustomMapProps,
} from "@/features/inventory/components/map/CustomMap";
import DetailedMap from "@/features/inventory/components/map/DetailedMap";
import { cn } from "@/shared/lib/utils";
import { IS_DEMO } from "@/demo/env";

export type { CustomMapPoint };

interface InventoryMapViewProps extends CustomMapProps {
  className?: string;
}

const InventoryMapView = (props: InventoryMapViewProps) => {
  const { t } = useTranslation();
  const [detailedReady, setDetailedReady] = useState(false);
  const [detailedUnavailable, setDetailedUnavailable] = useState(false);

  const handleReady = useCallback(() => {
    setDetailedUnavailable(false);
    setDetailedReady(true);
  }, []);

  const handleUnavailable = useCallback((reason: string) => {
    console.warn("[inventory-map] detailed map unavailable:", reason);
    setDetailedUnavailable(true);
    setDetailedReady(false);
  }, []);

  return (
    <div className={cn("relative h-full w-full overflow-hidden", props.className)}>
      {!detailedReady ? (
        <CustomMap
          {...props}
          className="h-full w-full"
        />
      ) : null}

      {!IS_DEMO && !detailedUnavailable ? (
        <DetailedMap
          {...props}
          className={cn(
            "absolute inset-0 h-full w-full transition-opacity duration-300",
            detailedReady ? "opacity-100" : "pointer-events-none opacity-0",
          )}
          onReady={handleReady}
          onUnavailable={handleUnavailable}
        />
      ) : null}

      {detailedUnavailable ? (
        <div className="absolute left-3 top-3 z-[1200] max-w-md rounded-md border border-status-warning bg-popover px-3 py-2 text-xs text-popover-foreground shadow-sm">
          <div className="flex items-start gap-2">
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-status-warning" />
            <div>
              <div className="font-medium">{t("inventory.map.detailedUnavailable")}</div>
              <div className="text-muted-foreground">{t("inventory.map.svgFallback")}</div>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
};

export default InventoryMapView;
