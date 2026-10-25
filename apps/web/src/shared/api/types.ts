/**
 * DTO contract between UI and backend.
 *
 * Every type here mirrors a payload the UI either sends or receives.
 * If the backend uses different field names, prefer to map them inside the
 * `src/api/<feature>.ts` function bodies rather than renaming types here —
 * the UI components import these types directly.
 *
 * Conventions:
 *   - All IDs are strings (UUIDs are fine; numeric IDs should be stringified server-side).
 *   - Timestamps are ISO 8601 strings (`new Date().toISOString()`); UI formats them.
 *   - Secrets (`password`, `privateKey`, `clientSecret`, `token`, …) are write-only:
 *     omit them in GET responses, accept them in POST/PUT/PATCH bodies.
 *   - Paginated list responses follow `Paginated<T>`.
 *
 * Note: types that are field-for-field mirrors of the wire shape are aliased to
 * the generated `schema.gen.ts` (see the `components["schemas"][...]` aliases
 * below). Hand-written declarations are reserved for UI projections that
 * intentionally differ from the wire shape (camelCase view models, flattened
 * resources, decoded blobs, etc.).
 */

import type { components } from "./schema.gen";
import type { Uuid } from "@/shared/lib/ids";

// ── Paginated wrapper ────────────────────────────────────

/**
 * Canonical list envelope. Wire shape is snake_case `{items, total, limit, offset}`
 * on every list endpoint; the TS field names mirror the wire exactly.
 */
export interface Paginated<T> {
  items: T[];
  total: number;
  limit: number;
  offset: number;
}

export interface ListQuery {
  search?: string;
  limit?: number;
  offset?: number;
  /** Comma-separated, e.g. "name:asc,createdAt:desc". */
  sort?: string;
  /** Free-form filters; backend documents which keys are accepted per resource. */
  filter?: Record<string, string>;
}

// Auth types moved to @/features/auth/api.

// ── Inventory: shared resource shape ─────────────────────
//
// The UI uses one polymorphic `Resource` for Sites / Devices / VMs / Tenants /
// Regions / Device Roles. API adapters map each typed backend row into
// this UI projection; page components render only fields in this projection.

export type ResourceStatus = "active" | "planned" | "staged" | "decommissioning" | "offline";

/**
 * UI projection of a materialized polymorphic generic-FK target (the backend
 * `ObjectRef`). `objectType` is the canonical `app_label.model` token,
 * `objectId` the target row UUID, and `label` the human display string — `null`
 * marks a dangling reference (target row missing; the UI renders "Deleted
 * object"). A `null`/absent ref on the owning resource means no assignment.
 */
export interface ObjectRefProjection {
  objectType: string;
  objectId: string;
  label: string | null;
}

