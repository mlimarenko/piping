import { resolveNavigation } from "@/app/navigation";

/** The tab title uses the same section owner as the sidebar, including hidden pages. */
export function sectionLabelKeyForPath(pathname: string): string | null {
  return resolveNavigation(pathname).section?.labelKey ?? null;
}
