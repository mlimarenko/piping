import { apiClient, unwrap } from "@/shared/api/client";
import { collectAllPages } from "@/shared/api/pagination";
import type { components } from "@/shared/api/schema.gen";
import type { ApiToken, CredentialProtocol, Paginated, PersonalCredential } from "@/shared/api/types";

export interface TokenScopeCatalogItem {
  action: string;
  resourceType: string;
  label: string;
  value: string;
}

const USER_CREDENTIAL_SET_CONSUMER_TYPE = "user_connection_credential_set";
const USER_CREDENTIAL_OWNER_SCOPE_TYPE = "user";

// ── Personal credentials ─────────────────────────────────

type BackendPersonalCredential = components["schemas"]["UserCredentialSetResponse"];

// UserCredentialSetResponse.params is `unknown` in the contract; the credential
// set stores optional non-secret RDP/SSH credential fields the UI surfaces.
interface CredentialParams {
  credentials?: { username?: string | null; domain?: string | null } | null;
}

function toCredentialProtocol(raw: string): CredentialProtocol {
  const normalized = raw.toUpperCase();
  if (normalized === "SSH" || normalized === "RDP" || normalized === "VNC") {
    return normalized;
  }
  throw new Error(`Unsupported credential protocol: ${raw}`);
}

function toPersonalCredential(raw: BackendPersonalCredential): PersonalCredential {
  const params = raw.params as CredentialParams | null | undefined;
  return {
    id: raw.id,
    userId: raw.user_id,
    protocol: toCredentialProtocol(raw.protocol),
    name: raw.name,
    isDefault: raw.is_default,
    username: params?.credentials?.username ?? undefined,
    domain: params?.credentials?.domain ?? undefined,
  };
}

function credentialParams(body: Partial<PersonalCredential>): Record<string, unknown> {
  if (body.protocol === "VNC") return {};
  const username = body.username?.trim();
  const domain = body.protocol === "RDP" ? body.domain?.trim() : undefined;
  if (!username && !domain) return {};
  const credentials: Record<string, string> = {};
  if (username) credentials.username = username;
  if (domain) credentials.domain = domain;
  return {
    credentials,
  };
}

function passwordFieldKey(protocol: CredentialProtocol): string {
  if (protocol === "SSH") return "ssh_password";
  if (protocol === "RDP") return "rdp_password";
  return "vnc_password";
}

async function applySecretField(
  credential: BackendPersonalCredential,
  fieldKey: string,
  value: string | undefined,
): Promise<void> {
  if (!value) return;
  const payload: components["schemas"]["ApplySecretFieldIntentPayload"] = {
    consumer_type: USER_CREDENTIAL_SET_CONSUMER_TYPE,
    consumer_id: credential.id,
    owner_scope_type: USER_CREDENTIAL_OWNER_SCOPE_TYPE,
    owner_scope_id: credential.user_id,
    field_key: fieldKey,
    intent: "set",
    value,
  };
  await apiClient.PUT("/api/v1/secret-fields", { body: payload });
}

async function applyCredentialSecrets(
  credential: BackendPersonalCredential,
  body: Partial<PersonalCredential>,
): Promise<void> {
  const protocol = toCredentialProtocol(credential.protocol);
  await applySecretField(credential, passwordFieldKey(protocol), body.password);
  if (protocol === "SSH") {
    await applySecretField(credential, "ssh_private_key", body.privateKey);
  }
}

export interface CredentialListQuery {
  sort?: string;
  order?: "asc" | "desc";
  protocol?: string[];
  is_default?: string[];
}

/**
 * Page size for the credential list.
 *
 * The endpoint honours `limit`/`offset` and defaults to a page when neither is
 * sent, so this list must page to completion rather than assume one response
 * carries every row — the callers render the result as the user's full set.
 */
const CREDENTIAL_LIST_PAGE_SIZE = 200;

export async function listMyCredentials(q?: CredentialListQuery): Promise<PersonalCredential[]> {
  const rows = await collectAllPages(
    async (limit, offset) => {
      const raw = unwrap(
        await apiClient.GET("/api/v1/user/settings/credentials", {
          params: {
            query: {
              sort: q?.sort,
              order: q?.order,
              protocol: q?.protocol?.join(","),
              is_default: q?.is_default?.join(","),
              limit,
              offset,
            },
          },
        }),
      );
      return {
        items: raw.items ?? [],
        total: raw.total ?? (raw.items ?? []).length,
        limit,
        offset,
      };
    },
    CREDENTIAL_LIST_PAGE_SIZE,
  );
  return rows.map(toPersonalCredential);
}

export async function createMyCredential(
  body: Omit<PersonalCredential, "id" | "userId">,
): Promise<PersonalCredential> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/user/settings/credentials", {
      body: {
        protocol: body.protocol.toLowerCase(),
        name: body.name,
        params: credentialParams(body),
        is_default: body.isDefault,
      },
    }),
  );
  await applyCredentialSecrets(raw, body);
  return toPersonalCredential(raw);
}

