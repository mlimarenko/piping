import { apiClient, unwrap } from "@/shared/api/client";
import { collectAllPages } from "@/shared/api/pagination";
import { assertUuid, type Uuid } from "@/shared/lib/ids";
import { listPlaybookServicePrincipals } from "@/features/playbooks/api";
import type { components } from "@/shared/api/schema.gen";
import type {
  AccessCheckRequest,
  AccessCheckResponse,
  CreateUserRequest,
  Grant,
  Group,
  ListQuery,
  Paginated,
  Policy,
  Role,
  SsoConfig,
  SubjectRule,
  UserRow,
} from "@/shared/api/types";

/** The picker view-model for a service principal subject source. */
export interface ServicePrincipalRow {
  id: string;
  name: string;
  slug: string;
}

// ── Backend DTOs (snake_case) ─────────────────────────────
//
// Every list endpoint under /api/v1/admin/access-control/* returns the canonical
// envelope `{ items, total, limit, offset }` with a real row count computed
// server-side. The generated schema expresses this as one
// `AccessControlListEnvelope_<T>` component per resource, so the response shapes
// below come straight from `components["schemas"]` with no casts.

/** Element type of a generated access-control list envelope's `items` array. */
type EnvelopeItem<E extends { items: unknown[] }> = E["items"][number];

function toList<E extends { items: unknown[]; total: number; limit: number; offset: number }, Out>(
  raw: E,
  map: (r: EnvelopeItem<E>) => Out,
): Paginated<Out> {
  // Defensive: never assume `items` is present/array. A malformed or partial
  // envelope (missing `items`) must degrade to an empty list, not throw
  // `e.items is undefined` and white-screen the whole view via the error
  // boundary. total/limit/offset are likewise backfilled from the items length.
  const items = (Array.isArray((raw as { items?: unknown }).items)
    ? (raw.items as EnvelopeItem<E>[])
    : []
  ).map(map);
  return {
    items,
    total: typeof raw.total === "number" ? raw.total : items.length,
    limit: typeof raw.limit === "number" ? raw.limit : items.length,
    offset: typeof raw.offset === "number" ? raw.offset : 0,
  };
}

function listQueryParams(q?: ListQuery): { limit?: number; offset?: number; search?: string } {
  return {
    limit: q?.limit,
    offset: q?.offset,
    ...(q?.search ? { search: q.search } : {}),
  };
}

/**
 * Sort/filter input for the access-control list tables. A superset of
 * {@link ListQuery} (existing `{ limit, offset, search }` callers keep
 * compiling). `sort`/`order` map to each endpoint's allowlisted sort contract;
 * `filters` is `backend param -> selected values`, comma-joined into the `IN`
 * form. For grants the caller pre-routes the subject selection into the
 * `user_id`/`group_id` params (it owns the users/groups membership lookup).
 */
export interface AccessListQuery extends ListQuery {
  sort?: string;
  order?: "asc" | "desc";
  /** Backend param name -> selected values (joined with `,`). */
  filters?: Record<string, string[]>;
}

/** The flat query shared by every access-control list endpoint: base
 *  limit/offset/search, optional sort/order, and comma-joined `IN` filters.
 *  Exported for unit testing the param-building contract (the new backend
 *  filtering is not reachable from the local stack). */
export function buildAccessListParams(
  q?: AccessListQuery,
): Record<string, string | number | undefined> {
  const params: Record<string, string | number | undefined> = listQueryParams(q);
  if (q?.sort) {
    params.sort = q.sort;
    params.order = q.order ?? "asc";
  }
  for (const [param, values] of Object.entries(q?.filters ?? {})) {
    if (values.length > 0) params[param] = values.join(",");
  }
  return params;
}

// ── Users ────────────────────────────────────────────────

/**
 * Canonical TanStack Query key base for user lists.
 *
 * Every user query must be keyed under this prefix so that
 * `invalidateQueries({ queryKey: USERS_QUERY_KEY })` prefix-matches all
 * variants (the full list, the paginated table, and the enrichment lookup).
 */
