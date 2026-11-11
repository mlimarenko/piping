import { useTranslation } from "react-i18next";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import EntityPicker, { type EntityPickerOption } from "@/shared/forms/EntityPicker";
import {
  parseFrontPortMappings,
  parseRearPortMappings,
  rearPortMappingError,
  serializeRearPortMappings,
  type RearPortMapping,
} from "@/features/inventory/rearPortMappings";

interface RearPortMappingsFieldProps {
  label: string;
  value: string;
  onChange?: (value: string) => void;
  disabled?: boolean;
  readOnly?: boolean;
  /** The front port's own `positions` draft value (bounds `position`). */
  frontPositions: string;
  /** Same-hardware rear port choices; each carries its `positions`. */
  options: EntityPickerOption[];
  loading?: boolean;
}

const clampPosition = (value: string): number => {
  const number = Number(value);
  if (!Number.isInteger(number) || number < 1) return 1;
  return Math.min(number, 1024);
};

/** Structured editor for FrontPortTemplate `rear_ports`. Rows are the only
 *  producer of the draft serialization; RearPortTemplate renders the reverse
 *  projection read-only through the same component. */
export default function RearPortMappingsField({
  label,
  value,
  onChange,
  disabled,
  readOnly,
  frontPositions,
  options,
  loading,
}: RearPortMappingsFieldProps) {
  const { t } = useTranslation();
  if (readOnly) {
    const projection = parseFrontPortMappings(value);
    return (
      <div className="space-y-1">
        {projection.length === 0 ? (
          <p className="text-sm text-muted-foreground">{t("inventory.resources.form.noRearPortMappings")}</p>
        ) : (
          <ul className="space-y-1 text-sm">
            {projection.map((row) => (
              <li key={`${row.rearPosition}-${row.frontPortId}`} className="rounded-md border border-border px-2.5 py-1.5">
                {t("inventory.resources.form.rearPortMappingRow", {
                  frontPosition: row.frontPosition,
                  rearPort: row.frontPortId,
                  rearPosition: row.rearPosition,
                })}
              </li>
            ))}
          </ul>
        )}
      </div>
    );
  }

  let rows: RearPortMapping[];
  try {
    rows = parseRearPortMappings(value) ?? [];
  } catch {
    rows = [];
  }
  const frontMax = clampPosition(frontPositions);
  const positionsOf = (rearPortId: string): number =>
    clampPosition(options.find((option) => option.id === rearPortId)?.positions ?? "1");
  const duplicatePositions = new Set(
    rows.map((row) => row.position).filter((position, index, all) => all.indexOf(position) !== index),
  );
  const errors = rows.map((row) => {
    const range = rearPortMappingError(row, frontMax, positionsOf(row.rearPortId));
    if (range === "position") return t("inventory.resources.form.frontPortPositionRange", { max: frontMax });
    if (range === "rearPosition") {
      return t("inventory.resources.form.rearPortPositionRange", { max: positionsOf(row.rearPortId) });
    }
    return duplicatePositions.has(row.position) ? t("inventory.resources.form.duplicateFrontPortPosition") : "";
  });
  const emit = (next: RearPortMapping[]) => onChange?.(serializeRearPortMappings(next));
  const update = (index: number, patch: Partial<RearPortMapping>) =>
    emit(rows.map((row, at) => (at === index ? { ...row, ...patch } : row)));

  return (
    <div className="space-y-2">
      {rows.length === 0 && (
        <p className="text-sm text-muted-foreground">{t("inventory.resources.form.noRearPortMappings")}</p>
      )}
        {rows.map((row, index) => (
          <div key={`mapping-${index}`} className="grid min-w-0 grid-cols-1 gap-2 rounded-md border border-border p-2 @md:grid-cols-[7rem_minmax(0,1fr)_7rem_auto]">
            <Input
              aria-label={t("inventory.resources.fields.frontPosition")}
              aria-invalid={Boolean(errors[index])}
              type="number"
              min={1}
              max={frontMax}
              value={String(row.position)}
              disabled={disabled}
              className="h-9 text-sm"
              onChange={(event) => update(index, { position: clampPosition(event.target.value) })}
            />
            <EntityPicker
              entity="Rear Port Templates"
              value={row.rearPortId}
              onChange={(rearPortId) => update(index, { rearPortId, rearPortPosition: 1 })}
              options={options}
              loading={loading}
              disabled={disabled}
              label={`${label} ${index + 1}`}
              placeholder={t("inventory.resources.fields.rearPort")}
            />
            <Input
              aria-label={t("inventory.resources.fields.rearPortPosition")}
              aria-invalid={Boolean(errors[index])}
              type="number"
              min={1}
              max={positionsOf(row.rearPortId)}
              value={String(row.rearPortPosition)}
              disabled={disabled}
              className="h-9 text-sm"
              onChange={(event) => update(index, { rearPortPosition: clampPosition(event.target.value) })}
            />
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label={t("inventory.resources.form.removeRearPortMapping", { position: row.position })}
              disabled={disabled}
              onClick={() => emit(rows.filter((_, at) => at !== index))}
            >
              <Trash2 className="h-4 w-4" />
            </Button>
            {errors[index] && <p role="alert" className="text-xs text-destructive @md:col-span-4">{errors[index]}</p>}
          </div>
        ))}
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={disabled || options.length === 0}
          title={options.length === 0 ? t("inventory.resources.form.noRearPortTargets") : undefined}
          onClick={() => emit([...rows, { position: rows.length + 1, rearPortId: options[0]?.id ?? "", rearPortPosition: 1 }])}
        >
          <Plus className="h-4 w-4" /> {t("inventory.resources.form.addRearPortMapping")}
        </Button>
      </div>
  );
}
