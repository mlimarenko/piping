import { useCallback, useState } from "react";

/**
 * Row-level intent for the shared launch-options query. A connection view calls
 * `markIntent` when the user hovers/focuses a host row; the matching
 * QuickAccessButtons receives the timestamp and warms the canonical cache entry
 * before selection. Only one host is intentful at a time.
 */
export function useQuickAccessIntent() {
  const [intent, setIntent] = useState<{ hostId: string; startedAt: number } | null>(null);
  const markIntent = useCallback((hostId: string) => {
    setIntent({ hostId, startedAt: Date.now() });
  }, []);
  const intentStartedAt = useCallback(
    (hostId: string) => intent?.hostId === hostId ? intent.startedAt : null,
    [intent],
  );
  return { intentStartedAt, markIntent };
}
