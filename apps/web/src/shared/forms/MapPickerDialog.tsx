import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import ScrollableDialog from "@/shared/ui-primitives/scrollable-dialog";
import { Button } from "@/shared/ui-primitives/button";
import CustomMap, {
  type CustomMapPoint,
} from "@/features/inventory/components/map/CustomMap";
import { formatCoord } from "@/shared/forms/coordUtils";

interface MapPickerDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Currently selected position `[lat, lng]`, or null when none is set yet. */
  value: [number, number] | null;
  /** Confirm handler — receives 6-dp string lat/lng. */
  onConfirm: (lat: string, lng: string) => void;
}

const PICK_ID = "pick";

/** Empty-map framing for the picker: European Russia (Moscow). `[lat, lng]`. */
const DEFAULT_CENTER: [number, number] = [55.75, 37.62];

/**
 * Modal coordinate picker over the offline {@link CustomMap}. Clicking the map
 * (re)places a single marker; Confirm reports it back as 6-dp strings, Cancel
 * discards. The picked position is local dialog state until Confirm so
 * cancelling restores the incoming value — and because MapPicker conditionally
 * mounts this dialog, the initializer re-seeds from `value` on every open.
 */
const MapPickerDialog = ({ open, onOpenChange, value, onConfirm }: MapPickerDialogProps) => {
  const { t } = useTranslation();
  const [position, setPosition] = useState<[number, number] | null>(value);

  const handleMapClick = useCallback((lat: number, lng: number) => {
    setPosition([lat, lng]);
  }, []);

  const handleConfirm = () => {
    if (position) onConfirm(formatCoord(position[0]), formatCoord(position[1]));
    onOpenChange(false);
  };

  const points = useMemo<CustomMapPoint[]>(
    () => (position ? [{ id: PICK_ID, lat: position[0], lng: position[1] }] : []),
    [position],
  );

  return (
    <ScrollableDialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("inventory.map.pickOnMap")}
      description={
        position
          ? t("inventory.map.selectedCoords", {
              lat: formatCoord(position[0]),
              lng: formatCoord(position[1]),
            })
          : t("inventory.map.empty.hint")
      }
      footer={
        <>
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            {t("inventory.map.cancel")}
          </Button>
          <Button size="sm" onClick={handleConfirm} disabled={!position}>
            {t("inventory.map.confirm")}
          </Button>
        </>
      }
    >
      <div className="h-[420px] w-full overflow-hidden rounded-md border border-border/40">
        {/* No `fitKey` here: the dialog remounts on every open, so the one-time
            auto-fit already frames the initial value (or `defaultCenter` when
            empty). Omitting it means a click re-places the marker WITHOUT
            recentring the map. */}
        <CustomMap
          className="h-full w-full"
          points={points}
          onMapClick={handleMapClick}
          defaultCenter={DEFAULT_CENTER}
        />
      </div>
    </ScrollableDialog>
  );
};

export default MapPickerDialog;