export interface Resource {
  users?: string[];
  userGroups?: string[];
  id: string;
  name: string;
  status?: ResourceStatus;
  /** "site" | "device" | "vm" | "tenant" | "region" | "role" */
  type: string;
  customFields?: Record<string, unknown>;
  /** Device/VM local config context override (arbitrary JSON). Merged as the
   *  highest-priority layer over inherited config contexts (NetBox semantics). */
  localContextData?: unknown;
  iconName?: string;
  iconDataUrl?: string;
  site?: string;
  tenant?: string;
  region?: string;
  role?: string;
  platform?: string;
  virtualMachineType?: string;
  defaultPlatform?: string;
  deviceType?: string;
  primaryIp?: string;
  cluster?: string;
  description?: string;
  createdAt?: string;
  updatedAt?: string;
  // Contact-specific scalar fields (surfaced for the Contacts tab).
  title?: string;
  phone?: string;
  email?: string;
  address?: string;
  link?: string;
  // Contact-assignment polymorphic target (`object` ObjectRef) + priority.
  object?: ObjectRefProjection | null;
  /** Inventory-item polymorphic component link (`component` ObjectRef). */
  component?: ObjectRefProjection | null;
  priority?: string;
  // DCIM scalar fields (Device Types / Platforms / Racks / Interfaces /
  // Cables). All stored as strings in the form; api/inventory.ts coerces to the
  // numeric/boolean wire types in toBackendPayload.
  model?: string;
  slug?: string;
  napalmDriver?: string;
  facilityId?: string;
  serial?: string;
  assetTag?: string;
  // Inventory-item scalar fields (PIP-86). `discovered` is stringified for the
  // form, like the other boolean columns.
  partId?: string;
  discovered?: string;
  position?: string;
  face?: string;
  airflow?: string;
  width?: string;
  uHeight?: string;
  rackType?: string;
  enabled?: string;
  interfaceType?: string;
  color?: string;
  length?: string;
  // IPAM + Virtualization scalar fields (Cluster Types / Cluster Groups /
  // Clusters / VLANs / Prefixes / IP Addresses / VM Interfaces). All stored as
  // strings in the form; api/inventory.ts coerces to the numeric/boolean wire
  // types in toBackendPayload.
  vid?: string;
  prefix?: string;
  dnsName?: string;
  isPool?: string;
  prefixUtilizedAddresses?: number;
  prefixUsableAddresses?: number;
  prefixUtilizationPercent?: number;
  vrf?: string;
  ipamRole?: string;
  macAddress?: string;
  mtu?: string;
  mode?: string;
  vcpus?: string;
  memory?: string;
  disk?: string;
  defaultVcpus?: string;
  defaultMemory?: string;
  virtualMachineCount?: string;
  // DCIM scalar fields (Rack Roles / Rack Types / Locations).
  formFactor?: string;
  weight?: string;
  facility?: string;
  // Scalar fields for RIRs / VRFs / Route Targets / Aggregates / ASNs /
  // IP Ranges / Services / Modules / Module Types / Virtual Chassis /
  // Virtual Disks. All stored as strings in the form; api.ts coerces to the
  // numeric/boolean wire types in toBackendPayload.
  isPrivate?: string;
  rd?: string;
  enforceUnique?: string;
  asn?: string;
  prefixCidr?: string;
  startAddress?: string;
  endAddress?: string;
  protocol?: string;
  ports?: string;
  portMappings?: string;
  serviceIpAddressIds?: string[];
  /** Service polymorphic parent (`parent_object` ObjectRef). Required on write. */
  parentObject?: ObjectRefProjection | null;
  size?: string;
  serialNumber?: string;
  assetTagModule?: string;
  domain?: string;
  partNumber?: string;
  comments?: string;
  /** Read-only module-type-profile attribute schema, rendered as a compact JSON cell. */
  schemaSummary?: string;
  // FK UUIDs preserved alongside the resolved *_name fields for edit flows.
  siteId?: string;
  tenantId?: string;
  regionId?: string;
  roleId?: string;
  platformId?: string;
  virtualMachineTypeId?: string;
  defaultPlatformId?: string;
  deviceTypeId?: string;
  clusterId?: string;
  siteGroupId?: string;
  deviceId?: string;
  vmId?: string;
  parentId?: string;
  groupId?: string | null;
  contactId?: string;
  manufacturerId?: string;
  clusterTypeId?: string;
  clusterGroupId?: string;
  vlanId?: string;
  virtualMachineId?: string;
  interfaceId?: string;
  // FK UUIDs.
  rirId?: string;
  vrfId?: string;
  moduleTypeId?: string;
  moduleBayId?: string;
  moduleBayName?: string;
  masterId?: string;
  ipamRoleId?: string;
  // VPN scalar fields.
  encapsulation?: string;
  presharedKey?: string;
  pfsGroup?: string;
  saLifetime?: string;
  saLifetimeSeconds?: string;
  saLifetimeData?: string;
  authenticationMethod?: string;
  encryptionAlgorithm?: string;
  authenticationAlgorithm?: string;
  vpnMode?: string;
  vpnVersion?: string;
  tunnelId?: string;
  vpnRole?: string;
  // VPN FK UUIDs.
  tunnelGroupId?: string;
  ipsecProfileId?: string;
  ikePolicyId?: string;
  ipsecPolicyId?: string;
  tunnelFkId?: string;
  /** Tunnel-termination polymorphic target (`termination` ObjectRef). */
  termination?: ObjectRefProjection | null;
  outsideIpId?: string;
  outsideIpAddress?: string;
  // VPN resolved names.
  tunnelGroupName?: string;
  ipsecProfileName?: string;
  ikePolicyName?: string;
  ipsecPolicyName?: string;
  tunnelFkName?: string;
  // FK UUIDs + scalar fields.
  providerId?: string;
  circuitTypeId?: string;
  cid?: string;
  serviceId?: string;
  providerAccountId?: string;
  terminationAId?: string;
  terminationZId?: string;
  installDate?: string;
  terminationDate?: string;
  commitRate?: string;
  distance?: string;
  distanceUnit?: string;
  asnStart?: string;
  asnEnd?: string;
  // Resolved names returned by the hydrated list endpoint (optional, for
  // FK-carrying entities such as devices/sites/vms/tenants/regions).
  siteGroupName?: string;
  parentName?: string;
  groupName?: string;
  deviceName?: string;
  vmName?: string;
  contactName?: string;
  manufacturerName?: string;
  clusterTypeName?: string;
  clusterGroupName?: string;
  vlanName?: string;
  virtualMachineName?: string;
  interfaceName?: string;
  // Resolved names.
  rirName?: string;
  vrfName?: string;
  moduleTypeName?: string;
  masterName?: string;
  // Resolved names.
  providerName?: string;
  circuitTypeName?: string;
  // Circuits remainder scalar fields.
  termSide?: string;
  xconnectId?: string;
  ppInfo?: string;
  portSpeed?: string;
  upstreamSpeed?: string;
  markConnected?: string;
  account?: string;
  // DCIM gaps scalar fields.
  supply?: string;
  phase?: string;
  voltage?: string;
  amperage?: string;
  maxUtilization?: string;
  availablePower?: string;
  units?: string;
  userId?: string;
  userName?: string;
  ownerId?: string;
  ownerName?: string;
  /** MAC-address polymorphic target (`assigned_object` ObjectRef). */
  assignedObject?: ObjectRefProjection | null;
  identifier?: string;
  primaryIp4Id?: string;
  primaryIp6Id?: string;
  primaryIp4?: string;
  primaryIp6?: string;
  // IPAM gaps scalar fields (ports already exists above).
  // FK UUIDs.
  circuitId?: string;
  providerNetworkId?: string;
  virtualCircuitTypeId?: string;
  powerPanelId?: string;
  rackId?: string;
  // Resolved names.
  circuitName?: string;
  providerNetworkName?: string;
  virtualCircuitTypeName?: string;
  powerPanelName?: string;
  rackName?: string;
  // Form field expansion: additional writable scalar/FK fields per backend payload.
  // All stored as strings in the form; api.ts coerces to numeric/boolean/date wire types.
  timeZone?: string;
  physicalAddress?: string;
  shippingAddress?: string;
  latitude?: string;
  longitude?: string;
  vmRole?: string;
  descUnits?: string;
  outerWidth?: string;
  outerDepth?: string;
  outerHeight?: string;
  outerUnit?: string;
  startingUnit?: string;
  mountingDepth?: string;
  maxWeight?: string;
  weightUnit?: string;
  label?: string;
  mgmtOnly?: string;
  lengthUnit?: string;
  markUtilized?: string;
  markPopulated?: string;
  dateAdded?: string;
  rackRoleId?: string;
  rackRoleName?: string;
}

