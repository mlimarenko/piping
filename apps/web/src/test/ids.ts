import type { Uuid } from "@/shared/lib/ids";

/**
 * Mint a branded {@link Uuid} from an arbitrary test-fixture literal.
 *
 * Production code mints `Uuid`s through `assertUuid` (runtime-checked) or `as Uuid`
 * at typed-contract boundaries. Test fixtures use readable, non-canonical ids
 * (`"host-1"`, `"device-2"`) that would fail the runtime UUID check, so this is a
 * type-only mint for fixtures — never import it from production code.
 */
export const testUuid = (id: string): Uuid => id as Uuid;
