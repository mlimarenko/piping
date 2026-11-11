/**
 * Read/write helpers for the ONLY secret-declaration channel a playbook has:
 * `token_policy.runtime.secrets.env = { ENV_NAME: secret_key }` on a version.
 *
 * `token_policy` is an opaque JSON blob (`unknown` in the generated schema) and
 * the secrets editor is the FIRST UI surface to touch it, so every read guards
 * against arbitrary shapes and every write deep-merges — sibling `token_policy`
 * keys (and other `runtime` keys) must survive untouched.
 */

/** One declared `ENV_NAME -> secret key` row, with a stable id for React keys. */
export interface SecretEnvRow {
  id: string;
  envName: string;
  /** The named-secret `key` (a string ref, never the uuid). */
  key: string;
}

/** ENV names must be shell-safe uppercase identifiers. */
export const ENV_NAME_PATTERN = /^[A-Z_][A-Z0-9_]*$/;

/** Backend caps the map at 32 entries. */
export const MAX_SECRET_ENV = 32;

/** Prefixes/names the runtime reserves; declaring them is rejected server-side. */
const RESERVED_ENV_PREFIXES = ["PIPING_", "ANSIBLE_", "SSH_"] as const;
const RESERVED_ENV_NAMES = ["HOME", "PATH"] as const;

export function isReservedEnvName(name: string): boolean {
  const upper = name.toUpperCase();
  if (RESERVED_ENV_NAMES.includes(upper as (typeof RESERVED_ENV_NAMES)[number])) {
    return true;
  }
  return RESERVED_ENV_PREFIXES.some((prefix) => upper.startsWith(prefix));
}

export type SecretEnvNameError = "empty" | "pattern" | "reserved" | null;

/** Validate a single ENV name; returns a stable error token (or null when ok). */
export function validateEnvName(name: string): SecretEnvNameError {
  const trimmed = name.trim();
  if (!trimmed) return "empty";
  if (!ENV_NAME_PATTERN.test(trimmed)) return "pattern";
  if (isReservedEnvName(trimmed)) return "reserved";
  return null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

let rowSeq = 0;
function nextRowId(): string {
  rowSeq += 1;
  return `secret-env-${rowSeq}`;
}

/**
 * Extract the declared `ENV_NAME -> key` rows from a version's `token_policy`.
 * Any non-conforming shape (missing keys, non-object, non-string values)
 * degrades to an empty list rather than throwing.
 */
export function readSecretEnv(tokenPolicy: unknown): SecretEnvRow[] {
  if (!isRecord(tokenPolicy)) return [];
  const runtime = tokenPolicy.runtime;
  if (!isRecord(runtime)) return [];
  const secrets = runtime.secrets;
  if (!isRecord(secrets)) return [];
  const env = secrets.env;
  if (!isRecord(env)) return [];
  const rows: SecretEnvRow[] = [];
  for (const [envName, key] of Object.entries(env)) {
    if (typeof key === "string") {
      rows.push({ id: nextRowId(), envName, key });
    }
  }
  return rows;
}

/**
 * Collapse editor rows into the canonical `{ ENV_NAME: key }` map, dropping
 * blank rows. On a duplicate ENV name the last row wins (mirrors object-literal
 * semantics), so the UI's duplicate warning is advisory only.
 */
export function serializeSecretEnv(rows: SecretEnvRow[]): Record<string, string> {
  const env: Record<string, string> = {};
  for (const row of rows) {
    const envName = row.envName.trim();
    const key = row.key.trim();
    if (!envName || !key) continue;
    env[envName] = key;
  }
  return env;
}

/**
 * Deep-merge the secret env map into an existing (opaque) `token_policy`,
 * preserving every sibling key. Passing an empty map still sets
 * `runtime.secrets.env = {}` so a cleared declaration is persisted.
 */
export function writeSecretEnv(
  tokenPolicy: unknown,
  rows: SecretEnvRow[],
): Record<string, unknown> {
  const base = isRecord(tokenPolicy) ? tokenPolicy : {};
  const runtime = isRecord(base.runtime) ? base.runtime : {};
  const secrets = isRecord(runtime.secrets) ? runtime.secrets : {};
  return {
    ...base,
    runtime: {
      ...runtime,
      secrets: {
        ...secrets,
        env: serializeSecretEnv(rows),
      },
    },
  };
}

/** True when the row set declares the same env map already on `token_policy`. */
export function secretEnvUnchanged(
  tokenPolicy: unknown,
  rows: SecretEnvRow[],
): boolean {
  const current = serializeSecretEnv(readSecretEnv(tokenPolicy));
  const next = serializeSecretEnv(rows);
  const currentKeys = Object.keys(current);
  const nextKeys = Object.keys(next);
  if (currentKeys.length !== nextKeys.length) return false;
  return currentKeys.every((envName) => current[envName] === next[envName]);
}