/** The inventory tabs the UI exposes. */
export type InventoryEntityKey =
  | "Owners" | "Owner Groups"
  | "Sites" | "Devices" | "Virtual Machines" | "Tenants"
  | "Regions" | "Device Roles"
  | "Tenant Groups" | "Contacts" | "Contact Roles"
  | "Contact Groups" | "Contact Assignments"
  | "Manufacturers" | "Site Groups" | "Platforms"
  | "Device Types" | "Racks" | "Interfaces" | "Cables"
  | "Cluster Types" | "Cluster Groups" | "Clusters" | "Virtual Machine Types"
  | "VLANs" | "Prefixes" | "IP Addresses" | "VM Interfaces"
  | "Rack Roles" | "Rack Types" | "Locations"
  | "RIRs" | "VRFs" | "Route Targets" | "Aggregates" | "ASNs"
  | "IP Ranges" | "Services" | "Modules" | "Module Types" | "Module Type Profiles" | "Tags"
  | "Virtual Chassis" | "Virtual Disks"
  // Circuits + IPAM/DCIM gaps.
  | "Providers" | "Circuit Types" | "Provider Networks" | "Circuits"
  | "ASN Ranges" | "Inventory Item Roles" | "Inventory Items"
  // VPN.
  | "Tunnel Groups" | "Tunnels" | "Tunnel Terminations"
  | "IKE Policies" | "IKE Proposals"
  | "IPsec Policies" | "IPsec Proposals" | "IPsec Profiles"
  // Circuits remainder + DCIM gaps + IPAM gaps.
  | "Circuit Groups" | "Provider Accounts" | "Circuit Terminations"
  | "Virtual Circuit Types" | "Virtual Circuits"
  | "Power Panels" | "Power Feeds" | "Rack Reservations"
  | "MAC Addresses" | "Virtual Device Contexts"
  | "Service Templates";

