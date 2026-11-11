import { useEffect, useRef } from "react";

/** Retry on data/layout changes until revealed, then leave manual scrolling alone. */
export function useHostReveal(
  hostId: string | null | undefined,
  reveal: () => boolean,
  { layoutKey = 0, navigationKey = "" }: { layoutKey?: number; navigationKey?: string } = {},
) {
  const request = useRef<{ hostId: typeof hostId; layoutKey: number; navigationKey: string; complete: boolean }>({
    hostId: null,
    layoutKey: 0,
    navigationKey: "",
    complete: false,
  });

  useEffect(() => {
    if (request.current.hostId !== hostId || request.current.layoutKey !== layoutKey || request.current.navigationKey !== navigationKey) {
      request.current = { hostId, layoutKey, navigationKey, complete: false };
    }
    if (hostId && !request.current.complete) request.current.complete = reveal();
  }, [hostId, layoutKey, navigationKey, reveal]);
}
