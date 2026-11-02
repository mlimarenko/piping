import {
  useMutation,
  type UseMutationOptions,
  type UseMutationResult,
} from "@tanstack/react-query";
import type { ApiError } from "./client";

/**
 * Canonical mutation hook for the Piping REST surface.
 *
 * TanStack Query v5 defaults `TError` to `Error`. A bare `useMutation` then
 * accepts `(err: ApiError) => …` parameter annotations as subtype-narrowing
 * **without any runtime guarantee** — if the throwing path ever yields a
 * non-`ApiError` (network abort, devtools middleware, third-party throw),
 * the handler reads `err.status`/`err.fields`/`err.code` as `undefined`
 * silently.
 *
 * `useApiMutation` makes the contract explicit: the request layer in
 * `client.ts` only ever throws `ApiError` instances (built by `parseError`),
 * so binding `TError = ApiError` here lets the type system keep the rest of
 * the app honest. `instanceof ApiError` and the strongly-typed fields are
 * available without casts.
 */
export function useApiMutation<TData = unknown, TVariables = void>(
  options: UseMutationOptions<TData, ApiError, TVariables>,
): UseMutationResult<TData, ApiError, TVariables> {
  return useMutation<TData, ApiError, TVariables>(options);
}

/**
 * Retry policy for endpoints proxied to a live external device (CCTV and
 * friends): a 5xx usually means the device or its proxy hop hiccuped and a
 * retry is cheap, while any 4xx is a definitive answer — retrying a 400/404
 * against a camera only delays the error state.
 *
 * The app-wide default (App.tsx) retries everything except 401/403/404; use
 * this stricter policy where the backend fronts hardware.
 */
export const retryOn5xx = (failureCount: number, error: unknown): boolean => {
  const status = (error as { status?: number }).status ?? 500;
  return status >= 500 && failureCount < 2;
};