export const INVENTORY_ENTITIES_WITH_STATUS: readonly InventoryEntityKey[] = [
  "Sites",
  "Devices",
  "Virtual Machines",
  "Racks",
  "Cables",
  "Clusters",
  "VLANs",
  "Prefixes",
  "IP Addresses",
  "Locations",
  "IP Ranges",
  "Modules",
  // Entities that carry a status field.
  "Circuits",
  "Tunnels",
  "Virtual Circuits",
  "Power Feeds",
  "Rack Reservations",
  "Virtual Device Contexts",
  "Inventory Items",
];

export function inventoryEntityHasStatus(entity: InventoryEntityKey): boolean {
  return INVENTORY_ENTITIES_WITH_STATUS.includes(entity);
}

// ── Connections: targets (workspace) ─────────────────────

export interface ConnectionTarget {
  id: Uuid;
  name: string;
  targetKind: "device" | "virtual_machine";
  /**
   * True when the target is a managed CCTV device (recorder/camera): its
   * device-management console (opened as a `cctv` workspace tab) and snapshot
   * are available. Optional (like `addressBound`) so existing target literals
   * stay valid; treated as false when absent. `api.ts` always maps a concrete
   * boolean from the hydrated list row.
   */
  isCctv?: boolean;
  /**
   * CCTV device family (stable wire slug, e.g. `dvrip_sofia`) derived from the
   * platform driver. Absent/null for non-CCTV targets.
   */
  family?: string | null;
  status?: ResourceStatus;
  site?: string;
  tenant?: string;
  role?: string;
  roleIconName?: string;
  roleIconDataUrl?: string;
  platform?: string;
  deviceType?: string;
  primaryIp?: string;
  /**
   * True when the target has a primary IP bound to an inventory interface. When
   * false the anti-SSRF guard rejects connect (reason_code
   * `endpoint_not_bound_to_target`); defaults to true if the backend omits it.
   */
  addressBound?: boolean;
  region?: string;
  cluster?: string;
  // FK UUIDs preserved alongside the resolved name fields (`site`, `tenant`, …)
  // for future edit flows; the hydrated list endpoint supplies both.
  siteId?: Uuid;
  tenantId?: Uuid;
  regionId?: Uuid;
  roleId?: Uuid;
  platformId?: Uuid;
  deviceTypeId?: Uuid;
  clusterId?: Uuid;
  deviceId?: Uuid;
  vmId?: Uuid;
}

// ── Connections: live sessions ───────────────────────────

// `cctv` is a client-only console session (no proxied websocket) that opens the
// CCTV device-management mini-admin in the workspace tab pane.
export type SessionProtocol = "ssh" | "rdp" | "vnc" | "cctv";
export type ConnectionSessionStatus =
  | "pending" | "active" | "disconnected" | "terminated" | "failed";

export type ViewerSessionStatus =
  | "connecting" | "connected" | "reconnecting" | "disconnected" | "failed" | "idle";

export interface ActiveSession {
  id: string;
  userId: string;
  hostId: string;
  hostName?: string;
  userDisplayName?: string;
  userEmail?: string;
  clientIp?: string;
  proxyNodeName?: string;
  protocol: SessionProtocol;
  status: ConnectionSessionStatus;
  /** ISO timestamp. */
  connectedAt: string;
  /** ISO timestamp. */
  lastActivity: string;
}

/**
 * Ad-hoc credentials entered at connect time, sent inline on the session-open
 * request as `params.credentials`. The backend accepts a per-protocol subset
 * (SSH: username/password/private_key; RDP: username/domain/password; VNC:
 * password) and either uses them directly (no `entryId`) or overrides the
 * selected account's missing fields (`entryId` present). Only the keys the
 * protocol allows are sent.
 */
