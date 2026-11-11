/**
 * Sort/filter metadata for the access-control {@link DataTable}s (users, roles,
 * groups, grants, policies).
 *
 * The shared table owns the sort/filter UX; this module is the consumer
 * contract per resource: which rendered column keys the backend can sort on,
 * the column-key -> backend sort key map where they differ, and which columns
 * expose a multi-value checkbox filter (with the exact `IN` query param and
 * where the option list comes from — static enum or an FK list from
 * `useAccessData()`).
 *
 * Single source of truth — the backend allowlists (see `schema.gen.ts`):
 *   users:    sort username,email,status,auth_source; IN status, auth_source
 *   roles:    sort name,description
 *   groups:   sort name,external_id
 *   grants:   sort created,role,subject; IN role_id,user_id,group_id
 *   policies: sort name,effect,enabled; IN effect, enabled
 */

/** A static enum option backing a checkbox filter. `label` is an i18n key
 *  resolved by the caller. */
export interface AccessFilterEnumValue {
  value: string;
  /** i18n key (resolved with `t`) for the option label. */
  label: string;
}

/** Where an FK filter column's option list comes from (a `useAccessData()`
 *  collection), mapped to `{ value: id, label }`. */
export type AccessFkSource = "users" | "groups" | "roles";

export interface AccessFilterContext {
  /** Grants split the `subject` picker into `user_id` and `group_id` buckets. */
  subjectUserIds?: Set<string>;
}

/** One filterable column: the table column `key`, the backend `IN` query param
 *  its selected values feed, and where the option list comes from.
 *  - `type: "enum"` — static {@link AccessFilterEnumValue} list (no fetch).
 *  - `type: "fk"`   — options from a `useAccessData()` collection.
 *  - `buildParams`  — optional custom router when one column fans out into
 *    multiple backend params (`grants.subject` → `user_id` / `group_id`). */
export type AccessFilterColumn =
  | { colKey: string; param: string; type: "enum"; enumValues: AccessFilterEnumValue[] }
  | {
      colKey: string;
      param?: string;
      type: "fk";
      fkSource: AccessFkSource | readonly AccessFkSource[];
      buildParams?: (
        values: string[],
        context: AccessFilterContext,
      ) => Record<string, string[]>;
    };

/** Sort + filter capability for one access-control resource table.
 *
 * Where a rendered column key must equal a backend sort key that differs from
 * the natural camelCase field (users `auth_source`, groups `external_id`), the
 * column is simply rendered under the backend key directly — there is no
 * column-key override map, so the key lives in exactly one place per table (the
 * column definition + `sortKeys`). */
export interface AccessTableConfig {
  /** Column keys that get a sort affordance. */
  sortKeys: string[];
  /** Columns that expose a checkbox filter. */
  filterColumns: AccessFilterColumn[];
}

const USER_STATUS_OPTIONS: AccessFilterEnumValue[] = [
  { value: "active", label: "accessControl.users.statuses.active" },
  { value: "inactive", label: "accessControl.users.statuses.inactive" },
  { value: "suspended", label: "accessControl.users.statuses.suspended" },
];

const USER_AUTH_SOURCE_OPTIONS: AccessFilterEnumValue[] = [
  { value: "local", label: "accessControl.users.authSources.local" },
  { value: "oidc", label: "accessControl.users.authSources.oidc" },
];

const POLICY_EFFECT_OPTIONS: AccessFilterEnumValue[] = [
  { value: "allow", label: "accessControl.policies.effectAllow" },
  { value: "deny", label: "accessControl.policies.effectDeny" },
];

const POLICY_ENABLED_OPTIONS: AccessFilterEnumValue[] = [
  { value: "true", label: "accessControl.policies.enabledTrue" },
  { value: "false", label: "accessControl.policies.enabledFalse" },
];

const GRANT_SUBJECT_TYPE_OPTIONS: AccessFilterEnumValue[] = [
  { value: "user", label: "accessControl.grants.subjectUser" },
  { value: "group", label: "accessControl.grants.subjectGroup" },
];

/**
 * Users: backend keys username, email, status, roles, auth_source. The auth
 * column is rendered under the `auth_source` key directly so its sort/filter
 * matches the backend. `roles` sorts through the backend role-name aggregate.
 */
export const USERS_TABLE_CONFIG: AccessTableConfig = {
  sortKeys: ["username", "email", "status", "roles", "auth_source"],
  filterColumns: [
    { colKey: "status", param: "status", type: "enum", enumValues: USER_STATUS_OPTIONS },
    { colKey: "auth_source", param: "auth_source", type: "enum", enumValues: USER_AUTH_SOURCE_OPTIONS },
  ],
};

/** Roles: backend sorts on persisted `name`, `description`, and the policy
 *  reference count aggregate. */
export const ROLES_TABLE_CONFIG: AccessTableConfig = {
  sortKeys: ["name", "description", "policyCount"],
  filterColumns: [],
};

/** Groups: backend keys name, external_id, assignments. The external-id column
 *  is rendered under the `external_id` key directly. */
export const GROUPS_TABLE_CONFIG: AccessTableConfig = {
  sortKeys: ["name", "external_id", "assignments"],
  filterColumns: [],
};

/**
 * Grants (role-assignments): backend keys created, role, subject, type. The
 * `role` column carries the `role` sort key and a `role_id` FK filter. The
 * `subject` column carries the `subject` sort key and routes one UI picker into
 * the backend `user_id` / `group_id` buckets via `buildParams`. `type` sorts
 * and filters through the backend subject discriminator. `created` is
 * backend-sortable but currently has no rendered UI column. `arrow`/`actions`
 * are decorative controls → no sort, no filter.
 */
export const GRANTS_TABLE_CONFIG: AccessTableConfig = {
  sortKeys: ["role", "subject", "type"],
  filterColumns: [
    { colKey: "role", param: "role_id", type: "fk", fkSource: "roles" },
    {
      colKey: "subject",
      type: "fk",
      fkSource: ["users", "groups"],
      buildParams: (values, context) => {
        const userIds = context.subjectUserIds ?? new Set<string>();
        const userBucket = values.filter((value) => userIds.has(value));
        const groupBucket = values.filter((value) => !userIds.has(value));
        return {
          ...(userBucket.length > 0 ? { user_id: userBucket } : {}),
          ...(groupBucket.length > 0 ? { group_id: groupBucket } : {}),
        };
      },
    },
    { colKey: "type", param: "subject_type", type: "enum", enumValues: GRANT_SUBJECT_TYPE_OPTIONS },
  ],
};

/**
 * Policies: backend keys name, effect, enabled, subjects, policy_actions,
 * scopes. The `effect` column is a static allow/deny enum; the `enabled` column
 * (rendered as a Switch) is a true/false enum.
 */
export const POLICIES_TABLE_CONFIG: AccessTableConfig = {
  sortKeys: ["name", "effect", "enabled", "subjects", "policy_actions", "scopes"],
  filterColumns: [
    { colKey: "effect", param: "effect", type: "enum", enumValues: POLICY_EFFECT_OPTIONS },
    { colKey: "enabled", param: "enabled", type: "enum", enumValues: POLICY_ENABLED_OPTIONS },
  ],
};
