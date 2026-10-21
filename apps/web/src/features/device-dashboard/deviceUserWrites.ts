import { ApiError } from "@/shared/api/client";
import type { DeviceNewUser, PreviewDeviceUsersBody } from "@/features/device-dashboard/api";

/**
 * A pending single-device user write awaiting a dry-run preview + explicit
 * confirm. `nonce` makes each request unique so repeating an identical action
 * re-runs the preview instead of reusing the previous verdict/outcome.
 */
export type PendingDeviceUserWrite =
  | { kind: "create"; title: string; nonce: number; users: DeviceNewUser[] }
  | { kind: "delete"; title: string; nonce: number; usernames: string[] };

/** The dry-run request body for a pending write. */
export function toPreviewBody(pending: PendingDeviceUserWrite): PreviewDeviceUsersBody {
  return pending.kind === "create"
    ? { op: "create", users: pending.users }
    : { op: "delete", usernames: pending.usernames };
}

/** True when an error is an RBAC denial (the caller lacks the cctv write permission). */
export function isPermissionDeniedError(error: unknown): boolean {
  return error instanceof ApiError && error.status === 403;
}