export const USERS_QUERY_KEY = ["access", "users"] as const;

/** Key for the paginated users table query (server-side pagination + search +
 *  optional sort/filter dimensions). Keeping the `USERS_QUERY_KEY` prefix lets
 *  `invalidateQueries({ queryKey: USERS_QUERY_KEY })` still prefix-match it. */
export const usersPaginatedQueryKey = (
  params: { page: number; pageSize: number; search: string },
  sort?: string,
  order?: "asc" | "desc",
  filters?: Record<string, string[]>,
) => [...USERS_QUERY_KEY, "paginated", params, { sort, order, filters }] as const;

/** Key for the full-user enrichment lookup (id → user). */
export const usersAllQueryKey = () => [...USERS_QUERY_KEY, "all"] as const;

type UsersEnvelope = components["schemas"]["AccessControlListEnvelope_UserResponse"];
type BackendUser = UsersEnvelope["items"][number];

function toUserRow(raw: BackendUser): UserRow {
  return {
    // Read boundary: `raw.id` is an id column from the typed backend contract.
    // Mint with `as Uuid` (not assertUuid) — a per-row throw would regress the
    // "degrade to empty list, never white-screen" contract of `toList`, and adds
    // no compile-time protection over the `id: Uuid` annotation itself.
    id: raw.id as Uuid,
    username: raw.username,
    email: raw.email ?? "",
    status: (raw.status as UserRow["status"]) ?? "active",
    authSource: (raw.auth_source as UserRow["authSource"]) ?? "local",
    externalId: raw.external_id ?? "",
  };
}

export async function listUsers(q?: AccessListQuery): Promise<Paginated<UserRow>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/admin/access-control/users", {
      params: { query: buildAccessListParams(q) },
    }),
  );
  return toList(raw, toUserRow);
}

export async function createUser(body: CreateUserRequest): Promise<UserRow> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/admin/access-control/users", {
      body: {
        username: body.username,
        email: body.email,
        status: body.status,
        auth_source: body.authSource,
        external_id: body.externalId,
        // UserInput has no top-level `password`; a local password must travel as
        // a CredentialInput (mode "manual"). Sending it at the top level was
        // silently dropped by the backend → the user got no credential and could
        // not log in.
        credential: body.password
          ? { mode: "manual", password: body.password }
          : undefined,
      },
    }),
  );
  return toUserRow(raw);
}

export async function updateUser(id: string, body: Omit<UserRow, "id">): Promise<UserRow> {
  const raw = unwrap(
    await apiClient.PUT("/api/v1/admin/access-control/users/{id}", {
      params: { path: { id } },
      // PUT replaces the user with a full UserInput (all fields required); the
      // body param mirrors that — a partial update is not supported here.
      body: {
        username: body.username,
        email: body.email,
        status: body.status,
        auth_source: body.authSource,
        external_id: body.externalId,
      },
    }),
  );
  return toUserRow(raw);
}

export async function deleteUser(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/admin/access-control/users/{id}", {
    params: { path: { id } },
  });
}

export async function changeUserPassword(id: string, newPassword: string): Promise<void> {
  void unwrap(
    await apiClient.PUT("/api/v1/admin/access-control/users/{id}/password", {
      params: { path: { id } },
      // PasswordRotationInput requires `mode`; "manual" pairs with an explicit password.
      body: { mode: "manual", password: newPassword },
    }),
  );
}

// ── Groups ───────────────────────────────────────────────

type GroupsEnvelope = components["schemas"]["AccessControlListEnvelope_GroupResponse"];
type BackendGroup = GroupsEnvelope["items"][number];

function toGroup(raw: BackendGroup): Group {
  return {
    // Read boundary: id column from the typed contract (see toUserRow note).
    id: raw.id as Uuid,
    name: raw.name,
    externalId: raw.external_id ?? "",
  };
}

export async function listGroups(q?: AccessListQuery): Promise<Paginated<Group>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/admin/access-control/groups", {
      params: { query: buildAccessListParams(q) },
    }),
  );
  return toList(raw, toGroup);
}

