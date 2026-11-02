import { apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";

/** TanStack Query key for the instance branding record. */
export const BRANDING_QUERY_KEY = ["branding"] as const;

/** Instance branding as returned by the backend. */
export type Branding = components["schemas"]["BrandingResponse"];

/** Payload accepted by `PUT /api/v1/instance/branding` (admin only). */
export type BrandingPayload = components["schemas"]["BrandingPayload"];

/**
 * Shape of the `theme` JSON column. The backend stores it opaquely (the
 * generated type is `Record<string, never>`), so the frontend owns this
 * contract and reads/writes it through the helpers in
 * `@/shared/theme/branding`.
 */
export interface BrandingTheme {
  mode: "preset" | "custom" | "default";
  presetId?: string;
  base?: { h: number; s: number; l: number };
}

/** GET /api/v1/instance/branding — public first-paint branding. */
export async function getBranding(): Promise<Branding> {
  return unwrap(await apiClient.GET("/api/v1/instance/branding", {}));
}

/** PUT /api/v1/instance/branding — admin only (backend-enforced). */
export async function updateBranding(body: BrandingPayload): Promise<Branding> {
  return unwrap(await apiClient.PUT("/api/v1/instance/branding", { body }));
}
