import { useCallback, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { closeSession, getConnectionLaunchOptions, openSession } from "../api";
import type { ConnectionLaunchAccount, CredentialProtocol, OpenSessionRequest, OpenSessionResponse } from "@/shared/api/types";
import type { AdHocCredentialsContext, AdHocCredentialsSubmit } from "../components/connections/AdHocCredentialsDialog";
import { connectionAccountText, presentLaunchAccounts, sessionAccountInput, type SessionLaunchProtocol } from "../components/connections/launchPresentation";
import { createMyCredential, setDefaultMyCredential } from "@/features/user-settings/api";
import { notify } from "@/shared/lib/notifications";

export interface SessionLaunchTarget { id: string; name: string }
export interface SessionReplacement { id: string; hostId: string; hostName: string; protocol: SessionLaunchProtocol }
interface Intent<D> {
  attempt: number;
  destination: D;
  target: SessionLaunchTarget;
  protocol: SessionLaunchProtocol;
  replacementId?: string;
  accounts?: ConnectionLaunchAccount[];
  context?: AdHocCredentialsContext;
  error?: string;
  submitting?: boolean;
}

/** Owns fresh launches, never retained credentials or live-session resume. */
export function useSessionLaunch<D>(options: {
  accept: (response: OpenSessionResponse, destination: D, username?: string) => boolean;
  prepare?: (destination: D) => D;
  discard?: (destination: D) => void;
  isCurrent?: (destination: D) => boolean;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [intent, setIntent] = useState<Intent<D> | null>(null);
  const current = useRef<Intent<D> | null>(null);
  const submitted = useRef(new Map<number, { cancelled: boolean }>());
  const sequence = useRef(0);
  const returnFocus = useRef<HTMLElement | null>(null);
  const focusAttempt = useRef(0);
  const mounted = useRef(true);
  const callbacks = useRef(options);
  useEffect(() => { callbacks.current = options; });
  useEffect(() => {
    mounted.current = true;
    const attempts = submitted.current;
    return () => {
      mounted.current = false;
      current.current = null;
      for (const attempt of attempts.values()) attempt.cancelled = true;
    };
  }, []);

  const publish = useCallback((next: Intent<D> | null) => {
    current.current = next;
    setIntent(next);
  }, []);
  const cancel = useCallback((attemptId: number) => {
    const pending = submitted.current.get(attemptId);
    if (pending) pending.cancelled = true;
    if (current.current?.attempt !== attemptId) return;
    callbacks.current.discard?.(current.current.destination);
    publish(null);
  }, [publish]);

  const send = useCallback(async (request: OpenSessionRequest, source: Intent<D>, username?: string) => {
    if (submitted.current.has(source.attempt)) return false;
    if ((source.context || source.replacementId) && current.current?.attempt !== source.attempt) return false;
    if (callbacks.current.isCurrent?.(source.destination) === false) {
      if (current.current?.attempt === source.attempt) publish(null);
      return false;
    }
    const attempt = { ...source, destination: callbacks.current.prepare?.(source.destination) ?? source.destination };
    const lifecycle = { cancelled: false };
    submitted.current.set(attempt.attempt, lifecycle);
    if (current.current?.attempt === attempt.attempt) publish({ ...attempt, submitting: true, error: undefined });
    try {
      const response = await openSession(request);
      if (lifecycle.cancelled || !mounted.current || callbacks.current.isCurrent?.(attempt.destination) === false || !callbacks.current.accept(response, attempt.destination, username)) {
        // A cancelled/closed target must not be resurrected by an obsolete POST.
        void closeSession(response.session.id).catch(() => undefined);
        callbacks.current.discard?.(attempt.destination);
        if (mounted.current && current.current?.attempt === attempt.attempt) publish(null);
        return false;
      }
      if (current.current?.attempt === attempt.attempt) {
        returnFocus.current = null;
        publish(null);
      }
      if (attempt.replacementId) void closeSession(attempt.replacementId).catch(() => undefined);
      return true;
    } catch (error) {
      callbacks.current.discard?.(attempt.destination);
      if (mounted.current && current.current?.attempt === attempt.attempt) publish({ ...attempt, error: t("connections.connectionFailed") });
      throw error;
    } finally {
      submitted.current.delete(attempt.attempt);
    }
  }, [publish, t]);

  const chooseAccount = useCallback(async (attempt: Intent<D>, account: ConnectionLaunchAccount) => {
    const context = sessionAccountInput(attempt.target, attempt.protocol, account);
    if (context) { publish({ ...attempt, context }); return; }
    await send({ hostId: attempt.target.id, protocol: attempt.protocol, entryId: account.entryId }, attempt, account.username ?? undefined);
  }, [publish, send]);

  const retry = useCallback(async (session: SessionReplacement, destination: D, refresh = false) => {
    const sameTarget = current.current?.replacementId === session.id;
    if (sameTarget && !refresh) return;
    if (!sameTarget) returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const attempt: Intent<D> = {
      attempt: ++sequence.current, destination, replacementId: session.id,
      target: { id: session.hostId, name: session.hostName }, protocol: session.protocol,
    };
    focusAttempt.current = attempt.attempt;
    publish(attempt);
    try {
      const launch = await getConnectionLaunchOptions(session.hostId);
      if (current.current?.attempt !== attempt.attempt) return;
      if (callbacks.current.isCurrent?.(destination) === false) { publish(null); return; }
      const protocol = launch.protocols.find(item => item.protocol === session.protocol);
      const accounts = protocol ? presentLaunchAccounts(protocol) : [];
      if (!accounts.length) { publish({ ...attempt, accounts, error: t("connections.retryLaunch.unavailable") }); return; }
      const ready = { ...attempt, accounts };
      publish(ready);
      if (accounts.length === 1 && accounts[0]) await chooseAccount(ready, accounts[0]);
    } catch {
      if (current.current?.attempt === attempt.attempt && !current.current.error) publish({ ...attempt, error: t("connections.retryLaunch.lookupFailed") });
    }
  }, [chooseAccount, publish, t]);

  const launch = useCallback((request: Omit<OpenSessionRequest, "protocol"> & { protocol: SessionLaunchProtocol }, destination: D, username?: string) => {
    const attempt: Intent<D> = { attempt: ++sequence.current, destination, target: { id: request.hostId, name: request.hostId }, protocol: request.protocol };
    return send(request, attempt, username);
  }, [send]);

  const openCredentials = useCallback((context: AdHocCredentialsContext, destination: D) => {
    returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const attempt = ++sequence.current;
    focusAttempt.current = attempt;
    publish({ attempt, destination, target: context.host, protocol: context.protocol, context });
  }, [publish]);

  const submit = useCallback(async (input: AdHocCredentialsSubmit, attemptId: number) => {
    const attempt = current.current;
    if (!attempt || attempt.attempt !== attemptId || !attempt.context || attempt.submitting) return;
    const accepted = await send({ hostId: input.host.id, protocol: input.protocol, entryId: input.entryId, credentials: input.credentials }, attempt, input.username);
    if (!accepted || !input.makeDefault) return;
    // Only the form's explicit opt-in persists a credential. The request never
    // enters mutation variables, intent state, session state or layout storage.
    try {
      const created = await createMyCredential({
        protocol: input.protocol.toUpperCase() as CredentialProtocol,
        name: input.username?.trim() ? `${input.protocol.toUpperCase()} · ${input.username.trim()}` : input.protocol.toUpperCase(),
        isDefault: true, username: input.username, domain: input.credentials.domain,
        password: input.credentials.password, privateKey: input.credentials.private_key,
      });
      await setDefaultMyCredential(created.id);
      await queryClient.invalidateQueries({ queryKey: ["user", "credentials"] });
      await queryClient.invalidateQueries({ queryKey: ["connections", "launch-options"] });
    } catch {
      notify("error", t("connections.adHocCredentials.saveDefaultFailed"));
    }
  }, [queryClient, send, t]);

  return {
    intent, cancel, retry, launch, openCredentials, submit,
    restoreFocus: (event: { preventDefault: () => void }, attemptId: number) => {
      event.preventDefault();
      if (current.current || focusAttempt.current !== attemptId) return;
      if (returnFocus.current?.isConnected) returnFocus.current.focus({ preventScroll: true });
      returnFocus.current = null;
    },
    refresh: (attemptId: number) => {
      const attempt = current.current;
      if (!attempt || attempt.attempt !== attemptId || !attempt.replacementId) return;
      return retry({ id: attempt.replacementId, hostId: attempt.target.id, hostName: attempt.target.name, protocol: attempt.protocol }, attempt.destination, true);
    },
    choose: async (account: ConnectionLaunchAccount, attemptId: number) => {
      const attempt = current.current;
      if (!attempt || attempt.attempt !== attemptId || attempt.submitting) return;
      try { await chooseAccount(attempt, account); } catch { /* send publishes a retryable, safe error. */ }
    },
    accountText: connectionAccountText,
  };
}