export async function updateMyCredential(
  id: string,
  body: Pick<PersonalCredential, "name" | "protocol" | "username" | "domain" | "password" | "privateKey">,
): Promise<PersonalCredential> {
  const raw = unwrap(
    await apiClient.PUT("/api/v1/user/settings/credentials/{id}", {
      params: { path: { id } },
      body: {
        name: body.name,
        params: credentialParams(body),
      },
    }),
  );
  await applyCredentialSecrets(raw, body);
  return toPersonalCredential(raw);
}

export async function deleteMyCredential(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/user/settings/credentials/{id}", {
    params: { path: { id } },
  });
}

export async function setDefaultMyCredential(id: string): Promise<void> {
  await apiClient.POST("/api/v1/user/settings/credentials/{id}/default", {
    params: { path: { id } },
  });
}

// ── API tokens ───────────────────────────────────────────

// UserApiTokenResponse (list) omits token_value; CreatedUserApiTokenResponse
// (create) adds it. The mapper accepts either and reads the plaintext only when
// present.
type BackendApiToken =
  | components["schemas"]["UserApiTokenResponse"]
  | components["schemas"]["CreatedUserApiTokenResponse"];
type BackendApiTokenScope = components["schemas"]["UserApiTokenScopeResponse"];

function toApiToken(raw: BackendApiToken): ApiToken {
  return {
    id: raw.id,
    name: raw.name,
    status: raw.status === "revoked" ? "revoked" : "active",
    scopes: raw.scopes.map(formatApiTokenScope).join(", "),
    createdAt: raw.created_at,
    expiresAt: raw.expires_at ?? "",
    plaintext: "token_value" in raw ? raw.token_value : undefined,
  };
}

function formatApiTokenScope(scope: BackendApiTokenScope): string {
  const base = `${scope.action}:${scope.resource_type}`;
  const pattern = scope.resource_pattern?.trim();
  if (!pattern || pattern === "*") {
    return base;
  }
  return `${base}@${pattern}`;
}

export interface ApiTokenListQuery {
  limit?: number;
  offset?: number;
  search?: string;
  sort?: string;
  order?: "asc" | "desc";
  scope?: string[];
  status?: string[];
}

export async function listMyTokens(q?: ApiTokenListQuery): Promise<Paginated<ApiToken>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/user/settings/api-tokens", {
      params: {
        query: {
          limit: q?.limit,
          offset: q?.offset,
          search: q?.search,
          sort: q?.sort,
          order: q?.order,
          scope: q?.scope?.join(","),
          status: q?.status?.join(","),
        },
      },
    }),
  );
  const items = (raw.items ?? []).map(toApiToken);
  return { items, total: raw.total, limit: raw.limit, offset: raw.offset };
}

export async function createMyToken(body: {
  name: string;
  scopes: string;
  expiresInSeconds?: number;
}): Promise<ApiToken> {
  const scopes = body.scopes
    .split(/[,\s]+/)
    .map((s) => s.trim())
    .filter(Boolean)
    .map(parseApiTokenScope);
  const raw = unwrap(
    await apiClient.POST("/api/v1/user/settings/api-tokens", {
      body: {
        name: body.name,
        scopes,
        expires_in_seconds: body.expiresInSeconds,
      },
    }),
  );
  return toApiToken(raw);
}

function parseApiTokenScope(scope: string): {
  action: string;
  resource_type: string;
  resource_pattern?: string;
} {
  const patternSeparator = scope.indexOf("@");
  const scopePart = patternSeparator === -1 ? scope : scope.slice(0, patternSeparator);
  const resourcePattern =
    patternSeparator === -1 ? undefined : scope.slice(patternSeparator + 1).trim();
  const resourceSeparator = scopePart.indexOf(":");
  if (resourceSeparator <= 0 || resourceSeparator === scopePart.length - 1) {
    throw new Error(`Invalid token scope: ${scope}`);
  }

  const left = scopePart.slice(0, resourceSeparator).trim();
  const right = scopePart.slice(resourceSeparator + 1).trim();
  return {
    action: left,
    resource_type: right,
    resource_pattern: resourcePattern || undefined,
  };
}

export async function revokeMyToken(id: string): Promise<void> {
  await apiClient.POST("/api/v1/user/settings/api-tokens/{id}/revoke", {
    params: { path: { id } },
  });
}

export async function listTokenScopes(): Promise<TokenScopeCatalogItem[]> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/user/settings/api-tokens/scope-catalog", {}),
  );
  return (raw.items ?? []).map((scope) => ({
    action: scope.action,
    resourceType: scope.action === "cctv" ? "cctv" : scope.resource_type,
    label: scope.label,
    value: scope.value,
  }));
}

// ── Login password ──────────────────────────────────────

export async function changeMyPassword(body: {
  currentPassword: string;
  newPassword: string;
}): Promise<void> {
  void unwrap(
    await apiClient.PUT("/api/v1/user/settings/password", {
      body: {
        current_password: body.currentPassword,
        new_password: body.newPassword,
      },
    }),
  );
}
