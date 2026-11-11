import { apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";
import type { LicenseActivateRequest, LicenseInfo } from "@/shared/api/types";
import { z } from "zod";

type BackendLicenseState = components["schemas"]["EntitlementState"];

const licenseStateSchema = z.object({
  status: z.enum(["active", "expired", "trial", "invalid", "missing", "inactive", "grace", "suspended"]),
  seatLimit: z.number().int().nonnegative(),
  seatsUsed: z.number().int().nonnegative(),
  version: z.number(),
  validFrom: z.string().nullish(),
  validUntil: z.string().nullish(),
});

function toLicenseInfo(input: BackendLicenseState): LicenseInfo {
  const raw = licenseStateSchema.parse(input);
  return {
    status: raw.status,
    version: String(raw.version ?? ""),
    seats: {
      used: raw.seatsUsed,
      total: raw.seatLimit,
    },
    validFrom: raw.validFrom ?? "",
    validUntil: raw.validUntil ?? "",
  };
}

/** GET /api/v1/license/state — canonical state endpoint. */
export async function getLicense(): Promise<LicenseInfo> {
  const raw = unwrap(await apiClient.GET("/api/v1/license/state", {}));
  return toLicenseInfo(raw);
}

/** POST /api/v1/license/activate — submit activation token. */
export async function activateLicense(body: LicenseActivateRequest): Promise<LicenseInfo> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/license/activate", { body }),
  );
  return toLicenseInfo(raw);
}

/** POST /api/v1/license/refresh — re-validate against control plane. */
export async function refreshLicense(): Promise<LicenseInfo> {
  unwrap(await apiClient.POST("/api/v1/license/refresh", {}));
  return getLicense();
}

export async function getLicenseHealth() {
  return unwrap(await apiClient.GET("/api/v1/license/health/details", {}));
}
