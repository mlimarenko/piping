import { apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";

// ── Backend DTO aliases (generated contract) ─────────────

export type ConnConfRecord = components["schemas"]["ConnConfRecordResponse"];
export type ConnConfAssignment = components["schemas"]["ConnConfAssignmentResponse"];
export type EffectiveConnConfig = components["schemas"]["EffectiveConnConfigResponse"];
export type EffectiveConnConfigSource =
  components["schemas"]["EffectiveConnConfigSourceResponse"];
export type ConnConfRecordCredentials =
  components["schemas"]["ConnConfRecordCredentialsResponse"];

type CreateRecordBody = components["schemas"]["CreateConnConfRecordRequest"];
type UpdateRecordBody = components["schemas"]["UpdateConnConfRecordRequest"];
type CreateAssignmentBody = components["schemas"]["CreateConnConfAssignmentRequest"];
type UpdateAssignmentBody = components["schemas"]["UpdateConnConfAssignmentRequest"];
type BulkAssignRecordBody = components["schemas"]["BulkAssignConnConfRequest"];

export type BulkAssignRecordResponse = components["schemas"]["BulkAssignConnConfResponse"];

interface ConnConfRecordListQuery {
  protocol?: string;
  base_record_id?: string;
  owner_user_id?: string;
  enabled?: string;
  sort?: string;
  order?: "asc" | "desc";
}

interface ConnConfAssignmentListQuery {
  record_id?: string;
  scope_type?: string;
  scope_id?: string;
  enabled?: string;
  sort?: string;
  order?: "asc" | "desc";
}

// ── Protocols ────────────────────────────────────────────

export const CONNCONF_PROTOCOLS = ["ssh", "rdp", "vnc", "app", "cctv"] as const;
export type ConnConfProtocol = (typeof CONNCONF_PROTOCOLS)[number];

export function isConnConfProtocol(value: string): value is ConnConfProtocol {
  return (CONNCONF_PROTOCOLS as readonly string[]).includes(value);
}

export function protocolDefaultPort(protocol: ConnConfProtocol): number {
  if (protocol === "ssh") return 22;
  if (protocol === "rdp") return 3389;
  if (protocol === "vnc") return 5900;
  return 0;
}

// ── Scope hierarchy (12 inventory layers, least → most specific) ──

export const CONNCONF_SCOPE_TYPES = [
  "global",
  "region",
  "site_group",
  "site",
  "location",
  "rack",
  "tenant",
  "cluster",
  "role",
  "device_type",
  "platform",
  "device",
  "vm",
] as const;
export type ConnConfScopeType = (typeof CONNCONF_SCOPE_TYPES)[number];

/**
 * Typed `params` tree for a record. The generated schema types `params` as
 * `unknown` (free-form `serde_json::Value`); this is the closed UI shape we
 * author against. Every credential leaf is a bare string: a literal value, or a
 * canonical secret reference (the bare secret UUID, resolved at connect time via
 * its `connection_config:record` secret_usage_binding keyed by field path).
 */
export interface ConnConfCredentials {
  username?: string;
  domain?: string;
  password?: string;
  private_key?: string;
}

export interface ConnConfRdpOptions {
  width?: number;
  height?: number;
  color_depth?: number;
}

export interface ConnConfUi {
  label?: string;
  color?: string;
}

export interface ConnConfJumpHop {
  connconf_record_id: string;
}

export interface ConnConfParams {
  host?: string;
  port?: number;
  timeout_secs?: number;
  /** Video device-management port (protocol `video` only). Family-specific default. */
  mgmt_port?: number;
  /** Video channel index (protocol `video` only). */
  channel?: number;
  /** Video device family (protocol `video` only). Canonical DeviceFamily wire value. */
  family?: string;
  credentials?: ConnConfCredentials;
  options?: ConnConfRdpOptions;
  jump?: ConnConfJumpHop[];
  ui?: ConnConfUi;
  /**
   * App launch target (protocol `app` only). It may be an absolute URI/custom
   * scheme or a trusted local command for Piping Desktop.
   * Stored as `params.url` (matches backend APP_ALLOWED_FIELDS canonical key).
   */
  url?: string;
}

export function asParams(value: unknown): ConnConfParams {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value;
  }
  return {};
}

// ── Records CRUD ─────────────────────────────────────────

export async function listRecords(query?: ConnConfRecordListQuery): Promise<ConnConfRecord[]> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/connection-config/records", {
      params: { query: query ?? {} },
    }),
  );
  return raw.items ?? [];
}

/**
 * Reveal a record's secret-reference credential fields. Only secret-ref fields
 * are echoed back, and `value` is populated only when the caller may view
 * secrets (`can_view_secrets`). Literal credentials are not returned — the form
 * already holds them locally.
 */
export async function revealRecordCredentials(
  id: string,
): Promise<ConnConfRecordCredentials> {
  return unwrap(
    await apiClient.GET("/api/v1/connection-config/records/{id}/credentials", {
      params: { path: { id } },
    }),
  );
}

export async function createRecord(body: CreateRecordBody): Promise<ConnConfRecord> {
  return unwrap(
    await apiClient.POST("/api/v1/connection-config/records", { body }),
  );
}

export async function updateRecord(
  id: string,
  body: UpdateRecordBody,
): Promise<ConnConfRecord> {
  return unwrap(
    await apiClient.PUT("/api/v1/connection-config/records/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteRecord(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/connection-config/records/{id}", {
    params: { path: { id } },
  });
}

// ── Assignments CRUD ─────────────────────────────────────

export async function listAssignments(
  query?: ConnConfAssignmentListQuery,
): Promise<ConnConfAssignment[]> {
  const raw = unwrap(
    await apiClient.GET("/api/v1/connection-config/assignments", {
      params: { query: query ?? {} },
    }),
  );
  return raw.items ?? [];
}

export async function createAssignment(
  body: CreateAssignmentBody,
): Promise<ConnConfAssignment> {
  return unwrap(
    await apiClient.POST("/api/v1/connection-config/assignments", { body }),
  );
}

export async function updateAssignment(
  id: string,
  body: UpdateAssignmentBody,
): Promise<ConnConfAssignment> {
  return unwrap(
    await apiClient.PUT("/api/v1/connection-config/assignments/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteAssignment(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/connection-config/assignments/{id}", {
    params: { path: { id } },
  });
}

export async function bulkAssignRecord(
  recordId: string,
  targetIds: string[],
  weight = 0,
): Promise<BulkAssignRecordResponse> {
  const body: BulkAssignRecordBody = {
    targets: targetIds.map((targetId) => ({ target_id: targetId })),
    weight,
  };
  return unwrap(
    await apiClient.POST(
      "/api/v1/connection-config/records/{id}/assignments/bulk",
      {
        params: { path: { id: recordId } },
        body,
      },
    ),
  );
}

// ── Effective config preview ─────────────────────────────

export async function getEffectiveConfig(
  deviceId: string,
  protocol: string,
): Promise<EffectiveConnConfig> {
  return unwrap(
    await apiClient.GET("/api/v1/connection-config/effective/{device_id}", {
      params: { path: { device_id: deviceId }, query: { protocol } },
    }),
  );
}