export interface AdHocCredentials {
  username?: string;
  domain?: string;
  password?: string;
  private_key?: string;
}

export interface OpenSessionRequest {
  hostId: string;
  protocol: SessionProtocol;
  /** Optional launch-account selector (entry_id from connection-launch-options). */
  entryId?: string;
  /**
   * Optional ad-hoc credentials entered at connect time. Threaded into the
   * request body as `params.credentials`; never persisted by the session-open
   * call (saving a default is a separate user-settings mutation).
   */
  credentials?: AdHocCredentials;
}

// ── Connection launch options ─────────────────────────────

export interface ConnectionLaunchAccountSource {
  /** Personal credential set name (personal only). */
  credentialSetName?: string | null;
  /** Connection-config record name (shared only). */
  recordName?: string | null;
  /** Scope layer (shared only). */
  scopeType?: string | null;
  scopeId?: string | null;
}

export interface ConnectionLaunchAccount {
  entryId: string;
  kind: "shared" | "personal";
  label: string;
  color?: string | null;
  icon?: string | null;
  username?: string | null;
  /**
   * App launch target (protocol `app` only). Absolute URI/custom scheme for a
   * browser, or a trusted local command for Piping Desktop.
   */
  uri?: string | null;
  source: ConnectionLaunchAccountSource;
  isDefault: boolean;
  requiresInput: boolean;
}

export interface ConnectionLaunchProtocol {
  // `app` is a launcher target (no proxied session); `cctv` opens the CCTV
  // device-management console in a workspace tab (no proxied session either).
  protocol: "ssh" | "rdp" | "vnc" | "app" | "cctv";
  render: "auto" | "buttons" | "split";
  accounts: ConnectionLaunchAccount[];
}

export interface ConnectionLaunchOptions {
  hostId: string;
  protocols: ConnectionLaunchProtocol[];
}

/**
 * Proxy WebSocket bootstrap data returned together with the session.
 * UI opens `wss://{host}{websocketUrl}?token=…` to attach a viewer.
 */
export interface ProxyConnectionDetails {
  connectionId: string;
  websocketUrl: string;
  token: string;
  tokenTtlSeconds: number;
}

export interface OpenSessionResponse {
  session: ActiveSession;
  proxy: ProxyConnectionDetails;
}

// ── Connection credential protocols ──────────────────────

export type CredentialProtocol = "SSH" | "RDP" | "VNC";

// ── Config contexts (NetBox-style arbitrary JSON) ────────

/**
 * UI projection of the config-context GET response. Kept hand-written because the
 * view-model is camelCase (`active`, `createdAt`, `updatedAt`) and decodes the
 * wire `data: unknown` into an editable JSON string — it is intentionally not the
 * raw snake_case contract type. The mapping from the generated `ConfigContext`
 * component lives in `features/inventory/config-contexts-api.ts`.
 */
export interface ConfigContext {
  id: string;
  name: string;
  description: string;
  weight: number;
  active: boolean;
  /** Arbitrary JSON payload, stored as a string in the form. */
  data: string;
  createdAt?: string;
  updatedAt?: string;
}

// ── Access control ───────────────────────────────────────

export interface UserRow {
  id: Uuid;
  username: string;
  email: string;
  status: "active" | "inactive" | "suspended";
  authSource: "local" | "oidc";
  // (removed `| string` widening — callers should narrow before assignment)
  externalId: string;
}

export interface CreateUserRequest extends Omit<UserRow, "id"> {
  /** Required when authSource === "local". */
  password?: string;
}

export interface Group {
  id: Uuid;
  name: string;
  externalId: string;
}

export interface Role {
  id: Uuid;
  name: string;
  description: string;
  /** Computed server-side: number of policies that reference this role. */
  policyCount: number;
}

export interface ActionRule { id: string; action: string; }
/**
 * A policy subject in the canonical `kind:uuid` grammar the backend stores and
 * matches on (`user:{uuid}`, `group:{uuid}`, `role:{uuid}`,
 * `service_principal:{uuid}`, plus the read-only `oidc_group:{external_name}`).
 * `id` is the bare entity id (uuid, or the external name for `oidc_group`);
 * `value` is the full wire string sent to / parsed from the backend; `label` is
 * the optional resolved display name (falls back to `id` when unresolved).
 */
