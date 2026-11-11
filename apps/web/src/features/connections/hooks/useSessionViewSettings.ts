import { useCallback, useSyncExternalStore } from "react";
import {
  DEFAULT_SESSION_VIEW,
  loadSessionViewSettings,
  subscribeSessionViewSettings,
  type SessionViewSettings,
} from "@/features/connections/sessionViewPreferences";

/** Subscribe a component to the per-host session view preference. */
export function useSessionViewSettings(
  hostId: string,
  protocol: string,
): SessionViewSettings {
  const subscribe = useCallback(
    (listener: () => void) => subscribeSessionViewSettings(hostId, protocol, listener),
    [hostId, protocol],
  );
  const getSnapshot = useCallback(
    () => loadSessionViewSettings(hostId, protocol),
    [hostId, protocol],
  );
  return useSyncExternalStore(subscribe, getSnapshot, () => DEFAULT_SESSION_VIEW);
}
