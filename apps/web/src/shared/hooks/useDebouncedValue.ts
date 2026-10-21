import { useEffect, useState } from "react";

/** Default delay: long enough to swallow a burst of typing, short enough not to feel laggy. */
export const DEFAULT_DEBOUNCE_MS = 250;

/**
 * The value, held back until it stops changing for `delayMs`.
 *
 * Feed this — not the raw input state — into a query key, so typing produces
 * one request instead of one per keystroke. The same twelve-line effect had
 * been copy-pasted into a dozen feature files while `shared/hooks` had no
 * debounce at all; two filter inputs in the IPAM workspace had simply been
 * missed, and fired a request per character.
 */
export function useDebouncedValue<T>(value: T, delayMs: number = DEFAULT_DEBOUNCE_MS): T {
  const [debounced, setDebounced] = useState(value);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);

  return debounced;
}