export interface SubjectRule {
  id: string;
  kind: "user" | "group" | "role" | "service_principal" | "oidc_group";
  value: string;
  label?: string;
}
export interface ScopeRule {
  id: string;
  /**
   * `resource_type` scopes a policy to a whole class (e.g. `secret_variable`);
   * `resource` scopes it to a specific canonical resource id (e.g.
   * `secret_variable:{uuid}`), serialized into the policy's `resources` list
   * instead of `resource_types`. Inventory-hierarchy scoping (region, site,
   * tenant, cluster, role, platform) is expressed as attribute constraints,
   * not scope rules — the old name-based `site`/`tenant` scope kinds were
   * never serialized into the payload and were removed.
   */
  type: "resource_type" | "resource";
  value: string;
}

export interface Policy {
  id: Uuid;
  name: string;
  description: string;
  effect: "allow" | "deny";
  enabled: boolean;
  actions: ActionRule[];
  subjects: SubjectRule[];
  scopes: ScopeRule[];
  /**
   * Free-form constraint expression (attribute equality / `and` / `or` /
   * `ip_range` / `time_window`) evaluated by the backend. The policy editor
   * authors the attribute-equality subset via a key/value builder and preserves
   * any other node verbatim. Absent when the policy has no constraints.
   */
  constraints?: unknown;
}

export interface Grant {
  id: Uuid;
  /** Role entity id (FK). */
  role: Uuid;
  subjectType: "user" | "group";
  /** Subject entity id (FK): a user id or group id per `subjectType`. */
  subject: Uuid;
}

export interface AccessCheckRequest {
  user: string;
  action: string;
  resourceType?: string;
}

export interface AccessCheckResponse {
  decision: "allow" | "deny";
  matchedPolicies: string[];
  viaRoles: string[];
  viaGroups: string[];
}

// ── SSO config ───────────────────────────────────────────

export interface SsoConfig {
  enabled: boolean;
  issuerUrl: string;
  clientId: string;
  /** Write-only; never returned. */
  clientSecret?: string;
  redirectUrl: string; // typically read-only, computed by backend
  scopes: string;
  groupClaim: string;
  /** "manual" | "jit"; round-tripped so a UI save never resets it. */
  provisioningMode?: string;
  /** Whether client-secret auth is enabled; round-tripped from the read state. */
  clientSecretEnabled?: boolean;
}

// ── Audit log ────────────────────────────────────────────

export interface AuditObjectRef {
  kind: string;
  id?: string;
  label: string;
  secondary?: string;
}

export type AuditLookupOption = components["schemas"]["AuditTargetOption"];

export interface AuditEntry {
  id: string;
  /** ISO timestamp. */
  timestamp: string;
  actor: string;
  /** Actor channel/kind (`user`, `mcp_agent`, `api_token`, …) for the channel badge. */
  actorKind: string;
  action: string;
  resourceType: string;
  resourceId: string;
  summary: string;
  actorRef?: AuditObjectRef;
  actorVia?: AuditObjectRef;
  targetRef?: AuditObjectRef;
  resourceRef?: AuditObjectRef;
  /** Optional structured diff for detail view. */
  details?: Record<string, unknown>;
}

export interface AuditQuery extends ListQuery {
  query?: string;
  actor?: string;
  action?: string;
  resourceType?: string;
  /** UUID of a specific target resource (device, host, template, …). */
  targetId?: string;
  result?: string;
  mcpTool?: string;
  mcpToolGroup?: string;
  mcpProtocol?: string;
  mcpRisk?: string;
  /** ISO timestamp range. */
  from?: string;
  to?: string;
}

// ── Proxy nodes ──────────────────────────────────────────

export type ProxyNode = components["schemas"]["ProxyNodeResponse"];
export type ProxyNodeAssignment = components["schemas"]["ProxyNodeAssignmentResponse"];

// ── Inventory templates ──────────────────────────────────

export type TemplateVariableValueType =
  | "string"
  | "integer"
  | "number"
  | "boolean"
  | "json"
  | "resource_ref";

