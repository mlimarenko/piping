import { useEffect } from "react";

/** Separator between title segments (e.g. `Piping · Inventory`). */
const TITLE_SEPARATOR = " · ";

/**
 * Own `document.title` for the lifetime of the calling component.
 *
 * Pass the ordered segments — falsy entries are dropped and the rest are
 * joined with a middot, so `["Piping", "", "Inventory"]` → `Piping · Inventory`.
 * Every route is expected to own its title via this hook (the app shell sets a
 * route-derived title; the detached session sets its connection context), so
 * there is intentionally no restore-on-unmount: restoring would only flicker
 * the previous title in during route transitions before the next owner runs.
 *
 * This is the single writer of `document.title` in the app — branding no longer
 * sets it directly (see `shared/theme/branding.ts`); the instance name flows in
 * here as the leading segment via `useBranding().appName`.
 */
export function useDocumentTitle(parts: ReadonlyArray<string | null | undefined>): void {
  const title = parts.filter((part): part is string => Boolean(part && part.trim())).join(
    TITLE_SEPARATOR,
  );

  useEffect(() => {
    if (typeof document === "undefined" || !title) return;
    document.title = title;
  }, [title]);
}