export async function createGroup(body: Omit<Group, "id">): Promise<Group> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/admin/access-control/groups", {
      body: {
        name: body.name,
        external_id: body.externalId,
      },
    }),
  );
  return toGroup(raw);
}

export async function updateGroup(id: string, body: Omit<Group, "id">): Promise<Group> {
  const raw = unwrap(
    await apiClient.PUT("/api/v1/admin/access-control/groups/{id}", {
      params: { path: { id } },
      body: {
        name: body.name,
        external_id: body.externalId,
      },
    }),
  );
  return toGroup(raw);
}

export async function deleteGroup(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/admin/access-control/groups/{id}", {
    params: { path: { id } },
  });
}

// ── Roles ────────────────────────────────────────────────

// Envelope item and detail RoleResponse are structurally identical; mapping uses
// the named RoleResponse and toList infers the envelope shape from the GET result.
type BackendRole = components["schemas"]["RoleResponse"];

// The generated RoleResponse (list item and detail) carries only id/name/description.
// policy_count is not part of the contract, so it defaults to 0 in the UI projection.
function toRole(raw: BackendRole): Role {
  return {
    // Read boundary: id column from the typed contract (see toUserRow note).
    id: raw.id as Uuid,
    name: raw.name,
    description: raw.description ?? "",
    policyCount: 0,
  };
}

export async function listRoles(q?: AccessListQuery): Promise<Paginated<Role>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/admin/access-control/roles", {
      params: { query: buildAccessListParams(q) },
    }),
  );
  return toList(raw, toRole);
}

export async function createRole(body: Omit<Role, "id" | "policyCount">): Promise<Role> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/admin/access-control/roles", {
      body: {
        name: body.name,
        description: body.description,
      },
    }),
  );
  return toRole(raw);
}

export async function updateRole(
  id: string,
  body: Omit<Role, "id" | "policyCount">,
): Promise<Role> {
  const raw = unwrap(
    await apiClient.PUT("/api/v1/admin/access-control/roles/{id}", {
      params: { path: { id } },
      body: {
        name: body.name,
        description: body.description,
      },
    }),
  );
  return toRole(raw);
}

export async function deleteRole(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/admin/access-control/roles/{id}", {
    params: { path: { id } },
  });
}

// ── Policies ─────────────────────────────────────────────

// Envelope item and detail PolicyResponse are structurally identical; mapping uses
// the named PolicyResponse and toList infers the envelope shape from the GET result.
type BackendPolicy = components["schemas"]["PolicyResponse"];

const SUBJECT_KINDS: readonly SubjectRule["kind"][] = [
  "user",
  "group",
  "role",
  "service_principal",
  "oidc_group",
];

/**
 * Parse a canonical backend subject string `"{kind}:{value}"` into a
 * {@link SubjectRule}. Splits on the FIRST `:` so an `oidc_group` external name
 * containing a colon survives intact. `id`/`label` default to the parsed value
 * (the bare uuid, or the external name for `oidc_group`); display names are
 * resolved at render time from the entity caches (`useAccessLabels`). An
 * unrecognised prefix falls back to `user` so the subject stays editable rather
 * than being silently dropped.
 */
function parseSubject(s: string): Omit<SubjectRule, "id"> & { id: string } {
  const sep = s.indexOf(":");
  const rawKind = sep === -1 ? "" : s.slice(0, sep);
  const value = sep === -1 ? s : s.slice(sep + 1);
  const kind = (SUBJECT_KINDS as readonly string[]).includes(rawKind)
    ? (rawKind as SubjectRule["kind"])
    : "user";
  return { id: value, kind, value: s, label: value };
}

