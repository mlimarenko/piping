/**
 * A string that is statically known to be an entity id (UUID).
 *
 * This is a *branded* (a.k.a. opaque) type: the `__brand` tag exists only in the
 * type system and is fully erased at runtime, so a `Uuid` is just a `string` at
 * runtime with zero behavioural cost. The brand makes the compiler reject the
 * "bound a display name where an id was expected" bug class — a plain `string`
 * (e.g. a `name`/`slug`) is not assignable to a `Uuid` parameter, while a `Uuid`
 * is still assignable wherever a `string` is wanted.
 *
 * Mint one only through {@link assertUuid} (runtime-checked) at trust boundaries,
 * or `as Uuid` where the value is already known to be an id from the typed
 * contract.
 */
export type Uuid = string & { readonly __brand: "uuid" };

/** Canonical UUID v-agnostic matcher (8-4-4-4-12 hex). */
export const UUID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** True (and narrows to {@link Uuid}) when `value` is a canonical UUID string. */
export function isUuid(value: unknown): value is Uuid {
  return typeof value === "string" && UUID_RE.test(value);
}

/**
 * Dev-friendly guard for places that must send an id (UUID) to the backend.
 *
 * Catches the "bound a name where an id was expected" class of bug at the
 * call site with a readable message, instead of letting it surface as an
 * opaque backend `Invalid UUID` 400 a round-trip later. Returns the value as a
 * {@link Uuid} so the brand propagates from the boundary outward.
 */
export function assertUuid(value: unknown, field: string): Uuid {
  if (!isUuid(value)) {
    throw new Error(
      `Expected a UUID for "${field}" but got ${JSON.stringify(value)}. ` +
        `This usually means a form bound a display name instead of an id.`,
    );
  }
  return value;
}