export interface TemplateVariable {
  key: string;
  label: string;
  description?: string;
  value_type: TemplateVariableValueType;
  required?: boolean;
  default_value?: unknown;
  validation_rules?: Record<string, unknown>;
  sort_order?: number;
}

// keep in sync with backend SUPPORTED_RESOURCE_TYPES in template_step_resource_repository.rs
export type TemplateStepResourceType =
  | "tenant"
  | "site"
  | "device"
  | "virtual_machine"
  | "device_interface"
  | "vm_interface"
  | "ip_address"
  | "tenant_group"
  | "region"
  | "site_group"
  | "location"
  | "prefix"
  | "rack"
  | "rack_role"
  | "rack_type"
  | "manufacturer"
  | "device_role"
  | "device_type"
  | "platform"
  | "contact"
  | "contact_assignment"
  | "cluster"
  | "cluster_type"
  | "cluster_group"
  | "tunnel_group"
  | "tunnel"
  | "tunnel_termination";

export type TemplateStepConflictPolicy = "fail" | "skip" | "update_selected_fields";

export interface TemplateStep {
  step_key: string;
  title: string;
  description?: string;
  resource_type: TemplateStepResourceType;
  operation: "create";
  depends_on_step_keys: string[];
  payload_template: Record<string, unknown>;
  repeat?: Record<string, unknown>;
  on_conflict_policy: TemplateStepConflictPolicy;
  sort_order?: number;
}

export interface InventoryTemplate {
  id: string;
  is_system?: boolean;
  name: string;
  slug: string;
  status: "active" | "draft" | "archived";
  description: string;
  variables: TemplateVariable[];
  steps: TemplateStep[];
  /** ISO timestamp. */
  updatedAt: string;
}

export interface TemplateRunResponse {
  /** Run record id (the backend exposes a full run object; UI surfaces this as the job id). */
  id: string;
  templateId: string;
  status: string;
  startedAt?: string;
  finishedAt?: string;
  summary?: {
    total_steps: number;
    succeeded_steps: number;
    failed_steps: number;
    skipped_steps: number;
  };
  templateName?: string;
}

export interface TemplateRunStep {
  id: string;
  runId: string;
  stepKey: string;
  status: string;
  createdResourceIds: string[];
  resolvedPayload: Record<string, unknown>;
  errorCode?: string;
  errorMessage?: string;
  durationMs?: number;
  createdAt: string;
}

// ── Sync providers ───────────────────────────────────────

export type SyncDirection = "pull" | "push" | "bidirectional";
export type SyncConflictResolution = "netbox_wins" | "piping_wins" | "last_write_wins" | "field_level";

// ── Sync mapping (canonical types — forward/reverse two-slot model) ───────

/**
 * Operator string union matching the backend operator registry.
 * Source of truth: GET /api/v1/sync/operators
 */
export type OperatorName =
  | "identity"
  | "literal"
  | "concat"
  | "split_index"
  | "trim"
  | "lower"
  | "upper"
  | "coerce"
  | "lookup"
  | "template"
  | "dereference"
  | "date_format"
  | "tags_merge"
  | "mac_normalize"
  | "ipv4_prefix_canonical";

/** Invertibility classification returned by the operator registry. */
export type InvertibilityHint = "invertible" | "forward_only" | "normalization";

/** A single transform expression: operator + variadic args. */
export interface Transform {
  operator: OperatorName;
  /** Operator-specific arguments — shape varies per operator. */
  args: Record<string, unknown>;
}

/** Canonical field mapping with forward + reverse two-slot model. */
export interface FieldMapping {
  id: string;
  /** Path within the source entity schema (e.g. "name", "site.slug"). */
  source_field: string;
  /** Path within the target entity schema. */
  target_field: string;
  enabled: boolean;
  /** Forward transform: source → target. */
  forward_transform: Transform;
  /**
   * Reverse transform: target → source.
   * null means forward-only; field cannot be written back from target.
   */
  reverse_transform: Transform | null;
}

/** Canonical entity mapping. */
export interface EntityMapping {
  id: string;
  /** Canonical entity name, e.g. "dcim.device". */
  source_entity: string;
  /** Piping inventory entity type. */
  target_entity: string;
  enabled: boolean;
  field_mappings: FieldMapping[];
}

/** Lifecycle status of one immutable mapping configuration version. */
export type MappingConfigurationStatus = "draft" | "active" | "retired";

