import type { TFunction } from "i18next";
import type { ApiError } from "@/shared/api/client";

const AUTH_ERROR_MESSAGE_KEYS: Record<string, string> = {
  license_seat_limit: "auth.errors.licenseSeatLimit",
  LICENSE_SEAT_LIMIT_EXCEEDED: "auth.errors.licenseSeatLimit",
  ENTITLEMENT_SEAT_LIMIT_REACHED: "auth.errors.licenseSeatLimit",
  license_refresh_required: "auth.errors.licenseRefreshRequired",
  LICENSE_REFRESH_REQUIRED: "auth.errors.licenseRefreshRequired",
  license_grace_ended: "auth.errors.licenseGraceEnded",
  LICENSE_GRACE_ENDED: "auth.errors.licenseGraceEnded",
  license_unavailable: "auth.errors.licenseUnavailable",
  LICENSE_UNAVAILABLE: "auth.errors.licenseUnavailable",
  ENTITLEMENT_BINDING_INVALID: "auth.errors.licenseUnavailable",
  auth_failed: "auth.errors.ssoFailed",
  invalid_request: "auth.errors.ssoFailed",
  invalid_state: "auth.errors.ssoFailed",
  server_error: "auth.errors.ssoFailed",
};

function isLicenseLikeAuthErrorValue(value: string | null | undefined): boolean {
  if (!value) return false;
  const trimmed = value.trim();
  return (
    trimmed.startsWith("LICENSE_") ||
    trimmed.startsWith("license_") ||
    trimmed.startsWith("ENTITLEMENT_")
  );
}

export function authErrorMessageFromCode(
  t: TFunction,
  code: string | null | undefined,
): string | null {
  if (!code) return null;
  const key = AUTH_ERROR_MESSAGE_KEYS[code];
  return key ? t(key) : null;
}

export function authErrorMessageFromApiError(t: TFunction, error: ApiError): string | null {
  const codeMessage = authErrorMessageFromCode(t, error.code);
  if (codeMessage) return codeMessage;

  const reasonMessage = authErrorMessageFromCode(t, error.reasonCode);
  if (reasonMessage) return reasonMessage;

  if (
    isLicenseLikeAuthErrorValue(error.code) ||
    isLicenseLikeAuthErrorValue(error.reasonCode)
  ) {
    return t("auth.errors.licenseUnavailable");
  }

  return null;
}
