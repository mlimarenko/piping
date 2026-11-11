/**
 * Hex-color normalization helpers shared by {@link ColorField} and its tests.
 *
 * Convention: the canonical form is a lowercase 6-digit `#rrggbb` string.
 * Input is accepted case-insensitively, with or without a leading `#`, in
 * either 3-digit (`#rgb`) or 6-digit (`#rrggbb`) form. 3-digit values are
 * expanded by doubling each nibble (`#abc` -> `#aabbcc`).
 */

const HEX3 = /^[0-9a-f]{3}$/;
const HEX6 = /^[0-9a-f]{6}$/;

/** Strip a single leading `#` and lowercase. Does not validate. */
function strip(raw: string): string {
  const trimmed = raw.trim().toLowerCase();
  return trimmed.startsWith("#") ? trimmed.slice(1) : trimmed;
}

/** True when `raw` is a parseable 3- or 6-digit hex color (with/without `#`). */
export function isValidHex(raw: string): boolean {
  const body = strip(raw);
  return HEX3.test(body) || HEX6.test(body);
}

/**
 * Normalize any accepted hex form to canonical lowercase `#rrggbb`.
 * Returns `null` when the input is not a valid hex color.
 */
export function normalizeHex(raw: string): string | null {
  const body = strip(raw);
  if (HEX6.test(body)) return `#${body}`;
  if (HEX3.test(body)) {
    const [r, g, b] = body;
    return `#${r}${r}${g}${g}${b}${b}`;
  }
  return null;
}

/**
 * Best-effort coercion for feeding the native `<input type="color">`, which
 * only accepts canonical `#rrggbb`. Falls back to `#000000` for invalid input
 * so the OS picker always opens on a sensible swatch.
 */
export function toColorInputValue(raw: string): string {
  return normalizeHex(raw) ?? "#000000";
}
