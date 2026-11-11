/**
 * Canonical navigation-label layout (PWA-65).
 *
 * A navigation label must be fully readable. A single-line ellipsis hides one —
 * the 208px rail rendered "Конфигурации подключений" as "Конфигурации подклю…"
 * with no way to read the rest — so no navigation surface may use `truncate`.
 * The one rule, applied by every surface (sidebar sections and items, the
 * collapsed-rail dropdown, the top-bar page title), is {@link navLabelClass}:
 *
 *   - `min-w-0` lets the label shrink as a flex child beside a leading icon;
 *   - `flex-1` makes it take the remaining width (a no-op outside a flex row);
 *   - `break-words` wraps a long unbroken token;
 *   - `line-clamp-2` caps the visual height at two lines.
 *
 * The DOM text is never removed, so the accessible name and the `title`
 * attribute always carry the whole label even past two lines. Pair the label
 * with {@link navLabelIconClass} on the row's leading icon, plus `items-start`
 * on the row, so the icon stays on the first line when the label wraps.
 */
export const navLabelClass = "min-w-0 flex-1 break-words line-clamp-2";

/**
 * Pins a leading navigation icon to the first text line once its sibling label
 * wraps to a second: `mt-0.5` is the half-leading of the 16px first line of
 * `text-xs` around a 12px icon. Pair with `items-start` on the row.
 */
export const navLabelIconClass = "shrink-0 mt-0.5";
