import { ApiError } from "@/shared/api/client";

export const SESSION_RESUME_MAX_RETRIES = 6;

export function isTerminalSessionResumeError(error: unknown): boolean {
  if (!(error instanceof ApiError)) return false;
  return error.status === 400 || error.status === 401 || error.status === 403 || error.status === 404;
}

export function sessionResumeRetryDelayMs(attempt: number): number {
  return Math.min(1_000 * 2 ** attempt, 8_000);
}

/**
 * Bounded retries for the single-attach race in the detached window. The
 * backend proxy guards a connection against concurrent attach and releases the
 * lock asynchronously on socket drop (see `proxy/ws.rs` `ActiveBridgeGuard`), so
 * a relinquish-first detach can briefly race the old in-panel WS still holding
 * the lock. We re-resume for a fresh token a few times before giving up.
 */
export const SESSION_ATTACH_RACE_MAX_RETRIES = 4;

export function sessionAttachRaceRetryDelayMs(attempt: number): number {
  return Math.min(250 * 2 ** attempt, 2_000);
}
