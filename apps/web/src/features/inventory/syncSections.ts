import type { TFunction } from "i18next";
import { Layers, Plug } from "lucide-react";
import type { Location } from "react-router";
import type { SectionLinkItem } from "@/shared/ui-shared/SectionLinks";

type SyncSection = "providers" | "mappings";

/** The single canonical query-param name for a selected provider on `/inventory/sync`. */
export const SYNC_PROVIDER_PARAM = "provider";

const SECTIONS = [
  { value: "providers", pathname: "/inventory/sync", icon: Plug, labelKey: "inventory.sync.providersTab" },
  { value: "mappings", pathname: "/inventory/sync/mappings", icon: Layers, labelKey: "inventory.sync.mappingsTab" },
] as const;

/**
 * Navigation target that selects (or clears) `providerId` on the current list
 * URL while preserving every other query param, the hash and the pathname.
 * Used by the three provider-selection writers (row click, create-success,
 * clear) so no one drops context/hash — only the `provider` param changes.
 */
export function syncProviderNavigation(
  location: Pick<Location, "pathname" | "search" | "hash">,
  providerId: string | null,
): { pathname: string; search: string; hash: string } {
  const next = new URLSearchParams(location.search);
  if (providerId) next.set(SYNC_PROVIDER_PARAM, providerId);
  else next.delete(SYNC_PROVIDER_PARAM);
  const qs = next.toString();
  return { pathname: location.pathname, search: qs ? `?${qs}` : "", hash: location.hash };
}

/**
 * Section links with a provider-aware destination: a selected provider is
 * carried as `?provider=<id>` on the providers list and as the `:providerId`
 * path segment on the provider-scoped mappings list, so a round-trip through
 * the other section preserves the selection (and reload/back keep the inspector).
 * Other query/hash context is preserved verbatim; only the `provider` param
 * moves between query and path.
 */
export function syncSectionLinks(
  t: TFunction,
  counts: Partial<Record<SyncSection, number>>,
  location: Pick<Location, "pathname" | "search" | "hash">,
  active: SyncSection,
  providerId?: string,
): SectionLinkItem<SyncSection>[] {
  return SECTIONS.map(({ value, pathname, icon, labelKey }) => {
    let to: { pathname: string; search: string; hash: string };
    if (value === active) {
      // The active section is a no-op link to the current URL.
      to = { pathname: location.pathname, search: location.search, hash: location.hash };
    } else if (value === "providers") {
      const search = new URLSearchParams(location.search);
      if (providerId) search.set(SYNC_PROVIDER_PARAM, providerId);
      const qs = search.toString();
      to = { pathname, search: qs ? `?${qs}` : "", hash: location.hash };
    } else {
      const search = new URLSearchParams(location.search);
      search.delete(SYNC_PROVIDER_PARAM);
      const qs = search.toString();
      to = {
        pathname: providerId ? `/inventory/sync/${encodeURIComponent(providerId)}/mappings` : pathname,
        search: qs ? `?${qs}` : "",
        hash: location.hash,
      };
    }
    return {
      value,
      to,
      icon,
      label: counts[value] == null ? t(labelKey) : `${t(labelKey)} (${counts[value]})`,
    };
  });
}
