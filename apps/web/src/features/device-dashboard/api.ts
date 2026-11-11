import { apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";

// ── Backend DTO aliases (generated contract) ─────────────
//
// Read-only device-management data pulled from a connection target that is a
// video-recorder / NVR. Every call is a plain GET keyed by the target id.

export type DeviceInfo = components["schemas"]["DeviceInfoResponse"];
export type DeviceDisk = components["schemas"]["DeviceDiskResponse"];
export type DeviceRecording = components["schemas"]["DeviceRecordingResponse"];
export type DeviceRecordingSearch = components["schemas"]["DeviceRecordingSearchResponse"];
export type DeviceRecordSpan = components["schemas"]["DeviceRecordSpanResponse"];
export type DeviceUsers = components["schemas"]["DeviceUsersResponse"];
export type DeviceAuthorities = components["schemas"]["DeviceAuthoritiesResponse"];

/** The device's static capability set + the current device-write flag state. */
export type DeviceCapabilities = components["schemas"]["DeviceCapabilitiesResponse"];
/** The device's camera/input channels. */
export type DeviceChannels = components["schemas"]["DeviceChannelsResponse"];
/** One camera/input channel. */
export type DeviceChannel = components["schemas"]["DeviceChannelResponse"];
/** A pan/tilt/zoom control command to issue against a channel. */
export type PtzCommand = components["schemas"]["PtzCommandRequest"];

// ── Write DTO aliases (device user management) ───────────
//
// A single-device write always goes through a dry-run PREVIEW first: the caller
// classifies each requested name (`create`/`skip`/`delete`/`missing`) and only
// then applies via `:create` / `:delete`. Preview responses carry
// `writes_enabled` — the caller's per-target RBAC verdict for the write — so
// the UI can surface a no-permission state instead of a scary failure.

/** One account to provision (password is write-only, never read back). */
export type DeviceNewUser = components["schemas"]["DeviceNewUserRequest"];
/** One partial edit for an existing account. */
export type DeviceUserUpdate = components["schemas"]["UserUpdateRequest"];
/** Dry-run request describing a create/delete write to classify. */
export type PreviewDeviceUsersBody = components["schemas"]["PreviewDeviceUsersRequest"];
/** Dry-run result: per-name verdicts plus whether a real write is permitted. */
export type DeviceUsersPreview = components["schemas"]["DeviceUsersPreviewResponse"];
/** One verdict in a preview (`action` in request order, with a `reason`). */
export type DeviceUserPreviewItem = components["schemas"]["DeviceUserPreviewItemResponse"];
/** What a preview determined would happen to one requested name. */
export type DeviceUserPreviewAction = components["schemas"]["DeviceUserPreviewActionResponse"];
/** Per-account outcome of an applied write, preserving partial success. */
export type DeviceUsersOutcome = components["schemas"]["DeviceUsersBulkOutcomeResponse"];

// ── Read endpoints ───────────────────────────────────────

/**
 * The device's static capability set + write-flag state — the control shape the
 * UI renders from. Resolving does not contact the device, so this returns even
 * for an offline device (the UI shows disabled controls, not a blank error).
 */
export async function getDeviceCapabilities(targetId: string): Promise<DeviceCapabilities> {
  return unwrap(
    await apiClient.GET("/api/v1/cctv/{target_id}/capabilities", {
      params: { path: { target_id: targetId } },
    }),
  );
}

/** The device's camera/input channels. */
export async function getDeviceChannels(targetId: string): Promise<DeviceChannels> {
  return unwrap(
    await apiClient.GET("/api/v1/cctv/{target_id}/channels", {
      params: { path: { target_id: targetId } },
    }),
  );
}

/** Device identity, firmware, and clock. */
export async function getDeviceInfo(targetId: string): Promise<DeviceInfo> {
  return unwrap(
    await apiClient.GET("/api/v1/cctv/{target_id}/info", {
      params: { path: { target_id: targetId } },
    }),
  );
}

/** The device's user accounts. */
export async function getDeviceUsers(targetId: string): Promise<DeviceUsers> {
  return unwrap(
    await apiClient.GET("/api/v1/device-fleet/{target_id}/users", {
      params: { path: { target_id: targetId } },
    }),
  );
}

/** Verbatim authority tokens accepted by this device for user accounts. */
export async function getDeviceAuthorities(targetId: string): Promise<DeviceAuthorities> {
  return unwrap(
    await apiClient.GET("/api/v1/cctv/{target_id}/authorities", {
      params: { path: { target_id: targetId } },
    }),
  );
}

/** The device's disk/storage health. */
export async function getDeviceDisk(targetId: string): Promise<DeviceDisk> {
  return unwrap(
    await apiClient.GET("/api/v1/cctv/{target_id}/disk", {
      params: { path: { target_id: targetId } },
    }),
  );
}

/** The device's recording state and available footage window. */
export async function getDeviceRecording(targetId: string): Promise<DeviceRecording> {
  return unwrap(
    await apiClient.GET("/api/v1/cctv/{target_id}/recording", {
      params: { path: { target_id: targetId } },
    }),
  );
}

/**
 * Per-channel recording search over an arbitrary window (up to the server's
 * 30-day search cap — far wider than the 2-hour export/playback window),
 * used to render the honest per-channel timeline rather than the device-wide
 * `/recording` aggregate.
 */
export async function searchDeviceRecordings(
  targetId: string,
  window: { channel: number; startIso: string; endIso: string },
): Promise<DeviceRecordingSearch> {
  return unwrap(
    await apiClient.GET("/api/v1/cctv/{target_id}/recordings", {
      params: {
        path: { target_id: targetId },
        query: { channel: window.channel, start: window.startIso, end: window.endIso },
      },
    }),
  );
}

// ── Write endpoints (preview → apply) ────────────────────

/**
 * Dry-run a user-management write without mutating the device. Preview and
 * apply use the same per-target `device_users:connection_target` permission.
 */
export async function previewDeviceUsers(
  targetId: string,
  body: PreviewDeviceUsersBody,
): Promise<DeviceUsersPreview> {
  return unwrap(
    await apiClient.POST("/api/v1/device-fleet/{target_id}/users:preview", {
      params: { path: { target_id: targetId } },
      body,
    }),
  );
}

/**
 * Provision user accounts on the device. Requires
 * `device_users:connection_target`; partial success is preserved in the outcome.
 */
export async function createDeviceUsers(
  targetId: string,
  users: DeviceNewUser[],
): Promise<DeviceUsersOutcome> {
  return unwrap(
    await apiClient.POST("/api/v1/device-fleet/{target_id}/users:create", {
      params: { path: { target_id: targetId } },
      body: { users },
    }),
  );
}

export async function updateDeviceUsers(
  targetId: string,
  users: DeviceUserUpdate[],
): Promise<DeviceUsersOutcome> {
  return unwrap(
    await apiClient.PUT("/api/v1/device-fleet/{target_id}/users", {
      params: { path: { target_id: targetId } },
      body: { users },
    }),
  );
}

/**
 * Remove user accounts from the device by login name. Same gating as
 * {@link createDeviceUsers}; returns a per-account outcome.
 */
export async function deleteDeviceUsers(
  targetId: string,
  usernames: string[],
): Promise<DeviceUsersOutcome> {
  return unwrap(
    await apiClient.POST("/api/v1/device-fleet/{target_id}/users:delete", {
      params: { path: { target_id: targetId } },
      body: { usernames },
    }),
  );
}

// ── PTZ control ──────────────────────────────────────────

/**
 * Issue a pan/tilt/zoom command against a channel. Requires `cctv:ptz`; a
 * continuous `move` is halted by a later `stop`.
 */
export async function sendPtzCommand(targetId: string, command: PtzCommand): Promise<void> {
  // `unwrap` throws on a non-2xx envelope; the 204 success body is discarded.
  unwrap(
    await apiClient.POST("/api/v1/cctv/{target_id}/ptz", {
      params: { path: { target_id: targetId } },
      body: command,
    }),
  );
}
