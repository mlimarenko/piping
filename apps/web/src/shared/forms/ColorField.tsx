import * as React from "react";
import { useTranslation } from "react-i18next";
import { Check } from "lucide-react";

import { cn } from "@/shared/lib/utils";
import { Input } from "@/shared/ui-primitives/input";
import {
  isValidHex,
  normalizeHex,
  toColorInputValue,
} from "@/shared/forms/colorUtils";

/**
 * Sensible default presets. Hardcoded on purpose — we do NOT reach into the
 * branding/theme layer. These mirror the app's status/primary hues so a
 * picked tag color reads coherently against the rest of the UI.
 */
const PRESETS: ReadonlyArray<{ hex: string; nameKey: string; fallback: string }> = [
  { hex: "#e6a817", nameKey: "inventory.resources.form.colorField.presets.amber", fallback: "Amber" },
  { hex: "#2fa86a", nameKey: "inventory.resources.form.colorField.presets.green", fallback: "Green" },
  { hex: "#e0a82e", nameKey: "inventory.resources.form.colorField.presets.warning", fallback: "Yellow" },
  { hex: "#d64545", nameKey: "inventory.resources.form.colorField.presets.red", fallback: "Red" },
  { hex: "#3399cc", nameKey: "inventory.resources.form.colorField.presets.blue", fallback: "Blue" },
  { hex: "#7c5cd6", nameKey: "inventory.resources.form.colorField.presets.violet", fallback: "Violet" },
  { hex: "#52606d", nameKey: "inventory.resources.form.colorField.presets.slate", fallback: "Slate" },
  { hex: "#1f2933", nameKey: "inventory.resources.form.colorField.presets.ink", fallback: "Ink" },
];

export interface ColorFieldProps {
  /** Current value. Any accepted hex form; rendered swatch uses the normalized value. */
  value: string;
  /** Fires with the canonical `#rrggbb` value when a swatch/preset/picker commits,
   * or with the raw text while the user is typing so the host stays controlled. */
  onChange: (hex: string) => void;
  id?: string;
  disabled?: boolean;
  /** Accessible label for the swatch trigger + text input group. */
  ariaLabel?: string;
}

/**
 * Polished, accessible color input: a clickable swatch that opens the OS color
 * picker, a hex text input with live validation, and a row of preset swatches.
 *
 * Controlled. Typing updates the host immediately (so invalid intermediate
 * states are visible); committing a valid hex normalizes to `#rrggbb`.
 */
const ColorField = ({ value, onChange, id, disabled, ariaLabel }: ColorFieldProps) => {
  const { t } = useTranslation();
  const nativeRef = React.useRef<HTMLInputElement>(null);

  const normalized = normalizeHex(value);
  const valid = value.trim() === "" || isValidHex(value);
  const swatchColor = normalized ?? "transparent";
  const label = ariaLabel ?? t("inventory.resources.form.colorField.label", { defaultValue: "Color" });
  const hexInputId = id;

  const openNativePicker = () => {
    if (disabled) return;
    nativeRef.current?.click();
  };

  /** Commit a fully-formed hex (preset / native picker). Always normalizes. */
  const commit = (raw: string) => {
    const next = normalizeHex(raw);
    if (next) onChange(next);
  };

  /** Blur on the text input: normalize a valid value, leave invalid text as-is. */
  const handleBlur = () => {
    if (value.trim() === "") return;
    const next = normalizeHex(value);
    if (next && next !== value) onChange(next);
  };

  return (
    <div className="space-y-1.5">
      <div className="flex items-center gap-1.5">
        <button
          type="button"
          onClick={openNativePicker}
          disabled={disabled}
          aria-label={t("inventory.resources.form.colorField.openPicker", {
            defaultValue: "Open color picker",
          })}
          className={cn(
            "size-9 shrink-0 rounded-md border border-input shadow-sm transition-shadow",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background",
            "disabled:cursor-not-allowed disabled:opacity-50",
            !normalized &&
              "bg-[repeating-conic-gradient(theme(colors.muted.DEFAULT)_0_25%,transparent_0_50%)] bg-[length:8px_8px]",
          )}
          style={normalized ? { backgroundColor: swatchColor } : undefined}
        />

        {/* Native picker: visually hidden, driven by the swatch button. */}
        <input
          ref={nativeRef}
          type="color"
          tabIndex={-1}
          aria-hidden="true"
          disabled={disabled}
          value={toColorInputValue(value)}
          onChange={(e) => commit(e.target.value)}
          className="pointer-events-none absolute h-0 w-0 opacity-0"
        />

        <Input
          id={hexInputId}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onBlur={handleBlur}
          disabled={disabled}
          spellCheck={false}
          autoComplete="off"
          inputMode="text"
          placeholder="#155dfc"
          aria-label={label}
          aria-invalid={!valid}
          className={cn(
            "h-9 flex-1 font-mono text-sm uppercase tracking-tight",
            !valid && "border-destructive focus-visible:ring-destructive",
          )}
        />
      </div>

      <div role="group" aria-label={t("inventory.resources.form.colorField.presetsLabel", { defaultValue: "Color presets" })} className="flex flex-wrap gap-1">
        {PRESETS.map((p) => {
          const isActive = normalized === p.hex;
          const name = t(p.nameKey, { defaultValue: p.fallback });
          return (
            <button
              key={p.hex}
              type="button"
              onClick={() => commit(p.hex)}
              disabled={disabled}
              aria-label={t("inventory.resources.form.colorField.usePreset", {
                defaultValue: "Use {{name}}",
                name,
              })}
              aria-pressed={isActive}
              title={name}
              className={cn(
                "flex size-6 items-center justify-center rounded-[5px] border border-border/60 transition-transform",
                "hover:scale-110 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background",
                "disabled:cursor-not-allowed disabled:opacity-50",
              )}
              style={{ backgroundColor: p.hex }}
            >
              {isActive && <Check className="h-3 w-3 text-white drop-shadow-[0_0_1px_rgba(0,0,0,0.7)]" />}
            </button>
          );
        })}
      </div>

      {!valid && (
        <p className="text-xs text-destructive">
          {t("inventory.resources.form.colorField.invalid", {
            defaultValue: "Enter a valid hex color, e.g. #155dfc",
          })}
        </p>
      )}
    </div>
  );
};

export default ColorField;
