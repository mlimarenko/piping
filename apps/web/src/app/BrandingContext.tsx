import { createContext, useContext, useEffect, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getBranding, BRANDING_QUERY_KEY, type Branding } from "@/shared/api/branding-api";
import { applyBranding } from "@/shared/theme/branding";

interface BrandingContextValue {
  /** Resolved app name (branding value, or the i18n `brand` fallback). */
  appName: string;
  /** Custom logo data URL, or `null` to use the stock `PipingLogo`. */
  logoDataUrl: string | null;
  /** Raw branding record once loaded, else `null`. */
  branding: Branding | null;
  isLoading: boolean;
}

const BrandingContext = createContext<BrandingContextValue | null>(null);

/**
 * Loads instance branding at app boot and applies it (title, favicon,
 * palette) whenever the data changes. Mounted high in the tree so the whole
 * app + sidebar can consume `useBranding()`.
 *
 * The first paint uses the stock tokens; branding snaps in once the public
 * `/instance/branding` endpoint resolves.
 */
export function BrandingProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const query = useQuery({
    queryKey: BRANDING_QUERY_KEY,
    queryFn: getBranding,
    staleTime: 5 * 60_000,
  });

  const branding = query.data ?? null;

  useEffect(() => {
    applyBranding(branding);
  }, [branding]);

  const value: BrandingContextValue = {
    appName: branding?.app_name?.trim() || t("brand"),
    logoDataUrl: branding?.logo_data_url ?? null,
    branding,
    isLoading: query.isLoading,
  };

  return <BrandingContext.Provider value={value}>{children}</BrandingContext.Provider>;
}

/** Consume the active branding. Safe outside the provider (returns defaults). */
export function useBranding(): BrandingContextValue {
  const { t } = useTranslation();
  const ctx = useContext(BrandingContext);
  if (ctx) return ctx;
  return { appName: t("brand"), logoDataUrl: null, branding: null, isLoading: false };
}
