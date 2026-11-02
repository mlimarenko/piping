import { useEffect, useState } from "react";

const INSPECTOR_QUERY_DEFER_MS = 80;

export function useDeferredInspectorQueries(resetKey: string): boolean {
  const [readyKey, setReadyKey] = useState<string | null>(null);

  useEffect(() => {
    const timeout = window.setTimeout(() => setReadyKey(resetKey), INSPECTOR_QUERY_DEFER_MS);
    return () => window.clearTimeout(timeout);
  }, [resetKey]);

  return readyKey === resetKey;
}
