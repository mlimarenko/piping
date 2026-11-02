import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";

type Owner = { id: string; navigationToken: string };
interface Registration {
  claim: (owner: Owner) => () => void;
  blockActions: (id: string) => (invoker?: Element | null) => void;
}
interface ChromeState {
  navigationToken: string;
  owner: Owner | null;
  titleHost: HTMLDivElement | null;
  actionsHost: HTMLDivElement | null;
  actionsBlocked: boolean;
  setTitleHost: (node: HTMLDivElement | null) => void;
  setActionsHost: (node: HTMLDivElement | null) => void;
}
const RegistrationContext = createContext<Registration | null>(null);
const HostContext = createContext<ChromeState | null>(null);

/** The shell owns hosts; features retain ownership of the portal children. */
export function PageChromeProvider({ navigationToken, children }: { navigationToken: string; children: ReactNode }) {
  const [owner, setOwner] = useState<Owner | null>(null);
  const [titleHost, setTitleHost] = useState<HTMLDivElement | null>(null);
  const [actionsHost, setActionsHost] = useState<HTMLDivElement | null>(null);
  const [blockers, setBlockers] = useState<Set<string>>(() => new Set());
  const pendingFocus = useRef(new Map<string, HTMLElement>());
  const claim = useCallback((next: Owner) => {
    setOwner(next);
    return () => setOwner(current => current === next ? null : current);
  }, []);
  const blockActions = useCallback((id: string) => {
    pendingFocus.current.delete(id);
    setBlockers(current => new Set(current).add(id));
    return (invoker?: Element | null) => {
      if (invoker instanceof HTMLElement) pendingFocus.current.set(id, invoker);
      setBlockers(current => {
        const next = new Set(current);
        next.delete(id);
        return next;
      });
    };
  }, []);
  useEffect(() => {
    // A release queues state; only this post-commit effect observes the host
    // after React removed inert. Other open editors still own their blockers.
    for (const [id, invoker] of pendingFocus.current) {
      if (!invoker.isConnected) { pendingFocus.current.delete(id); continue; }
      if (actionsHost?.contains(invoker) && blockers.size > 0) continue;
      pendingFocus.current.delete(id);
      invoker.focus();
    }
  }, [actionsHost, blockers]);
  const registration = useMemo(() => ({ claim, blockActions }), [claim, blockActions]);
  const state = useMemo(() => ({ navigationToken, owner: owner?.navigationToken === navigationToken ? owner : null,
    titleHost, actionsHost, actionsBlocked: blockers.size > 0, setTitleHost, setActionsHost }),
  [navigationToken, owner, titleHost, actionsHost, blockers]);
  return <RegistrationContext.Provider value={registration}><HostContext.Provider value={state}>{children}</HostContext.Provider></RegistrationContext.Provider>;
}

export function usePageChromeRegistration() {
  const value = useContext(RegistrationContext);
  if (!value) throw new Error("PageChrome requires the shell PageChromeProvider");
  return value;
}
export function usePageChromeHosts() {
  const value = useContext(HostContext);
  if (!value) throw new Error("PageChrome requires the shell PageChromeProvider");
  return value;
}
export function usePageActionsBlocker(id: string, active: boolean, invoker?: Element | null) {
  const { blockActions } = usePageChromeRegistration();
  useLayoutEffect(() => {
    if (!active) return;
    const release = blockActions(id);
    return () => release(invoker);
  }, [active, id, blockActions, invoker]);
}
