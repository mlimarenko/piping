import React, { createContext, useCallback, useContext, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { notify } from "@/shared/lib/notifications";
import {
  createGrant,
  createGroup,
  createPolicy,
  createRole,
  createUser,
  changeUserPassword,
  deleteGrant,
  deleteGroup,
  deletePolicy,
  deleteRole,
  deleteUser,
  disablePolicy,
  enablePolicy,
  getAccessVocabulary,
  listAllGroups,
  listAllRoles,
  listAllUsers,
  listGrants,
  listPolicies,
  listServicePrincipals,
  USERS_QUERY_KEY,
  updateGroup,
  updatePolicy,
  updateRole,
  updateUser,
  type ServicePrincipalRow,
} from "@/features/access-control/api";
import { listResources } from "@/features/inventory/api";
import type {
  CreateUserRequest,
  Grant,
  Group,
  Policy,
  Role,
  SubjectRule,
  UserRow,
} from "@/shared/api/types";

export type { ActionRule, ScopeRule } from "@/shared/api/types";

export type {
  Grant,
  Group,
  Policy,
  Role,
  ServicePrincipalRow,
  SubjectRule,
  UserRow,
};

/* Catalog metadata used by policy editors. These are domain-level constants,
   not user data, so they stay in the frontend. */
export const actionCatalog = [
  {
    group: "Connections",
    actions: [
      "READ",
      "CONNECT",
      "OBSERVE",
      "EXECUTE",
      "UPDATE",
      "SECRET",
      "CCTV_VIEW",
      "CCTV_RECORDINGS",
      "CCTV_EXPORT",
      "DEVICE_USERS",
      "CCTV_SETTINGS",
      "CCTV_PTZ",
    ],
  },
  {
    group: "Inventory",
    actions: ["READ", "CREATE", "UPDATE", "DELETE", "EXECUTE"],
  },
  {
    group: "Admin",
    actions: ["READ", "CREATE", "UPDATE", "DELETE", "MANAGE_ROLES", "MANAGE_POLICIES", "VIEW_AUDIT"],
  },
  {
    group: "Assistant",
    actions: ["READ", "EXECUTE", "UPDATE", "VIEW_AUDIT"],
  },
  {
    group: "Playbooks",
    actions: ["READ", "CREATE", "UPDATE", "DELETE", "EXECUTE"],
  },
  {
    // SECRET_VARIABLE vocabulary: EXECUTE = "use in automation" (the C3 gate),
    // SECRET = "set/reveal the value", distinct from READ/UPDATE/DELETE of the
    // secret record itself.
    group: "Secrets",
    actions: ["READ", "EXECUTE", "SECRET", "UPDATE", "DELETE"],
  },
  { group: "Audit", actions: ["READ", "VIEW_AUDIT"] },
];

/* The full action list in canonical (SCREAMING_SNAKE) order. This is the
   fallback / static default used before the {@link useAccessVocabulary}
   query resolves, and the value the AccessCheck dropdown reads through the
   context. It is a superset of `actionCatalog` (which omits the reserved
   `WRITE` action from the human policy-builder groups on purpose). */
export const allActions = [
  "CREATE",
  "READ",
  "UPDATE",
  "DELETE",
  "CONNECT",
  "OBSERVE",
  "EXECUTE",
  "CCTV_VIEW",
  "CCTV_RECORDINGS",
  "CCTV_EXPORT",
  "DEVICE_USERS",
  "CCTV_SETTINGS",
  "CCTV_PTZ",
  "MANAGE_ROLES",
  "MANAGE_POLICIES",
  "VIEW_AUDIT",
  "SECRET",
];

/* Every supported resource type (lowercased wire form, in the vocabulary
   endpoint's canonical order). This is the fallback the policy editor and
   AccessCheck render until {@link useAccessVocabulary} resolves — keeping the
   list complete means nothing renders empty and the order never flickers when
   the live vocabulary confirms it. The live list is derived per-request in
   {@link AccessDataProvider}; consumers read it from `useAccessData()`. */
export const resourceTypes = [
  "connection_target",
  "connection_session",
  "connection_filter_preset",
  "user",
  "group",
  "role",
  "policy",
  "inventory",
  "config_context",
  "secret_variable",
  "audit_event",
  "keycloak",
  "webhook",
  "assistant",
  "assistant_conversation",
  "playbook",
  "playbook_version",
  "playbook_assignment",
  "playbook_target_set",
  "playbook_run_group",
  "playbook_run",
  "playbook_schedule",
  "playbook_artifact",
  "playbook_service_principal",
  "playbook_execution_environment",
  "playbook_project",
  "playbook_job_template",
  "license",
  "settings",
  "branding",
  "proxy_node",
  "monitoring",
  "backup",
  "cctv",
];

/**
 * The permission vocabulary (actions + resource types) from the single
 * source-of-truth registry. Static-ish config, so it is cached for 5 minutes.
 * The policy editor and AccessCheck derive their catalogs from it (with the
 * module-level fallbacks above until it resolves).
 */
export const useAccessVocabulary = () =>
  useQuery({
    queryKey: ["access", "vocabulary"],
    queryFn: () => getAccessVocabulary(),
    staleTime: 5 * 60 * 1000,
  });

const uid = () => Math.random().toString(36).slice(2, 8);

interface AccessDataContextType {
  users: UserRow[];
  groups: Group[];
  roles: Role[];
  policies: Policy[];
  grants: Grant[];
  servicePrincipals: ServicePrincipalRow[];
  availableSites: string[];
  availableTenants: string[];
  /** Every supported resource type (lowercased), vocabulary-driven with a
   *  static fallback. Consumed by the policy editor + AccessCheck. */
  resourceTypes: string[];
  /** Every action (SCREAMING_SNAKE), vocabulary-driven with a static fallback. */
  allActions: string[];
  /** Per resource type (lowercased), the object-attribute keys a policy
   *  constraint may scope on. Empty array = not attribute-scopable. Empty map
   *  until the vocabulary resolves. Consumed by the policy constraint builder. */
  constraintAttributeKeys: Record<string, string[]>;
  isPending: boolean;

  // Users
  createUser: (body: CreateUserRequest) => Promise<void>;
  updateUser: (id: string, body: Omit<UserRow, "id">) => Promise<void>;
  changeUserPassword: (id: string, newPassword: string) => Promise<void>;
  deleteUser: (id: string) => Promise<void>;

  // Groups
  createGroup: (body: Omit<Group, "id">) => Promise<void>;
  updateGroup: (id: string, body: Omit<Group, "id">) => Promise<void>;
  deleteGroup: (id: string) => Promise<void>;

  // Roles
  createRole: (body: Omit<Role, "id" | "policyCount">) => Promise<void>;
  updateRole: (id: string, body: Omit<Role, "id" | "policyCount">) => Promise<void>;
  deleteRole: (id: string) => Promise<void>;

  // Policies
  createPolicy: (body: Omit<Policy, "id">) => Promise<void>;
  updatePolicy: (id: string, body: Omit<Policy, "id">) => Promise<void>;
  deletePolicy: (id: string) => Promise<void>;
  enablePolicy: (id: string) => Promise<void>;
  disablePolicy: (id: string) => Promise<void>;

  // Grants
  createGrant: (body: Omit<Grant, "id">) => Promise<void>;
  deleteGrant: (id: string) => Promise<void>;

  uid: () => string;
}

const AccessDataContext = createContext<AccessDataContextType | null>(null);

export const useAccessData = () => {
  const ctx = useContext(AccessDataContext);
  if (!ctx) throw new Error("useAccessData must be used within AccessDataProvider");
  return ctx;
};

/**
 * Resolvers that turn raw role/subject ids (or names) into `{ id, name }` pairs
 * for `<EntityRef>`. Grants can carry either a UUID id or an already-human name
 * depending on how they were created, so each resolver looks up by id first,
 * then by name, and leaves `name` undefined when nothing matches (EntityRef then
 * renders a graceful short-id instead of a raw UUID).
 */
export const useAccessLabels = () => {
  const { roles, users, groups, servicePrincipals } = useAccessData();
  return useMemo(() => {
    // These lookups are keyed by *either* a bare id or a display name (callers
    // pass a free `string` that may be either), so the key type is widened to
    // `string` — a branded id `Uuid` is still a valid `string` key.
    const roleById = new Map<string, (typeof roles)[number]>(roles.map((r) => [r.id, r]));
    const roleByName = new Map(roles.map((r) => [r.name, r] as const));
    const userById = new Map<string, (typeof users)[number]>(users.map((u) => [u.id, u]));
    const userByName = new Map(users.map((u) => [u.username, u] as const));
    const groupById = new Map<string, (typeof groups)[number]>(groups.map((g) => [g.id, g]));
    const groupByName = new Map(groups.map((g) => [g.name, g] as const));
    const spById = new Map(servicePrincipals.map((s) => [s.id, s] as const));
    const role = (value: string): { id: string; name?: string } => {
      const r = roleById.get(value) ?? roleByName.get(value);
      return { id: value, name: r?.name };
    };
    const subject = (
      value: string,
      type: "user" | "group",
    ): { id: string; name?: string } => {
      if (type === "user") {
        const u = userById.get(value) ?? userByName.get(value);
        return { id: value, name: u?.username };
      }
      const g = groupById.get(value) ?? groupByName.get(value);
      return { id: value, name: g?.name };
    };
    return {
      role,
      subject,
      /**
       * Kind-aware resolver for a canonical policy subject. `rule.id` is the bare
       * entity id (uuid, or the external name for `oidc_group`); resolves to the
       * display name from the loaded caches, leaving `name` undefined (EntityRef
       * then renders a short id) when nothing matches. `oidc_group` has no local
       * entity, so its external name is surfaced directly.
       */
      policySubject: (rule: SubjectRule): { id: string; name?: string } => {
        switch (rule.kind) {
          case "role":
            return role(rule.id);
          case "user":
            return subject(rule.id, "user");
          case "group":
            return subject(rule.id, "group");
          case "service_principal":
            return { id: rule.id, name: spById.get(rule.id)?.name };
          case "oidc_group":
            return { id: rule.id, name: rule.label ?? rule.id };
        }
      },
    };
  }, [roles, users, groups, servicePrincipals]);
};

export const AccessDataProvider = ({ children }: { children: React.ReactNode }) => {
  const { t } = useTranslation();
  const qc = useQueryClient();

  const handleError = useCallback(
    (action: string, err: unknown) => {
      const message =
        err instanceof Error ? err.message : t("accessControl.data.actionFailed", { action });
      notify("error", message);
    },
    [t],
  );

  const queries = useQueries({
    queries: [
      { queryKey: USERS_QUERY_KEY, queryFn: () => listAllUsers() },
      { queryKey: ["access", "groups"], queryFn: () => listAllGroups() },
      { queryKey: ["access", "roles"], queryFn: () => listAllRoles() },
      { queryKey: ["access", "policies"], queryFn: () => listPolicies() },
      { queryKey: ["access", "grants"], queryFn: () => listGrants() },
      { queryKey: ["access", "service-principals"], queryFn: () => listServicePrincipals() },
      { queryKey: ["inventory", "resources", "Sites"], queryFn: () => listResources("Sites") },
      { queryKey: ["inventory", "resources", "Tenants"], queryFn: () => listResources("Tenants") },
    ],
  });

  const [usersQ, groupsQ, rolesQ, policiesQ, grantsQ, spQ, sitesQ, tenantsQ] = queries;
  const usersData = usersQ?.data;
  const groupsData = groupsQ?.data;
  const rolesData = rolesQ?.data;
  const policiesData = policiesQ?.data;
  const grantsData = grantsQ?.data;
  const spData = spQ?.data;
  const sitesData = sitesQ?.data;
  const tenantsData = tenantsQ?.data;
  // listAll* return the fully-paged arrays directly (not a Paginated envelope),
  // so the label + subject-picker caches resolve every entity, not just page 1.
  const users: UserRow[] = useMemo(() => usersData ?? [], [usersData]);
  const groups: Group[] = useMemo(() => groupsData ?? [], [groupsData]);
  const roles: Role[] = useMemo(() => rolesData ?? [], [rolesData]);
  const policies: Policy[] = useMemo(() => policiesData?.items ?? [], [policiesData]);
  const grants: Grant[] = useMemo(() => grantsData?.items ?? [], [grantsData]);
  const servicePrincipals: ServicePrincipalRow[] = useMemo(() => spData ?? [], [spData]);
  const sitesLive = useMemo<{ name: string }[]>(() => (sitesData?.items ?? []), [sitesData]);
  const tenantsLive = useMemo<{ name: string }[]>(() => (tenantsData?.items ?? []), [tenantsData]);
  const availableSitesLive = sitesLive.map((s) => s.name).filter(Boolean);
  const availableTenantsLive = tenantsLive.map((t) => t.name).filter(Boolean);
  const isPending = queries.some((q) => q.isPending);

  // Permission vocabulary → the resource-type + action catalogs the policy
  // editor and AccessCheck render. Resource types are lowercased to match the
  // existing `accessControl.policies.resourceTypes.*` i18n keys; actions stay
  // SCREAMING_SNAKE. Fall back to the static lists when the derived list is
  // empty *or* absent (a partial/failed/empty response must never render an
  // empty picker) — mirrors the defensive envelope handling in `api.ts`.
  const vocabularyQuery = useAccessVocabulary();
  const vocabulary = vocabularyQuery.data;
  const resourceTypesLive = useMemo(() => {
    const derived = (vocabulary?.resources ?? [])
      .map((r) => r.resource_type.toLowerCase())
      .filter(Boolean);
    return derived.length > 0 ? derived : resourceTypes;
  }, [vocabulary]);
  const allActionsLive = useMemo(() => {
    const derived = (vocabulary?.actions ?? []).filter(Boolean);
    return derived.length > 0 ? derived : allActions;
  }, [vocabulary]);
  // Attribute-scoping keys per resource type, keyed by the lowercased type to
  // match the scope values the policy editor stores. Only resources the backend
  // marks scopable carry keys; everything else maps to `[]`. Empty until the
  // vocabulary resolves (the constraint builder then hides its picker).
  const constraintAttributeKeysLive = useMemo<Record<string, string[]>>(() => {
    const map: Record<string, string[]> = {};
    for (const r of vocabulary?.resources ?? []) {
      map[r.resource_type.toLowerCase()] = r.constraint_attribute_keys ?? [];
    }
    return map;
  }, [vocabulary]);

  const invalidate = useCallback(
    (key: "users" | "groups" | "roles" | "policies" | "grants") =>
      // Anchor the users invalidation on the shared base key so it prefix-covers
      // every user variant (base list, paginated table, enrichment "all" lookup).
      qc.invalidateQueries({
        queryKey: key === "users" ? USERS_QUERY_KEY : ["access", key],
      }),
    [qc],
  );

  /**
   * Collapse the near-identical CRUD mutation blocks (the plan's
   * `defineCrudMutation`): invalidate the owning collection on success, toast the
   * success message, and route errors to the shared `handleError`. `key` null
   * skips invalidation (a password change mutates no list); `notifyKey` null
   * stays silent (policy enable/disable flip no toast).
   */
  const useCrudMutation = <TVars,>(
    key: "users" | "groups" | "roles" | "policies" | "grants" | null,
    mutationFn: (vars: TVars) => Promise<unknown>,
    notifyKey: string | null,
    actionKey: string,
  ) =>
    useApiMutation({
      mutationFn,
      onSuccess: async () => {
        if (key) await invalidate(key);
        if (notifyKey) notify("success", t(`accessControl.data.${notifyKey}`));
      },
      onError: (err) => handleError(t(`accessControl.data.actions.${actionKey}`), err),
    });

  const userCreate = useCrudMutation("users", (body: CreateUserRequest) => createUser(body), "userCreated", "createUser");
  const userUpdate = useCrudMutation("users", ({ id, body }: { id: string; body: Omit<UserRow, "id"> }) => updateUser(id, body), "userUpdated", "updateUser");
  const userDelete = useCrudMutation("users", (id: string) => deleteUser(id), "userDeleted", "deleteUser");
  const userPasswordChange = useCrudMutation(null, ({ id, newPassword }: { id: string; newPassword: string }) => changeUserPassword(id, newPassword), "userPasswordChanged", "changeUserPassword");

  const groupCreate = useCrudMutation("groups", (body: Omit<Group, "id">) => createGroup(body), "groupCreated", "createGroup");
  const groupUpdate = useCrudMutation("groups", ({ id, body }: { id: string; body: Omit<Group, "id"> }) => updateGroup(id, body), "groupUpdated", "updateGroup");
  const groupDelete = useCrudMutation("groups", (id: string) => deleteGroup(id), "groupDeleted", "deleteGroup");

  const roleCreate = useCrudMutation("roles", (body: Omit<Role, "id" | "policyCount">) => createRole(body), "roleCreated", "createRole");
  const roleUpdate = useCrudMutation("roles", ({ id, body }: { id: string; body: Omit<Role, "id" | "policyCount"> }) => updateRole(id, body), "roleUpdated", "updateRole");
  const roleDelete = useCrudMutation("roles", (id: string) => deleteRole(id), "roleDeleted", "deleteRole");

  const policyCreate = useCrudMutation("policies", (body: Omit<Policy, "id">) => createPolicy(body), "policyCreated", "createPolicy");
  const policyUpdate = useCrudMutation("policies", ({ id, body }: { id: string; body: Omit<Policy, "id"> }) => updatePolicy(id, body), "policyUpdated", "updatePolicy");
  const policyDelete = useCrudMutation("policies", (id: string) => deletePolicy(id), "policyDeleted", "deletePolicy");
  const policyEnable = useCrudMutation("policies", (id: string) => enablePolicy(id), null, "enablePolicy");
  const policyDisable = useCrudMutation("policies", (id: string) => disablePolicy(id), null, "disablePolicy");

  const grantCreate = useCrudMutation("grants", (body: Omit<Grant, "id">) => createGrant(body), "grantCreated", "createGrant");
  const grantDelete = useCrudMutation("grants", (id: string) => deleteGrant(id), "grantDeleted", "deleteGrant");

  const value = useMemo<AccessDataContextType>(
    () => ({
      users,
      groups,
      roles,
      policies,
      grants,
      servicePrincipals,
      availableSites: availableSitesLive,
      availableTenants: availableTenantsLive,
      resourceTypes: resourceTypesLive,
      allActions: allActionsLive,
      constraintAttributeKeys: constraintAttributeKeysLive,
      isPending,
      uid,

      createUser: async (body) => {
        await userCreate.mutateAsync(body);
      },
      updateUser: async (id, body) => {
        await userUpdate.mutateAsync({ id, body });
      },
      changeUserPassword: async (id, newPassword) => {
        await userPasswordChange.mutateAsync({ id, newPassword });
      },
      deleteUser: async (id) => {
        await userDelete.mutateAsync(id);
      },

      createGroup: async (body) => {
        await groupCreate.mutateAsync(body);
      },
      updateGroup: async (id, body) => {
        await groupUpdate.mutateAsync({ id, body });
      },
      deleteGroup: async (id) => {
        await groupDelete.mutateAsync(id);
      },

      createRole: async (body) => {
        await roleCreate.mutateAsync(body);
      },
      updateRole: async (id, body) => {
        await roleUpdate.mutateAsync({ id, body });
      },
      deleteRole: async (id) => {
        await roleDelete.mutateAsync(id);
      },

      createPolicy: async (body) => {
        await policyCreate.mutateAsync(body);
      },
      updatePolicy: async (id, body) => {
        await policyUpdate.mutateAsync({ id, body });
      },
      deletePolicy: async (id) => {
        await policyDelete.mutateAsync(id);
      },
      enablePolicy: async (id) => {
        await policyEnable.mutateAsync(id);
      },
      disablePolicy: async (id) => {
        await policyDisable.mutateAsync(id);
      },

      createGrant: async (body) => {
        await grantCreate.mutateAsync(body);
      },
      deleteGrant: async (id) => {
        await grantDelete.mutateAsync(id);
      },
    }),
    [
      users,
      groups,
      roles,
      policies,
      grants,
      servicePrincipals,
      availableSitesLive,
      availableTenantsLive,
      resourceTypesLive,
      allActionsLive,
      constraintAttributeKeysLive,
      isPending,
      userCreate,
      userUpdate,
      userPasswordChange,
      userDelete,
      groupCreate,
      groupUpdate,
      groupDelete,
      roleCreate,
      roleUpdate,
      roleDelete,
      policyCreate,
      policyUpdate,
      policyDelete,
      policyEnable,
      policyDisable,
      grantCreate,
      grantDelete,
    ],
  );

  return <AccessDataContext.Provider value={value}>{children}</AccessDataContext.Provider>;
};