function toPolicy(raw: BackendPolicy): Policy {
  return {
    // Read boundary: id column from the typed contract (see toUserRow note).
    id: raw.id as Uuid,
    name: raw.name,
    description: raw.description ?? "",
    effect: (raw.effect as Policy["effect"]) ?? "allow",
    enabled: raw.enabled ?? true,
    actions: (raw.actions ?? []).map((a, i) => ({ id: `${raw.id}:a:${i}`, action: a })),
    subjects: (raw.subjects ?? []).map((s) => parseSubject(s)),
    scopes: [
      ...(raw.resource_types ?? []).map((v, i) => ({
        id: `${raw.id}:rt:${i}`,
        type: "resource_type" as const,
        value: v,
      })),
      // Specific resource grants (e.g. `secret_variable:{uuid}`) round-trip as
      // `resource` scopes — the wildcard `*` is the "all resources" default and
      // is folded back out.
      ...(raw.resources ?? [])
        .filter((v) => v !== "*")
        .map((v, i) => ({ id: `${raw.id}:r:${i}`, type: "resource" as const, value: v })),
    ],
    constraints: raw.constraints,
  };
}

function toPolicyPayload(
  body: Omit<Policy, "id">,
): components["schemas"]["PolicyInput"] {
  return {
    name: body.name,
    description: body.description,
    effect: body.effect,
    enabled: body.enabled,
    actions: body.actions.map((a) => a.action),
    resource_types: body.scopes
      .filter((s) => s.type === "resource_type")
      .map((s) => s.value),
    // Specific `resource` scopes serialize into `resources` (e.g. per-secret
    // grants `secret_variable:{uuid}`); with none selected we fall back to the
    // backend wildcard `*` (all resources of the chosen types), which the read
    // mapper strips back out.
    resources: (() => {
      const specific = body.scopes.filter((s) => s.type === "resource").map((s) => s.value);
      return specific.length > 0 ? specific : ["*"];
    })(),
    subjects: body.subjects.map((s) => s.value),
    // Only forward `constraints` when the editor produced one — an unconstrained
    // policy keeps the exact payload shape it had before this field existed.
    ...(body.constraints !== undefined ? { constraints: body.constraints } : {}),
  };
}

export async function listPolicies(q?: AccessListQuery): Promise<Paginated<Policy>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/admin/access-control/policies", {
      params: { query: buildAccessListParams(q) },
    }),
  );
  return toList(raw, toPolicy);
}

export async function createPolicy(body: Omit<Policy, "id">): Promise<Policy> {
  const raw = unwrap(
    await apiClient.POST("/api/v1/admin/access-control/policies", {
      body: toPolicyPayload(body),
    }),
  );
  return toPolicy(raw);
}

export async function updatePolicy(id: string, body: Omit<Policy, "id">): Promise<Policy> {
  const raw = unwrap(
    await apiClient.PUT("/api/v1/admin/access-control/policies/{id}", {
      params: { path: { id } },
      body: toPolicyPayload(body),
    }),
  );
  return toPolicy(raw);
}

export async function deletePolicy(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/admin/access-control/policies/{id}", {
    params: { path: { id } },
  });
}

export async function enablePolicy(id: string): Promise<void> {
  void unwrap(
    await apiClient.POST("/api/v1/admin/access-control/policies/{id}/enable", {
      params: { path: { id } },
    }),
  );
}

export async function disablePolicy(id: string): Promise<void> {
  void unwrap(
    await apiClient.POST("/api/v1/admin/access-control/policies/{id}/disable", {
      params: { path: { id } },
    }),
  );
}

// ── Grants (role-assignments in backend) ─────────────────

type BackendRoleAssignment = components["schemas"]["RoleAssignmentResponse"];
type BackendMatchedPolicy = components["schemas"]["PolicyTraceRefResponse"];
type BackendAccessCheckResult = components["schemas"]["AccessEvaluationResponse"];

function toGrant(raw: BackendRoleAssignment): Grant {
  // Read boundary: id columns from the typed contract (see toUserRow note). A
  // role-assignment always carries exactly one of user_id / group_id; the `?? ""`
  // is a defensive fallback for a contract-violating row (rendered as an unknown
  // ref rather than throwing mid-list).
  return {
    id: raw.id as Uuid,
    role: raw.role_id as Uuid,
    subjectType: raw.user_id ? "user" : "group",
    subject: (raw.user_id ?? raw.group_id ?? "") as Uuid,
  };
}

