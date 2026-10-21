import { lazy, Suspense, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { MapPin } from "lucide-react";
import { Input } from "@/shared/ui-primitives/input";
import { Button } from "@/shared/ui-primitives/button";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import {
  formatCoord,
  LAT_MAX,
  LAT_MIN,
  LNG_MAX,
  LNG_MIN,
  parseLat,
  parseLng,
} from "@/shared/forms/coordUtils";

// The dialog mounts the offline SVG map; load it only when the user opens it.
const MapPickerDialog = lazy(() => import("@/shared/forms/MapPickerDialog"));

export interface MapPickerProps {
  /** Latitude as a string (the form's canonical storage). */
  lat?: string;
  /** Longitude as a string. */
  lng?: string;
  /** Writes both coordinates back as strings (6-dp on map pick). */
  onChange: (lat: string, lng: string) => void;
  /** Disable both inputs and the picker button. */
  disabled?: boolean;
}

/**
 * Coordinate field pair with a map affordance. The manual numeric inputs stay
 * the canonical data source (range-validated −90..90 / −180..180); the "pick on
 * map" button opens an offline SVG-map dialog whose marker writes 6-dp strings
 * back into the same fields.
 *
 * Two-way sync: the parent owns `lat`/`lng` strings and receives every change
 * through `onChange`, so the inputs and the dialog never diverge.
 */
const MapPicker = ({ lat, lng, onChange, disabled = false }: MapPickerProps) => {
  const { t } = useTranslation();
  const [dialogOpen, setDialogOpen] = useState(false);

  const latNum = parseLat(lat);
  const lngNum = parseLng(lng);
  const latInvalid = (lat ?? "").trim() !== "" && latNum == null;
  const lngInvalid = (lng ?? "").trim() !== "" && lngNum == null;

  // A complete, valid pair the dialog can open on; otherwise null (default view).
  const value = useMemo<[number, number] | null>(
    () => (latNum != null && lngNum != null ? [latNum, lngNum] : null),
    [latNum, lngNum],
  );

  const handleConfirm = (nextLat: string, nextLng: string) => {
    onChange(nextLat, nextLng);
  };

  return (
    <div className="space-y-2">
      <div className="grid grid-cols-2 gap-2">
        <div className="space-y-1">
          <FieldLabel className="text-xs font-medium">{t("inventory.map.latitude")}</FieldLabel>
          <Input
            type="number"
            inputMode="decimal"
            step="any"
            min={LAT_MIN}
            max={LAT_MAX}
            value={lat ?? ""}
            onChange={(event) => onChange(event.target.value, lng ?? "")}
            placeholder={t("inventory.resources.fields.latitudePlaceholder")}
            className="h-9 text-sm"
            disabled={disabled}
            aria-invalid={latInvalid}
            aria-label={t("inventory.map.latitude")}
          />
        </div>
        <div className="space-y-1">
          <FieldLabel className="text-xs font-medium">{t("inventory.map.longitude")}</FieldLabel>
          <Input
            type="number"
            inputMode="decimal"
            step="any"
            min={LNG_MIN}
            max={LNG_MAX}
            value={lng ?? ""}
            onChange={(event) => onChange(lat ?? "", event.target.value)}
            placeholder={t("inventory.resources.fields.longitudePlaceholder")}
            className="h-9 text-sm"
            disabled={disabled}
            aria-invalid={lngInvalid}
            aria-label={t("inventory.map.longitude")}
          />
        </div>
      </div>

      {(latInvalid || lngInvalid) && (
        <p className="text-xs text-destructive">{t("inventory.map.rangeError")}</p>
      )}

      <div className="flex items-center gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="gap-2 text-sm"
          onClick={() => setDialogOpen(true)}
          disabled={disabled}
        >
          <MapPin className="h-3.5 w-3.5" />
          {t("inventory.map.pickOnMap")}
        </Button>
        {value && (
          <button
            type="button"
            className="text-xs text-muted-foreground hover:text-foreground hover:underline"
            onClick={() => onChange("", "")}
            disabled={disabled}
          >
            {t("inventory.map.clear")}
          </button>
        )}
      </div>

      {value && (
        <p className="text-xs text-muted-foreground">
          {t("inventory.map.selectedCoords", {
            lat: formatCoord(value[0]),
            lng: formatCoord(value[1]),
          })}
        </p>
      )}

      {dialogOpen && (
        <Suspense fallback={null}>
          <MapPickerDialog
            open={dialogOpen}
            onOpenChange={setDialogOpen}
            value={value}
            onConfirm={handleConfirm}
          />
        </Suspense>
      )}
    </div>
  );
};

export default MapPicker;
