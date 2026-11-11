import { ApiError } from "@/shared/api/client";
import {
  connectionFailureFromApiError,
  type ConnectionFailure,
} from "@/features/connections/connectionFailure";

/**
 * Protocol-neutral bridge for media fetch/parse failures. Canonical API errors
 * retain their typed reason/status mapping; browser and malformed-stream errors
 * collapse to a safe network category. Raw error prose never enters UI state.
 */
export function connectionFailureFromMediaError(error: unknown): ConnectionFailure {
  return error instanceof ApiError
    ? connectionFailureFromApiError(error)
    : { kind: "network" };
}
