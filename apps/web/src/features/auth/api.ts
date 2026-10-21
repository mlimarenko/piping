import { apiClient, unwrap } from "@/shared/api/client";
import { API_BASE_URL } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";

// ── Frontend-shaped types (camelCase) ────────────────────

export interface LoginRequest {
  identifier: string;
  password: string;
}

export interface LoginResponse {
  user: CurrentUser;
}

/** The permission-vocabulary unions, sourced from the OpenAPI contract (which
 *  is generated from the backend registry). Gating on these — rather than free
 *  strings — makes a typo or a pruned name (`DEVICE`) a compile error. */
export type AccessAction = components["schemas"]["Action"];
export type AccessResourceType = components["schemas"]["ResourceType"];

/** One allowed `(action, resourceType)` pair from the caller's effective
 *  authorization. Type-level: presence means "allowed on some resource of this
 *  type" — the SPA uses it to gate navigation and controls. */
export interface Capability {
  action: AccessAction;
  resourceType: AccessResourceType;
}

export interface CurrentUser {
  id: string;
  username: string;
  email: string;
  /** "active" | "disabled" | … */
  status?: string;
  /** Session absolute expiry (ISO timestamp). */
  expiresAt?: string;
  /** Role names the caller resolves to (direct + group). */
  roles: string[];
  /** Type-level capability grid the SPA gates UI on. */
  capabilities: Capability[];
  /** "local" | "oidc" | … */
  authSource?: string;
}

export interface AuthProviders {
  localEnabled: boolean;
  oidcEnabled: boolean;
  ldapEnabled: boolean;
  adEnabled: boolean;
}

// ── Backend DTOs (snake_case, mapped at the boundary) ────

type BackendAuthSessionResponse = components["schemas"]["AuthSessionResponse"];

function toCurrentUser(raw: BackendAuthSessionResponse): CurrentUser {
  return {
    id: raw.user_id,
    username: raw.username,
    email: raw.email,
    status: raw.status,
    authSource: raw.auth_source,
    expiresAt: raw.expires_at,
    roles: raw.roles ?? [],
    capabilities: (raw.capabilities ?? []).map((c) => ({
      action: c.action,
      resourceType: c.resource_type,
    })),
  };
}

// ── Operations ───────────────────────────────────────────

/** POST /api/v1/auth/login — local identifier+password; backend sets session cookie. */
export async function login(body: LoginRequest): Promise<LoginResponse> {
  const data = unwrap(await apiClient.POST("/api/v1/auth/login", { body }));
  return { user: toCurrentUser(data) };
}

/** POST /api/v1/auth/logout — clears the session cookie. */
export async function logout(): Promise<void> {
  await apiClient.POST("/api/v1/auth/logout", {});
}

/** GET /api/v1/auth/me — current authenticated user or throws ApiError(401). */
export async function me(): Promise<CurrentUser> {
  const data = unwrap(await apiClient.GET("/api/v1/auth/me", {}));
  return toCurrentUser(data);
}

/** GET /api/v1/auth/providers — which auth sources are enabled. */
export async function getAuthProviders(): Promise<AuthProviders> {
  const raw = unwrap(await apiClient.GET("/api/v1/auth/providers", {}));
  return {
    localEnabled: raw.local_enabled,
    oidcEnabled: raw.oidc_enabled,
    ldapEnabled: raw.ldap_enabled,
    adEnabled: raw.ad_enabled,
  };
}

/**
 * Build URL to start the OIDC login flow.
 * Browser navigates to this URL; backend issues a 302 to the IdP and returns
 * via `GET /api/v1/auth/sso/callback` — which itself redirects back to the UI.
 */
export function buildSsoStartUrl(redirectTo?: string): string {
  const qs = redirectTo ? `?redirect_to=${encodeURIComponent(redirectTo)}` : "";
  return `${API_BASE_URL}/api/v1/auth/login${qs}`;
}

export function safeLoginReturnTo(value: string | null): string {
  if (!value || !value.startsWith("/") || value.startsWith("//") || value.includes("\\") || [...value].some((char) => (char.codePointAt(0) ?? 0) <= 32)) return "/connections";
  const url = new URL(value, window.location.origin);
  if (url.origin !== window.location.origin || url.pathname.startsWith("//") || url.pathname.startsWith("/auth/")) return "/connections";
  return url.pathname + url.search + url.hash;
}