/**
 * Where a mapping writes, mirroring the backend `MappingTarget` internally
 * tagged by `kind`. The variants are mutually exclusive: `provider` is a remote
 * provider addressed by id, `local_inventory` is the typed local inventory.
 */
export type MappingTarget =
  | { kind: "local_inventory" }
  | { kind: "provider"; provider_id: string };

/** A versioned mapping configuration. */
export interface MappingConfiguration {
  id: string;
  /** Stable identifier across versions (slug). */
  logical_id: string;
  /** Human-readable name. */
  name: string;
  version: number;
  is_latest: boolean;
  provider_id: string;
  /** Explicit mapping target; a configuration without one is rejected. */
  target: MappingTarget;
  /** Lifecycle status of this version. */
  status: MappingConfigurationStatus;
  direction: SyncDirection;
  conflict_mode: SyncConflictResolution;
  entity_mappings: EntityMapping[];
  created_at: string;
  created_by?: string;
  /** Number of in-flight sync jobs pinned to this version. */
  in_flight_job_count: number;
}

/**
 * Presence of one write-only secret on a sync provider. The wire returns only
 * `has_value`; the secret itself is never sent back, so the UI must never claim
 * a stored value is empty/missing.
 */
export interface SyncProviderSecretField {
  /** Config key the secret is stored under, e.g. "api_token". */
  fieldKey: string;
  /** Whether a value is stored for this provider (the value is not returned). */
  hasValue: boolean;
  /** Whether the field is enabled for this provider. */
  enabled: boolean;
}

/**
 * UI projection of `SourceResponse`: the configured provider identity and its
 * connection settings. There is no per-provider transport state — the API does
 * not expose one — so callers read `enabled` (the configured state) instead of
 * a connected/disconnected status.
 */
export interface SyncProvider {
  id: string;
  /** Provider implementation, e.g. "netbox" | "custom". */
  providerId: string;
  /** User-defined display name. */
  name: string;
  /** Provider-specific connection settings (`url`, …). Secrets are write-only and omitted from the wire. */
  config: Record<string, string>;
  enabled: boolean;
  /** Write-only secret fields (`api_token`, …); presence only, never the value. */
  secretFields: SyncProviderSecretField[];
}

export interface TestConnectionResult {
  ok: boolean;
  message?: string;
  /** Echoed metadata if connection succeeded (version, capabilities, …). */
  meta?: Record<string, unknown>;
}

export type SyncJobStatus = "queued" | "running" | "completed" | "failed";

/**
 * UI projection of `SyncJobResponse`. Kept hand-written: the api mapper narrows
 * the wire `status: string` to the `SyncJobStatus` union (throwing on unknown)
 * and decodes the wire `stats: unknown` into a structured record — neither of
 * which the contract component expresses.
 */
export interface SyncJob {
  id: string;
  providerId: string;
  mappingVersion: string;
  status: SyncJobStatus;
  startedAt?: string;
  finishedAt?: string;
  stats: Record<string, unknown>;
}

// ── User settings ────────────────────────────────────────

export interface PersonalCredential {
  id: string;
  userId: string;
  protocol: CredentialProtocol;
  name: string;
  isDefault: boolean;
  /** Write-only. */
  username?: string;
  domain?: string;
  /** Write-only. */
  password?: string;
  /** Write-only. */
  privateKey?: string;
}

export interface ApiToken {
  id: string;
  name: string;
  status: "active" | "revoked";
  /** Comma-separated scope list, e.g. "read:inventory, write:connections". */
  scopes: string;
  /** ISO timestamp. */
  createdAt: string;
  /** ISO timestamp. */
  expiresAt: string;
  /** Returned ONLY by POST /tokens — the plaintext token to copy. Never on GET. */
  plaintext?: string;
}

// ── License ──────────────────────────────────────────────

export interface LicenseInfo {
  status: "active" | "expired" | "trial" | "invalid" | "missing" | "inactive" | "grace" | "suspended";
  version: string;
  seats: { used: number; total: number };
  /** ISO timestamp. */
  validFrom: string;
  /** ISO timestamp. */
  validUntil: string;
  lastSyncError?: string;
}

export type LicenseActivateRequest = components["schemas"]["LicenseActivateRequest"];