export async function listGrants(q?: AccessListQuery): Promise<Paginated<Grant>> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/admin/access-control/role-assignments", {
      params: { query: buildAccessListParams(q) },
    }),
  );
  return toList(raw, toGrant);
}

export async function createGrant(body: Omit<Grant, "id">): Promise<Grant> {
  const payload: components["schemas"]["RoleAssignmentInput"] = {
    role_id: assertUuid(body.role, "role_id"),
  };
  if (body.subjectType === "user")
    payload.user_id = assertUuid(body.subject, "user_id");
  else payload.group_id = assertUuid(body.subject, "group_id");
  const raw = unwrap(
    await apiClient.POST("/api/v1/admin/access-control/role-assignments", {
      body: payload,
    }),
  );
  return toGrant(raw);
}

export async function deleteGrant(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/admin/access-control/role-assignments/{id}", {
    params: { path: { id } },
  });
}

// ── Exhaustive label/picker caches ───────────────────────
//
// The single-page list endpoints above back the paginated tables. The policy
// editor and grant picker instead need EVERY subject resolvable by id — a
// subject on server page 2+ would otherwise render as a bare short-id (the
// label cache never saw it). These loop to completion, mirroring inventory's
// `listAllResources`: a fixed page size, stopping once the running offset
// reaches the server total (empty/short page guards a runaway loop).

/** Per-request page size for the exhaustive loops. Admin-scale entities
 *  (users/groups/roles) are bounded in the hundreds, so a single large page is
 *  usually enough; the loop just guarantees completeness past that.
 *  (Known tradeoff: for very large tenants the right shape is batch-resolve by
 *  referenced id, not load-everything — deferred until that scale is real.) */
const ACCESS_LIST_PAGE_SIZE = 500;

/** Every user, paged to completion (label + subject-picker cache). */
export async function listAllUsers(
  q?: Omit<AccessListQuery, "limit" | "offset">,
): Promise<UserRow[]> {
  return collectAllPages(
    (limit, offset) => listUsers({ ...q, limit, offset }),
    ACCESS_LIST_PAGE_SIZE,
  );
}

/** Every group, paged to completion. */
export async function listAllGroups(
  q?: Omit<AccessListQuery, "limit" | "offset">,
): Promise<Group[]> {
  return collectAllPages(
    (limit, offset) => listGroups({ ...q, limit, offset }),
    ACCESS_LIST_PAGE_SIZE,
  );
}

/** Every role, paged to completion. */
export async function listAllRoles(
  q?: Omit<AccessListQuery, "limit" | "offset">,
): Promise<Role[]> {
  return collectAllPages(
    (limit, offset) => listRoles({ ...q, limit, offset }),
    ACCESS_LIST_PAGE_SIZE,
  );
}

// ── Service principals (subject source for the policy picker) ─────────────
//
// Service principals live in the playbooks context; the policy subject picker
// reuses that list endpoint (mirrors how AccessDataContext pulls sites/tenants
// from the inventory feature). Only the id/name/slug needed for the picker and
// label resolution are projected. Paged to completion for the same reason as
// the user/group/role caches: a subject principal on page 2+ must still resolve
// to its name, not a bare short-id.
export async function listServicePrincipals(): Promise<ServicePrincipalRow[]> {
  const all = await collectAllPages(
    (limit, offset) => listPlaybookServicePrincipals({ limit, offset }),
    ACCESS_LIST_PAGE_SIZE,
  );
  return all.map((sp) => ({ id: sp.id, name: sp.name, slug: sp.slug }));
}

// ── Permission vocabulary ────────────────────────────────
//
// The single-source-of-truth permission dictionary (every Action, every
// ResourceType with its governance flags). Authenticated-only, not admin-gated:
// the policy editor and the token scope picker both render from it. Returned
// verbatim (both `actions` and `resource_type` are SCREAMING_SNAKE on the wire);
// consumers lowercase resource types to match the existing i18n key convention.
export type AccessVocabulary = components["schemas"]["AccessVocabularyResponse"];

export async function getAccessVocabulary(): Promise<AccessVocabulary> {
  return unwrap(await apiClient.GET("/api/v1/access-control/vocabulary", {}));
}

// ── Access check ─────────────────────────────────────────

export async function checkAccess(body: AccessCheckRequest): Promise<AccessCheckResponse> {
  const raw: BackendAccessCheckResult = unwrap(
    await apiClient.POST("/api/v1/admin/access-control/access-evaluations", {
      body: {
        user_id: assertUuid(body.user, "user_id"),
        action: body.action,
        // AccessEvaluationRequest requires resource_type; an unspecified UI
        // resource type maps to the empty string the backend treats as "any".
        resource_type: body.resourceType ?? "",
      },
    }),
  );
  const all: BackendMatchedPolicy[] = [...(raw.matched_allow ?? []), ...(raw.matched_deny ?? [])];
  const roles = new Set<string>();
  const groups = new Set<string>();
  for (const p of all) {
    for (const r of p.via_roles ?? []) roles.add(r);
    for (const s of p.via_subjects ?? []) groups.add(s);
  }
  return {
    decision: (raw.decision as "allow" | "deny") ?? "deny",
    matchedPolicies: all.map((p) => p.policy_name ?? p.policy_id),
    viaRoles: Array.from(roles),
    viaGroups: Array.from(groups),
  };
}

// ── SSO / OIDC settings ──────────────────────────────────

// OidcSettingsResponse returns client_secret_state ({ enabled, has_value }) and
// provisioning_mode; the secret value itself is never returned (write-only). The
// UI projection (SsoConfig) seeds clientSecret to "" on read and surfaces scopes
// as a space-joined string.
type BackendOidcSettings = components["schemas"]["OidcSettingsResponse"];

function toSsoConfig(raw: BackendOidcSettings): SsoConfig {
  return {
    enabled: raw.enabled,
    issuerUrl: raw.issuer_url,
    clientId: raw.client_id,
    redirectUrl: raw.redirect_url,
    scopes: raw.scopes.join(" "),
    groupClaim: raw.group_claim,
    provisioningMode: raw.provisioning_mode,
    clientSecretEnabled: raw.client_secret_state.enabled,
  };
}

export async function getSsoConfig(): Promise<SsoConfig> {
  const raw = unwrap(await apiClient.GET("/api/v1/admin/auth/oidc/settings", {}));
  return toSsoConfig(raw);
}

export async function updateSsoConfig(body: Partial<SsoConfig>): Promise<SsoConfig> {
  const raw = unwrap(
    // OidcSettingsInput requires every field, but the settings form saves a
    // partial patch over the persisted config (with `?? default` fallbacks), so
    // the assembled body can omit fields the user did not touch. The single
    // assertion bridges that intentional partial-save shape.
    await apiClient.PUT("/api/v1/admin/auth/oidc/settings", {
      body: {
        enabled: body.enabled,
        issuer_url: body.issuerUrl,
        client_id: body.clientId,
        // Enable secret auth when a new secret is entered, otherwise preserve the
        // current state (round-tripped via SsoConfig); never reset on a UI save.
        client_secret_enabled: body.clientSecret
          ? true
          : (body.clientSecretEnabled ?? false),
        client_secret_value: body.clientSecret,
        provisioning_mode: body.provisioningMode ?? "manual",
        redirect_url: body.redirectUrl,
        scopes: body.scopes ? body.scopes.split(/\s+/).filter(Boolean) : undefined,
        group_claim: body.groupClaim,
      } as components["schemas"]["OidcSettingsInput"],
    }),
  );
  return toSsoConfig(raw);
}

export async function testSsoConnection(): Promise<{ ok: boolean; message?: string }> {
  return unwrap(
    await apiClient.POST("/api/v1/admin/auth/oidc/test-connection", {}),
  );
}
