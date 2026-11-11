/**
 * MSW request handlers for demo mode.
 *
 * These reproduce the CURRENT backend response envelopes consumed by the
 * connections workspace + inventory tree + launch-options, captured
 * from the live seeded backend (http://127.0.0.1:9080) so the mocked shapes are
 * guaranteed correct against current `main`. Data is intentionally plausible
 * (Amsterdam Edge / Frankfurt DC sites, Linux servers, a Windows RDP jump host,
 * a VNC lab box, a router) so the embedded product demo looks like a working
 * install.
 *
 * Scope: every endpoint the demoed screens read. Mutations return a benign
 * echo/no-op so the UI never errors, but demo data is not persisted.
 */
import { http, HttpResponse } from "msw";
import { z } from "zod";
import { effectiveParameters, jobVarsSchema, parameterErrors, parseParameterValues, projectFileVarsSchema } from "@/features/playbooks/varsSchema";
import type { components } from "@/shared/api/schema.gen";
// Bundle the REAL OpenAPI contract into the demo build so the API reference
// (Swagger UI) renders the actual product surface. `?raw` ships the file verbatim
// as a string; Swagger UI parses the YAML client-side once it has a valid
// `openapi: 3.x` document with the right content-type. The path resolves up out
// of apps/web into the sibling apps/api contracts dir (inside the pnpm workspace
// root, so Vite's fs.allow covers it).
import openapiYaml from "../../../api/contracts/piping.openapi.yaml?raw";
import { cctvClipBytes, cctvFrameBytes } from "./cctv-media";
import { demoLanguageFromLocation } from "./language";
import { DEMO_APPLICATION_CONTENT_TYPE } from "./dataflows";
import {
  findById,
  insert,
  isCollection,
  listEnvelope,
  remove,
  rows,
  update,
  type DemoRecord,
} from "./store";

const NOW = Date.parse("2026-06-02T11:20:00Z");
const iso = (minutesAgo: number) => new Date(NOW - minutesAgo * 60_000).toISOString();
const demoRevisions = new Map<string, components["schemas"]["ProjectRevision"]>();
const seedRevision: components["schemas"]["ProjectRevision"] = {
  project_id: "00000000-0000-4000-8000-000000000196",
  revision_id: "00000000-0000-4000-8000-000000000199", created_at: iso(60),
  tree_sha256: "0".repeat(64), entries: [{ path: "site.yml", content: { kind: "file", executable: false,
    bytes: Array.from(new TextEncoder().encode("---\n- name: Synthetic demo\n  hosts: all\n  tasks: []\n")) } }],
};
demoRevisions.set(seedRevision.revision_id, seedRevision);

function validEntry(value: unknown): value is components["schemas"]["ProjectEntry"] {
  if (!value || typeof value !== "object" || !("path" in value) || typeof value.path !== "string" ||
      !value.path || value.path.startsWith("/") || value.path.split("/").some((part) => !part || part === "." || part === "..") ||
      !("content" in value) || !value.content || typeof value.content !== "object" || !("kind" in value.content)) return false;
  const content = value.content;
  if (content.kind === "symlink") return "target" in content && typeof content.target === "string";
  return content.kind === "file" && "executable" in content && typeof content.executable === "boolean" &&
    "bytes" in content && Array.isArray(content.bytes) && content.bytes.every((byte: unknown) => Number.isInteger(byte) && Number(byte) >= 0 && Number(byte) <= 255);
}

const uuid = z.uuid();
const sourceSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("editable") }),
  z.object({ kind: z.literal("git"), url: z.string().min(1), reference: z.string().min(1), auth: z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("anonymous") }), z.object({ kind: z.literal("https_basic"), username: z.string(), secret_id: uuid }),
    z.object({ kind: z.literal("https_bearer"), secret_id: uuid }), z.object({ kind: z.literal("ssh_record"), ssh_record_id: uuid }),
  ]) }),
]);
const inventorySchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("managed"), target_set_id: uuid }),
  z.object({ kind: z.literal("native"), sources: z.array(z.string()).min(1), ssh_record_id: uuid.nullish() }),
]);
const jobSchema = z.object({ name: z.string().min(1), project_id: uuid, execution_environment_id: uuid, workdir: z.string(), playbook_paths: z.array(z.string().min(1)).min(1), inventory: inventorySchema,
  vars_schema: z.discriminatedUnion("kind", [z.object({ kind: z.literal("none") }), z.object({ kind: z.literal("inline"), schema: z.unknown() }), z.object({ kind: z.literal("project_file"), path: z.string() })]).default({ kind: "none" }),
  revision: z.discriminatedUnion("kind", [z.object({ kind: z.literal("current") }), z.object({ kind: z.literal("fixed"), revision_id: uuid })]),
  options: z.object({ argv: z.array(z.string()), env: z.record(z.string(), z.string()), extra_vars: z.unknown(), secret_refs: z.record(z.string(), z.string()), timeout_seconds: z.number().int().positive(), limit: z.string().nullish() }),
});
const targetSchema = z.object({ hostname: z.string().min(1), source_id: uuid.nullish(), target_id: uuid.nullish() });
const overridesSchema = z.object({ extra_vars: z.record(z.string(), z.unknown()).optional() });
const scheduleSchema = z.object({ name: z.string().min(1), template_id: uuid, cron_expression: z.string().min(1), timezone: z.string().nullish(), enabled: z.boolean().nullish(), next_run_at: z.string().nullish(), metadata: z.unknown().optional(), concurrency_policy: z.enum(["skip_if_running", "queue_pending", "replace_pending"]).nullish(), misfire_policy: z.literal("fire_once").nullish(), targets: z.discriminatedUnion("kind", [z.object({ kind: z.literal("fixed"), targets: z.array(targetSchema) }), z.object({ kind: z.literal("dynamic"), selector: z.unknown() })]), overrides: overridesSchema.optional() });
const assignmentSchema = z.object({ job_template_id: uuid, target_set_id: uuid, scope_kind: z.enum(["global", "connection_target", "role", "site", "region", "tenant", "cluster", "platform"]), scope_id: uuid.nullish(), name: z.string().nullish(), priority: z.number().int().nullish(), enabled: z.boolean().nullish(), visible: z.boolean().nullish(), executable: z.boolean().nullish(), quick_action_enabled: z.boolean().nullish(), default_input: z.unknown().optional(), default_metadata: z.unknown().optional(), metadata: z.unknown().optional(), default_target_limit: z.number().int().nullish(), default_timeout_seconds: z.number().int().nullish() });
const projectSchema = z.object({ name: z.string().min(1), description: z.string().nullish(), source: sourceSchema });
const memberSchema = z.object({ source_id: uuid.nullish(), source_kind: z.string(), target_id: uuid, target_kind: z.string(), sort_order: z.number().int().nullish() });
const targetSetSchema = z.object({ name: z.string().min(1), mode: z.enum(["static", "dynamic"]), selector: z.unknown().optional(), members: z.array(memberSchema).nullish() });
const syntheticTargetId = "00000000-0000-4000-8001-000000000001";
const syntheticTarget = { target_id: syntheticTargetId, source_id: syntheticTargetId, source_kind: "device", target_kind: "device", name: "web-prod-01", address_bound: false } satisfies components["schemas"]["PlaybookTargetPreviewItem"];
const targetMembers = new Map<string, components["schemas"]["PlaybookTargetPreviewItem"][]>([["00000000-0000-4000-8000-000000000201", [syntheticTarget]]]);
const demoError = (status: number, message: string) => HttpResponse.json({ code: status === 404 ? "NOT_FOUND" : "DEMO_ERROR", message }, { status });
const now = () => new Date().toISOString();

function newRevision(projectId: string, entries: components["schemas"]["ProjectEntry"][]): components["schemas"]["ProjectRevision"] {
  // The all-zero digest is a synthetic placeholder, not a backend digest claim.
  const revision = { project_id: projectId, revision_id: crypto.randomUUID(), created_at: now(), tree_sha256: "0".repeat(64), entries: structuredClone(entries) };
  demoRevisions.set(revision.revision_id, revision);
  update("playbooks/projects", projectId, { current_revision_id: revision.revision_id });
  return revision;
}

function demoList(key: string, request: Request): Response {
  const query = new URL(request.url).searchParams;
  const filtered = rows(key).filter((row) => ["template_id", "job_template_id", "target_set_id", "scope_kind", "enabled", "quick_action_enabled"].every((field) => !query.has(field) || String(row[field]) === query.get(field)));
  const number = (field: string, fallback: number) => { const value = Number(query.get(field)); return query.has(field) && Number.isSafeInteger(value) && value >= 0 ? value : fallback; };
  const limit = number("limit", 100), offset = number("offset", 0);
  return HttpResponse.json({ items: filtered.slice(offset, offset + limit), total: filtered.length, limit, offset });
}

function launchDemo(id: string, body: Record<string, unknown>): Response {
  const job = findById("playbooks/job-templates", id);
  if (!job) return demoError(404, "Job template not found");
  const parsedJob = jobSchema.safeParse(job);
  const parsed = z.object({ target_ids: z.array(uuid).nullish(), metadata: z.unknown().optional(), overrides: overridesSchema.optional() }).safeParse(body);
  if (!parsed.success || !parsedJob.success) return demoError(400, "Invalid synthetic launch request");
  const template = parsedJob.data;
  const requested = parsed.data.target_ids;
  if (requested && (!requested.length || new Set(requested).size !== requested.length || template.inventory.kind !== "managed")) return demoError(400, "Invalid target selection");
  const members = template.inventory.kind === "managed" ? targetMembers.get(template.inventory.target_set_id) : [];
  if (!members || (requested && requested.some((target) => !members.some((member) => member.target_id === target)))) return demoError(400, "Target is outside the synthetic target set");
  const selected = requested ? members.filter((member) => requested.includes(member.target_id)) : members;
  const project = findById("playbooks/projects", template.project_id);
  const revisionId = template.revision.kind === "fixed" ? template.revision.revision_id : project?.current_revision_id;
  const revision = typeof revisionId === "string" ? demoRevisions.get(revisionId) : undefined;
  if (!revision || revision.project_id !== template.project_id) return demoError(409, "Project revision not found");
  let schema: unknown;
  let extraVars;
  try {
    schema = template.vars_schema.kind === "project_file" ? projectFileVarsSchema(revision.entries, template.vars_schema.path) : jobVarsSchema(template.vars_schema);
    extraVars = effectiveParameters(schema, parseParameterValues(JSON.stringify(template.options.extra_vars)), parseParameterValues(JSON.stringify(parsed.data.overrides?.extra_vars ?? {})));
  } catch { return demoError(400, "Invalid synthetic parameters"); }
  if (Object.keys(parameterErrors(schema, extraVars)).length) return demoError(400, "Synthetic parameters do not match the schema");
  template.options.extra_vars = extraVars;
  const environment = findById("playbooks/execution-environments", template.execution_environment_id);
  if (!environment || typeof environment.image !== "string") return demoError(400, "Execution environment not found");
  const groupId = crypto.randomUUID();
  const timestamp = now();
  const targets = selected.map((member) => ({ hostname: member.name, target_id: member.target_id, source_id: member.source_id }));
  const spec: components["schemas"]["LaunchSpec"] = { template_id: id, project_id: template.project_id, revision_id: revision.revision_id, tree_sha256: revision.tree_sha256, execution_environment_id: template.execution_environment_id, image: environment.image, image_digest: "synthetic-demo", inventory: template.inventory, options: template.options, playbook_paths: template.playbook_paths, workdir: template.workdir, targets };
  const group: components["schemas"]["PlaybookRunGroup"] = { id: groupId, template_id: id, launch_spec: spec, status: "succeeded", trigger_kind: "manual", total_runs: targets.length, succeeded_runs: targets.length, failed_runs: 0, canceled_runs: 0, timeout_runs: 0, unreported_runs: 0, dispatch_attempt: 1, metadata: { synthetic: true, notice: "Simulation only; no Ansible or SSH execution occurred.", input: parsed.data.metadata ?? null }, created_at: timestamp, updated_at: timestamp, started_at: timestamp, finished_at: timestamp };
  const runs: components["schemas"]["PlaybookRun"][] = targets.map((target) => ({ id: crypto.randomUUID(), run_group_id: groupId, hostname: target.hostname, target_id: target.target_id, target_name: target.hostname, source_id: target.source_id, target_address_bound: false, status: "succeeded", attempt: 1, timeout_seconds: template.options.timeout_seconds, credential_account_kind: "none", credential_merge_policy: "none", credential_resolution: {}, metadata: { synthetic: true, notice: "No execution occurred." }, queued_at: timestamp, started_at: timestamp, finished_at: timestamp }));
  rows("playbooks/run-groups").unshift(group);
  rows("playbooks/runs").unshift(...runs);
  return HttpResponse.json({ run_group: group, runs }, { status: 201 });
}

async function automationRequest({ request }: { request: Request }): Promise<Response> {
  const path = new URL(request.url).pathname.replace(/^.*\/api\/v1\/playbooks\/?/, "");
  const [collection = "", id, action, ...extra] = path.split("/").filter(Boolean);
  const key = `playbooks/${collection}`;
  const method = request.method;
  const body = method === "POST" || method === "PATCH" || method === "PUT" ? await readBody(request) : {};
  const existing = id ? findById(key, id) : undefined;
  if (extra.length) return demoError(501, "Unsupported synthetic demo automation operation");

  if (collection === "projects" && id && action === "fork" && method === "POST") {
    if (!existing) return demoError(404, "Project not found");
    const parsed = z.object({ name: z.string().min(1), revision_id: uuid }).safeParse(body);
    if (!parsed.success) return demoError(400, "Invalid fork request");
    const revision = demoRevisions.get(parsed.data.revision_id);
    if (!revision || revision.project_id !== id) return demoError(404, "Revision not found");
    const project = insert(key, { name: parsed.data.name, description: "Synthetic demo fork; no Git operation was performed.", source: { kind: "editable" }, current_revision_id: null });
    newRevision(project.id, revision.entries);
    return HttpResponse.json(findById(key, project.id), { status: 201 });
  }
  if (collection === "projects" && id && action === "sync" && method === "POST") {
    if (!existing) return demoError(404, "Project not found");
    const parsed = z.object({ execution_environment_id: uuid, base_revision_id: uuid.nullish() }).safeParse(body);
    if (!parsed.success || !findById("playbooks/execution-environments", parsed.data.execution_environment_id)) return demoError(400, "Invalid synthetic sync request");
    if (!existing.source || typeof existing.source !== "object" || !("kind" in existing.source) || existing.source.kind !== "git") return demoError(400, "Only Git projects can be synced");
    if ((parsed.data.base_revision_id ?? null) !== (existing.current_revision_id ?? null)) return demoError(409, "Base revision changed");
    const entries: components["schemas"]["ProjectEntry"][] = [{ path: "SYNTHETIC-DEMO.txt", content: { kind: "file", executable: false, bytes: Array.from(new TextEncoder().encode("Synthetic demo only: no Git repository was contacted.\n")) } }];
    const revision = newRevision(id, entries);
    // Like the demo tree digest, this OID is a placeholder, not a real Git identity.
    revision.source_commit_oid = "0".repeat(40);
    return HttpResponse.json(revision, { status: 201 });
  }
  if (collection === "target-sets" && id && action === "preview" && method === "GET") {
    if (!existing) return demoError(404, "Target set not found");
    if (existing.mode === "dynamic") return demoError(501, "Dynamic target resolution is not simulated in the demo");
    const items = targetMembers.get(id) ?? [];
    const requested = Number(new URL(request.url).searchParams.get("limit") ?? 50);
    const limit = Number.isSafeInteger(requested) && requested >= 0 ? requested : 50;
    return HttpResponse.json({ target_set: existing, items: items.slice(0, limit), total: items.length, limit, truncated: items.length > limit });
  }
  if (collection === "job-templates" && id && action === "launch" && method === "POST") return launchDemo(id, body);
  if (collection === "job-templates" && id && action === "vars-schema" && method === "GET") {
    const job = jobSchema.safeParse(findById("playbooks/job-templates", id));
    if (!job.success) return demoError(404, "Job not found");
    const source = job.data.vars_schema;
    if (source.kind !== "project_file") return HttpResponse.json({ source, schema: source.kind === "none" ? null : source.schema, revision: source.kind === "none" ? null : "synthetic-demo" });
    const project = findById("playbooks/projects", job.data.project_id);
    const revisionId = job.data.revision.kind === "fixed" ? job.data.revision.revision_id : project?.current_revision_id;
    const revision = typeof revisionId === "string" ? demoRevisions.get(revisionId) : undefined;
    if (!revision) return demoError(409, "Project revision not found");
    try { return HttpResponse.json({ source, schema: projectFileVarsSchema(revision.entries, source.path), revision: "synthetic-demo" }); } catch { return demoError(400, "Schema file could not be read"); }
  }
  if (collection === "targets" && id && action === "assignments" && method === "GET") {
    const host = HOSTS.find((item) => item.id === id);
    if (id !== syntheticTargetId && !host) return demoError(404, "Synthetic target not found");
    const target = host ? { target_id: host.id, source_id: host.id, source_kind: host.target_kind, target_kind: host.target_kind, name: host.name, address: host.address, address_bound: true, role_id: host.role_id, site_id: host.site_id, tenant_id: host.tenant_id, region_id: host.region_id, cluster_id: host.cluster_id, platform_id: host.platform_id } : syntheticTarget;
    const scopes: Record<string, string | null | undefined> = { connection_target: id, role: host?.role_id, site: host?.site_id, region: host?.region_id, tenant: host?.tenant_id, cluster: host?.cluster_id, platform: host?.platform_id };
    const items = rows("playbooks/assignments").filter((a) => a.enabled && a.visible && (a.scope_kind === "global" || (typeof a.scope_kind === "string" && a.scope_id != null && scopes[a.scope_kind] === a.scope_id)) && typeof a.target_set_id === "string" && targetMembers.get(a.target_set_id)?.some((member) => member.target_id === id)).flatMap((assignment) => { const job_template = typeof assignment.job_template_id === "string" ? findById("playbooks/job-templates", assignment.job_template_id) : undefined; return job_template ? [{ assignment, job_template }] : []; });
    return HttpResponse.json({ target, items });
  }
  if (action) return demoError(501, "Unsupported synthetic demo automation operation");
  const readable = ["projects", "job-templates", "schedules", "assignments", "target-sets", "execution-environments", "run-groups", "runs"];
  if (!readable.includes(collection)) return demoError(501, "Unsupported synthetic demo automation operation");
  if (method === "GET") return id ? existing ? HttpResponse.json(existing) : demoError(404, "Resource not found") : demoList(key, request);
  if (id && !existing) return demoError(404, "Resource not found");
  const writable = ["projects", "job-templates", "schedules", "assignments", "target-sets"];
  if (!writable.includes(collection)) return demoError(501, "Unsupported synthetic demo automation operation");
  if (method === "DELETE" && id) {
    if (collection === "projects" && rows("playbooks/job-templates").some((job) => job.project_id === id)) return demoError(409, "Project has job templates");
    remove(key, id); if (collection === "target-sets") targetMembers.delete(id); return new HttpResponse(null, { status: 204 });
  }
  if ((!id && method !== "POST") || (id && method !== "PATCH" && !(collection === "target-sets" && method === "PUT"))) return demoError(501, "Unsupported synthetic demo automation operation");
  const schema = collection === "projects" ? projectSchema : collection === "job-templates" ? jobSchema : collection === "schedules" ? scheduleSchema : collection === "assignments" ? assignmentSchema : targetSetSchema;
  const nullable = new Set(collection === "projects" ? ["description"] : collection === "assignments" ? ["name", "scope_id", "default_target_limit", "default_timeout_seconds"] : collection === "schedules" ? ["next_run_at"] : []);
  const merged = id ? { ...existing, ...Object.fromEntries(Object.entries(body).filter(([field, value]) => value !== null || nullable.has(field))) } : body;
  const parsed = schema.safeParse(merged);
  if (!parsed.success) return demoError(400, "Invalid synthetic demo request");
  const data = parsed.data;
  if (collection === "target-sets" && "mode" in data && data.mode === "dynamic") return demoError(501, "Dynamic target resolution is not simulated in the demo");
  if (collection === "job-templates" && "project_id" in data && !findById("playbooks/projects", data.project_id)) return demoError(400, "Project not found");
  if ((collection === "assignments" && "job_template_id" in data && !findById("playbooks/job-templates", data.job_template_id)) || (collection === "schedules" && "template_id" in data && !findById("playbooks/job-templates", data.template_id))) return demoError(400, "Job template not found");
  const defaults = collection === "projects" ? { current_revision_id: null } : collection === "schedules" ? { enabled: true, timezone: "UTC", concurrency_policy: "skip_if_running", misfire_policy: "fire_once", metadata: {}, owner_user_id: "00000000-0000-4000-8000-000000000202" } : collection === "assignments" ? { enabled: true, visible: true, executable: true, quick_action_enabled: false, priority: 0, default_input: {}, default_metadata: {}, metadata: {} } : collection === "target-sets" ? { entity_kind: "connection_target", selector: {} } : {};
  const payload: Record<string, unknown> = { ...(id ? {} : defaults), ...Object.fromEntries(Object.entries(data).filter(([field, value]) => value != null || nullable.has(field))) };
  if (collection === "projects" && !id && "source" in data && data.source.kind === "git") payload.description = `Synthetic demo: no Git repository is contacted. ${data.description ?? ""}`.trim();
  const record = id ? update(key, id, payload) : insert(key, payload);
  if (collection === "projects" && !id && "source" in data && data.source.kind === "editable") newRevision(record.id, []);
  if (collection === "target-sets" && "mode" in data) {
    if (data.mode === "dynamic") targetMembers.set(record.id, []);
    else targetMembers.set(record.id, (data.members ?? []).map((member) => ({ ...member, source_id: member.source_id ?? member.target_id, name: HOSTS.find((host) => host.id === member.target_id)?.name ?? `Synthetic target ${member.target_id}`, address_bound: false })));
    if (!id) { const items = targetMembers.get(record.id) ?? []; return HttpResponse.json({ target_set: record, items, total: items.length, limit: 50, truncated: false }, { status: 201 }); }
  }
  return HttpResponse.json(findById(key, record.id), { status: id ? 200 : 201 });
}

/** Parse limit/offset/search from a request URL for store list queries. */
function listOpts(url: URL): { limit?: number; offset?: number; search?: string } {
  const num = (k: string): number | undefined => {
    const v = url.searchParams.get(k);
    if (v === null) return undefined;
    const n = Number(v);
    return Number.isFinite(n) ? n : undefined;
  };
  const search = url.searchParams.get("search") ?? undefined;
  return { limit: num("limit"), offset: num("offset"), search: search || undefined };
}

/** Shape a list response for a store key using the canonical inventory envelope. */
function listResponse(key: string, url: URL): Response {
  const contactFilters = key === "inventory/contact-assignments"
    ? ["object_id", "content_type", "contact_id", "role_id", "priority"].flatMap(field => {
      const values = url.searchParams.get(field)?.split(",").filter(Boolean);
      return values?.length ? [{ field, values }] : [];
    }) : [];
  const profileFilter = key === "monitoring/profile-assignments" ? url.searchParams.get("profile_id") : null;
  const envelope = listEnvelope(key, { ...listOpts(url), filter: record => (!profileFilter || record.profile_id === profileFilter) && contactFilters.every(({ field, values }) => typeof record[field] === "string" && values.includes(record[field])) });
  return HttpResponse.json({ ...envelope, items: envelope.items.map(item => shapeRecord(key, item)) });
}

function groupChain(groupId: unknown): Record<string, unknown>[] {
  if (typeof groupId !== "string") return [];
  const groups = rows("inventory/dataflow-groups");
  const chain: Record<string, unknown>[] = [];
  const visited = new Set<string>();
  let current = groups.find(group => group.id === groupId);
  while (current && !visited.has(current.id)) {
    visited.add(current.id);
    chain.unshift(current);
    current = typeof current.parent_id === "string" ? groups.find(group => group.id === current!.parent_id) : undefined;
  }
  return chain;
}

function endpointLabel(endpoint: Record<string, unknown>): string | null {
  const type = endpoint.object_type;
  const id = endpoint.object_id;
  if (type === "all") return "All Piping hosts";
  if (type === "internet") return "Internet / any IP";
  if (type === "extras.tag") return typeof id === "string" ? rows("inventory/tags").find(tag => tag.id === id)?.name as string ?? null : null;
  if (type === "inventory.addressgroup") return typeof id === "string" ? rows("inventory/address-groups").find(group => group.id === id)?.name as string ?? null : null;
  if (type === "ipam.iprange") return typeof id === "string" ? rows("inventory/ip_ranges").find(range => range.id === id)?.name as string ?? null : null;
  const collections: Record<string, string> = {
    "inventory.application": "inventory/applications",
    "virtualization.cluster": "inventory/clusters",
    "vpn.l2vpn": "inventory/l2vpns",
    "dcim.device": "inventory/devices",
    "virtualization.virtualmachine": "inventory/virtual_machines",
    "dcim.site": "inventory/sites",
    "dcim.region": "inventory/regions",
    "dcim.devicerole": "inventory/device_roles",
    "tenancy.tenant": "inventory/tenants",
    "ipam.service": "inventory/services",
    "ipam.prefix": "inventory/prefixes",
  };
  const collection = typeof type === "string" ? collections[type] : undefined;
  return collection && typeof id === "string" ? rows(collection).find(row => row.id === id)?.name as string ?? null : null;
}

function shapeEndpoint(endpoint: unknown): Record<string, unknown> | null {
  if (!endpoint || typeof endpoint !== "object") return null;
  const source = endpoint as Record<string, unknown>;
  return {
    ...source,
    address_mode: source.address_mode ?? "default",
    ipaddresses: Array.isArray(source.ipaddresses) ? source.ipaddresses : [],
    label: source.label ?? endpointLabel(source),
  };
}

function shapeEndpoints(value: unknown): Record<string, unknown>[] {
  const endpoints: unknown[] = Array.isArray(value) ? value : [];
  return endpoints.map(shapeEndpoint).filter((endpoint): endpoint is Record<string, unknown> => endpoint !== null);
}

function shapeDataflow(record: Record<string, unknown>): Record<string, unknown> {
  const chain = groupChain(record.group_id);
  const inheritedTags = [...new Set(chain.flatMap(group => Array.isArray(group.tags) ? group.tags as string[] : []))];
  const directTags = Array.isArray(record.tags) ? record.tags as string[] : [];
  const flowStatus = typeof record.status === "string" ? record.status : "draft";
  const reversedChain = [...chain].reverse();
  const parentDisabled = reversedChain.find(group => group.status === "disabled");
  const parentDraft = reversedChain.find(group => group.status === "draft");
  const effectiveStatus = parentDisabled ? "disabled" : flowStatus === "disabled" ? "disabled" : parentDraft || flowStatus === "draft" ? "draft" : "active";
  const blocker = effectiveStatus === flowStatus ? null : (parentDisabled ?? parentDraft)?.id ?? null;
  return {
    ...record,
    name: typeof record.name === "string" ? record.name : "",
    description: typeof record.description === "string" ? record.description : "",
    comments: typeof record.comments === "string" ? record.comments : "",
    status: flowStatus,
    effective_status: effectiveStatus,
    blocked_by_group_id: blocker,
    sources: shapeEndpoints(record.sources),
    destinations: shapeEndpoints(record.destinations),
    source_ports: Array.isArray(record.source_ports) ? record.source_ports : null,
    destination_ports: Array.isArray(record.destination_ports) ? record.destination_ports : null,
    icmp_types: Array.isArray(record.icmp_types) ? record.icmp_types : null,
    application_id: record.application_id ?? null,
    group_id: record.group_id ?? null,
    owner_id: record.owner_id ?? null,
    tenant_id: record.tenant_id ?? null,
    tags: directTags,
    inherited_tags: inheritedTags,
    effective_tags: [...new Set([...directTags, ...inheritedTags])].sort(),
    custom_field_data: record.custom_field_data && typeof record.custom_field_data === "object" ? record.custom_field_data : {},
    allow_cross_vrf: record.allow_cross_vrf === true,
  };
}

function shapeGroup(record: Record<string, unknown>): Record<string, unknown> {
  const chain = groupChain(record.id);
  const ancestors = chain.slice(0, -1);
  const inheritedTags = [...new Set(ancestors.flatMap(group => Array.isArray(group.tags) ? group.tags as string[] : []))];
  const directTags = Array.isArray(record.tags) ? record.tags as string[] : [];
  const status = typeof record.status === "string" ? record.status : "active";
  const reversedAncestors = [...ancestors].reverse();
  const parentDisabled = reversedAncestors.find(group => group.status === "disabled");
  const parentDraft = reversedAncestors.find(group => group.status === "draft");
  const effectiveStatus = parentDisabled ? "disabled" : status === "disabled" ? "disabled" : parentDraft || status === "draft" ? "draft" : "active";
  return {
    ...record,
    status,
    effective_status: effectiveStatus,
    blocked_by_group_id: effectiveStatus === status ? null : (parentDisabled ?? parentDraft)?.id ?? null,
    parent_id: record.parent_id ?? null,
    application_id: record.application_id ?? null,
    tags: directTags,
    inherited_tags: inheritedTags,
    effective_tags: [...new Set([...directTags, ...inheritedTags])].sort(),
    comments: typeof record.comments === "string" ? record.comments : "",
    owner_id: record.owner_id ?? null,
    tenant_id: record.tenant_id ?? null,
    custom_field_data: record.custom_field_data && typeof record.custom_field_data === "object" ? record.custom_field_data : {},
  };
}

function shapeAddressGroup(record: Record<string, unknown>): Record<string, unknown> {
  return {
    comments: "",
    owner_id: null,
    tenant_id: null,
    tags: [],
    custom_field_data: {},
    ...record,
    members: (Array.isArray(record.members) ? record.members : []).map((member, position) => ({
      ...shapeEndpoint(member), position,
    })),
  };
}

function shapeRecord(key: string, record: Record<string, unknown>): Record<string, unknown> {
  if (key === "monitoring/profiles") {
    const items = profileItems(record);
    return { ...record, items, item_count: items.length, assignment_count: rows("monitoring/profile-assignments").filter(assignment => assignment.profile_id === record.id).length };
  }
  if (key === "monitoring/profile-assignments") {
    const profile = findById("monitoring/profiles", String(record.profile_id));
    return { ...record, profile_name: profile?.name ?? "", profile_slug: profile?.slug ?? "" };
  }
  if (key === "inventory/contact-assignments") return {
    id: record.id,
    object: { object_type: record.object_type, object_id: record.object_id, label: endpointLabel(record) },
    contact_id: record.contact_id, role_id: record.role_id, priority: record.priority ?? null,
  };
  if (key === "inventory/dataflows") return shapeDataflow(record);
  if (key === "inventory/dataflow-groups") return shapeGroup(record);
  if (key === "inventory/applications") return { description: "", comments: "", role_id: null, owner_id: null, tenant_id: null, tags: [], custom_field_data: {}, ...record };
  if (key === "inventory/application-roles") return { description: "", comments: "", owner_id: null, ...record };
  if (key === "inventory/address-groups") return shapeAddressGroup(record);
  return record;
}


function demoDataflowEndpointResolution(body: Record<string, unknown>): Response {
  const inventory = [
    ...rows("inventory/devices").map(host => ({ host, type: "dcim.device" as const })),
    ...rows("inventory/virtual_machines").map(host => ({ host, type: "virtualization.virtualmachine" as const })),
  ];
  const diagnostics: Array<{ code: string; message: string }> = [];
  const tagScope = body.tag_object_type;
  const resolveHost = ({ host, type }: { host: Record<string, unknown>; type: "dcim.device" | "virtualization.virtualmachine" }) => {
    const primary = typeof host.primary_ip === "string" ? host.primary_ip : "";
    const oob = type === "dcim.device" && typeof host.oob_ip === "string" ? host.oob_ip : "";
    const address = body.address_mode === "oob" ? oob : primary;
    if (body.address_mode === "oob" && !oob) diagnostics.push({ code: "oob_unavailable", message: "No out-of-band address on this device." });
    const hostId = typeof host.id === "string" ? host.id : "";
    const id = body.address_mode === "oob" ? `demo-oob-${hostId}` : `demo-ip-${hostId}`;
    const selected = new Set(Array.isArray(body.ipaddresses) ? body.ipaddresses : []);
    const eligible = address.length > 0 && host.status === "active" && (body.address_mode !== "selected" || selected.has(id));
    return {
      host: { object_type: type, object_id: host.id, label: host.name },
      addresses: eligible ? [{ cidr: `${address}/32`, vrf_id: null }] : [],
      diagnostics: [] as Array<{ code: string; message: string }>,
    };
  };
  const resolve = (selector: Record<string, unknown>): { members: ReturnType<typeof resolveHost>[]; addresses: Array<{ cidr: string; vrf_id: null }>; ipaddresses: Array<{ id: string; address: string; vrf_id: null }> } => {
    const type = selector.object_type;
    const id = selector.object_id;
    if (type === "ipam.iprange") {
      const cidr = id === "ip-range-demo-0000-0000-000000000001" ? "198.51.100.0/24" : "";
      return { members: [], addresses: cidr ? [{ cidr, vrf_id: null }] : [], ipaddresses: [] };
    }
    if (type === "inventory.addressgroup") {
      const group = rows("inventory/address-groups").find(item => item.id === id);
      if (!group) {
        diagnostics.push({ code: "address_group_missing", message: "Address group is unavailable." });
        return { members: [], addresses: [], ipaddresses: [] };
      }
      const members = (Array.isArray(group.members) ? group.members : []).map(member => resolve(member as Record<string, unknown>));
      return {
        members: members.flatMap(member => member.members),
        addresses: members.flatMap(member => member.addresses),
        ipaddresses: members.flatMap(member => member.ipaddresses),
      };
    }
    const known = type === "extras.tag"
      ? inventory.filter(({ host, type: hostType }) => hostType === tagScope && Array.isArray(host.tags) && host.tags.includes(id))
      : inventory.filter(({ host, type: hostType }) =>
        type === "all" || type === hostType && id === host.id ||
        type === "dcim.site" && id === host.site_id || type === "dcim.devicerole" && id === host.role_id);
    const members = known.map(resolveHost);
    const ipaddresses = known.flatMap(({ host }) => typeof host.primary_ip === "string" ? [{
      id: `demo-ip-${host.id}`, address: `${host.primary_ip}/32`, vrf_id: null,
    }] : []);
    return { members, addresses: members.flatMap(member => member.addresses), ipaddresses };
  };
  const resolved = resolve(body);
  return HttpResponse.json({ endpoint: body, members: resolved.members, addresses: resolved.addresses,
    ipaddresses: resolved.ipaddresses, default_ipaddresses: resolved.ipaddresses.map(address => address.id), diagnostics });
}

export function matchesAuditSearch(event: Record<string, unknown>, query: string): boolean {
  const context = event.context;
  const display = context && typeof context === "object" && "display" in context ? context.display : undefined;
  const labels = display && typeof display === "object" ? [
    "actor" in display ? display.actor : undefined,
    "target" in display ? display.target : undefined,
  ] : [];
  const search = query.trim().toLowerCase();
  return !search || [event.id, event.event_type, event.target_type, event.actor_id, event.target_id, ...labels]
    .some(value => typeof value === "string" && value.toLowerCase().includes(search));
}

function auditTargetsResponse(request: Request): Response {
  const url = new URL(request.url);
  const targetType = url.searchParams.get("target_type");
  const search = (url.searchParams.get("search") ?? "").trim().toLowerCase();
  const limit = Number(url.searchParams.get("limit") ?? 30);
  const max = Number.isFinite(limit) ? Math.min(Math.max(limit, 1), 50) : 30;
  const unique = new Map<string, { target_type: string; kind: string; id: string; label: string }>();
  for (const event of rows("audit/events")) {
    if (typeof event.target_type !== "string" || typeof event.target_id !== "string") continue;
    if (targetType && event.target_type !== targetType) continue;
    if (search && !event.target_id.toLowerCase().includes(search)) continue;
    unique.set(
      `${event.target_type}:${event.target_id}`,
      { target_type: event.target_type, kind: event.target_type, id: event.target_id, label: event.target_id },
    );
  }
  return HttpResponse.json({ items: [...unique.values()].slice(0, max), limit: max });
}

async function readBody(request: Request): Promise<Record<string, unknown>> {
  try {
    const body = (await request.clone().json()) as unknown;
    if (body && typeof body === "object" && !Array.isArray(body)) {
      return body as Record<string, unknown>;
    }
  } catch {
    // Empty / non-JSON body — treat as no fields.
  }
  return {};
}

function demoPrefixTreeResponse(request: Request): Response {
  const url = new URL(request.url);
  const search = (url.searchParams.get("search") ?? "").trim().toLowerCase();
  const family = url.searchParams.get("family");
  const status = url.searchParams.get("status");
  const rootPrefix = url.searchParams.get("root_prefix");
  const limit = Math.min(Math.max(Number(url.searchParams.get("limit") ?? 300), 1), 500);
  const nodes = [
    {
      id: "px-0000-0000-0000-000000000001",
      node_id: "prefix:px-0000-0000-0000-000000000001",
      prefix: "192.0.2.0/24",
      parent_id: null,
      parent_prefix: null,
      root_id: null,
      root_prefix: rootPrefix,
      depth: 0,
      child_count: 2,
      status: "container",
      family: 4,
      site_id: SITE_AMS.id,
      site_name: SITE_AMS.name,
      vrf_id: null,
      vrf_name: null,
      tenant_id: TENANT_CORE.id,
      tenant_name: TENANT_CORE.name,
      mark_utilized: false,
      utilized_addresses: 5,
      usable_addresses: 254,
      utilization_percent: 2.0,
      breadcrumbs: ["192.0.2.0/24"],
    },
    {
      id: "px-0000-0000-0000-000000000011",
      node_id: "prefix:px-0000-0000-0000-000000000011",
      prefix: "192.0.2.0/25",
      parent_id: "px-0000-0000-0000-000000000001",
      parent_prefix: "192.0.2.0/24",
      root_id: "px-0000-0000-0000-000000000001",
      root_prefix: "192.0.2.0/24",
      depth: 1,
      child_count: 0,
      status: "active",
      family: 4,
      site_id: SITE_AMS.id,
      site_name: SITE_AMS.name,
      vrf_id: null,
      vrf_name: null,
      tenant_id: TENANT_CORE.id,
      tenant_name: TENANT_CORE.name,
      mark_utilized: true,
      utilized_addresses: 120,
      usable_addresses: 126,
      utilization_percent: 100,
      breadcrumbs: ["192.0.2.0/24", "192.0.2.0/25"],
    },
    {
      id: "px-0000-0000-0000-000000000012",
      node_id: "prefix:px-0000-0000-0000-000000000012",
      prefix: "192.0.2.128/25",
      parent_id: "px-0000-0000-0000-000000000001",
      parent_prefix: "192.0.2.0/24",
      root_id: "px-0000-0000-0000-000000000001",
      root_prefix: "192.0.2.0/24",
      depth: 1,
      child_count: 0,
      status: "reserved",
      family: 4,
      site_id: SITE_AMS.id,
      site_name: SITE_AMS.name,
      vrf_id: null,
      vrf_name: null,
      tenant_id: TENANT_CORE.id,
      tenant_name: TENANT_CORE.name,
      mark_utilized: false,
      utilized_addresses: 1,
      usable_addresses: 126,
      utilization_percent: 0.8,
      breadcrumbs: ["192.0.2.0/24", "192.0.2.128/25"],
    },
    {
      id: "px-0000-0000-0000-000000000002",
      node_id: "prefix:px-0000-0000-0000-000000000002",
      prefix: "198.51.100.0/24",
      parent_id: null,
      parent_prefix: null,
      root_id: null,
      root_prefix: rootPrefix,
      depth: 0,
      child_count: 0,
      status: "active",
      family: 4,
      site_id: SITE_FRA.id,
      site_name: SITE_FRA.name,
      vrf_id: null,
      vrf_name: null,
      tenant_id: TENANT_CORE.id,
      tenant_name: TENANT_CORE.name,
      mark_utilized: false,
      utilized_addresses: 4,
      usable_addresses: 254,
      utilization_percent: 1.6,
      breadcrumbs: ["198.51.100.0/24"],
    },
    {
      id: "px-0000-0000-0000-000000000006",
      node_id: "prefix:px-0000-0000-0000-000000000006",
      prefix: "2001:db8:20::/64",
      parent_id: null,
      parent_prefix: null,
      root_id: null,
      root_prefix: rootPrefix,
      depth: 0,
      child_count: 0,
      status: "active",
      family: 6,
      site_id: SITE_AMS.id,
      site_name: SITE_AMS.name,
      vrf_id: null,
      vrf_name: null,
      tenant_id: TENANT_CORE.id,
      tenant_name: TENANT_CORE.name,
      mark_utilized: false,
      utilized_addresses: 0,
      usable_addresses: null,
      utilization_percent: null,
      breadcrumbs: ["2001:db8:20::/64"],
    },
  ];
  const filtered = nodes.filter((node) => {
    if (family && String(node.family) !== family) return false;
    if (status && !status.split(",").includes(node.status)) return false;
    if (rootPrefix && !node.prefix.startsWith(rootPrefix.split("/")[0] ?? rootPrefix)) return false;
    if (!search) return true;
    return [node.prefix, node.status, node.site_name, node.tenant_name]
      .filter(Boolean)
      .some((value) => String(value).toLowerCase().includes(search));
  });
  return HttpResponse.json({
    nodes: filtered.slice(0, limit),
    total: filtered.length,
    limit,
    applied_filters: {
      search: search || null,
      root_id: null,
      root_prefix: rootPrefix,
      family: family ? Number(family) : null,
      site_id: [],
      vrf_id: [],
      tenant_id: [],
      status: status ? status.split(",") : [],
      limit,
    },
  });
}

function demoPrefixPlanningPreview(request: Request): Response {
  const url = new URL(request.url);
  const prefixLength = Number(url.searchParams.get("prefix_length") ?? 26);
  return HttpResponse.json({
    parent_id: "px-0000-0000-0000-000000000001",
    parent_prefix: "192.0.2.0/24",
    requested_prefix_length: prefixLength,
    status: prefixLength > 32 ? "invalid" : "available",
    candidate_prefix: prefixLength > 32 ? null : "192.0.2.64/26",
    reason: prefixLength > 32 ? "invalid_prefix_length" : null,
    conflicts: [],
    searched_candidates: 2,
  });
}

const CABLING_DEMO_ENDPOINTS = [
  {
    endpoint: { kind: "interface", id: "cb-iface-0000-0000-0000-000000000001", label: "edge-router-01 · ge-0/0/1" },
    parent: { entity: "Devices", id: "00000000-0000-4000-8001-000000000005", label: "edge-router-01" },
    location: {
      site: { entity: "Sites", id: "site-ams-0000-0000-0000-000000000001", label: "Amsterdam Edge" },
      rack: { entity: "Racks", id: "rack-ams-0000-0000-0000-000000000001", label: "AMS-R1" },
      location: null,
    },
    link_state: "cabled",
    trace_state: "complete",
    cable: { id: "cable-0000-0000-0000-000000000001", label: "CAB-AMS-001", status: "connected", color: "2f80ed" },
    related_links: [
      { entity: "Devices", id: "00000000-0000-4000-8001-000000000005", label: "edge-router-01" },
      { entity: "Cables", id: "cable-0000-0000-0000-000000000001", label: "CAB-AMS-001" },
    ],
  },
  {
    endpoint: { kind: "interface", id: "cb-iface-0000-0000-0000-000000000002", label: "web-prod-01 · eth0" },
    parent: { entity: "Devices", id: "00000000-0000-4000-8001-000000000001", label: "web-prod-01" },
    location: {
      site: { entity: "Sites", id: "site-ams-0000-0000-0000-000000000001", label: "Amsterdam Edge" },
      rack: { entity: "Racks", id: "rack-ams-0000-0000-0000-000000000001", label: "AMS-R1" },
      location: null,
    },
    link_state: "cabled",
    trace_state: "complete",
    cable: { id: "cable-0000-0000-0000-000000000001", label: "CAB-AMS-001", status: "connected", color: "2f80ed" },
    related_links: [
      { entity: "Devices", id: "00000000-0000-4000-8001-000000000001", label: "web-prod-01" },
      { entity: "Cables", id: "cable-0000-0000-0000-000000000001", label: "CAB-AMS-001" },
    ],
  },
  {
    endpoint: { kind: "front-port", id: "cb-front-0000-0000-0000-000000000001", label: "patch-panel-01 · F01" },
    parent: { entity: "Devices", id: "device-patch-0000-0000-0000-0000000001", label: "patch-panel-01" },
    location: {
      site: { entity: "Sites", id: "site-ams-0000-0000-0000-000000000001", label: "Amsterdam Edge" },
      rack: { entity: "Racks", id: "rack-ams-0000-0000-0000-000000000001", label: "AMS-R1" },
      location: null,
    },
    link_state: "marked-connected",
    trace_state: "incomplete",
    cable: null,
    related_links: [{ entity: "Devices", id: "device-patch-0000-0000-0000-0000000001", label: "patch-panel-01" }],
  },
  {
    endpoint: { kind: "power-outlet", id: "cb-pout-0000-0000-0000-000000000001", label: "pdu-a · outlet 08" },
    parent: { entity: "Devices", id: "device-pdu-0000-0000-0000-00000000001", label: "pdu-a" },
    location: {
      site: { entity: "Sites", id: "site-ams-0000-0000-0000-000000000001", label: "Amsterdam Edge" },
      rack: { entity: "Racks", id: "rack-ams-0000-0000-0000-000000000001", label: "AMS-R1" },
      location: null,
    },
    link_state: "available",
    trace_state: "unsupported",
    cable: null,
    related_links: [{ entity: "Devices", id: "device-pdu-0000-0000-0000-00000000001", label: "pdu-a" }],
  },
] as const;

function demoCablingEndpoints(request: Request): Response {
  const url = new URL(request.url);
  const query = (url.searchParams.get("query") ?? "").trim().toLowerCase();
  const endpointKinds = new Set((url.searchParams.get("endpoint_kind") ?? "").split(",").filter(Boolean));
  const linkStates = new Set((url.searchParams.get("link_state") ?? "").split(",").filter(Boolean));
  const traceStates = new Set((url.searchParams.get("trace_state") ?? "").split(",").filter(Boolean));
  const issuesOnly = url.searchParams.get("issues_only") === "true";
  const limit = Math.min(Math.max(Number(url.searchParams.get("limit") ?? 50), 1), 100);
  const filtered = CABLING_DEMO_ENDPOINTS.filter((item) => {
    if (query && ![item.endpoint.label, item.parent?.label, item.cable?.label].some((value) => value?.toLowerCase().includes(query))) {
      return false;
    }
    if (endpointKinds.size > 0 && !endpointKinds.has(item.endpoint.kind)) return false;
    if (linkStates.size > 0 && !linkStates.has(item.link_state)) return false;
    if (traceStates.size > 0 && !traceStates.has(item.trace_state)) return false;
    if (issuesOnly && item.trace_state === "complete") return false;
    return true;
  });
  return HttpResponse.json({ items: filtered.slice(0, limit), total: filtered.length, limit, offset: 0 });
}

function demoCablingTrace(request: Request): Response {
  const url = new URL(request.url);
  const endpointId = url.searchParams.get("endpoint_id");
  const start = CABLING_DEMO_ENDPOINTS.find((item) => item.endpoint.id === endpointId) ?? CABLING_DEMO_ENDPOINTS[0];
  const peer = start.cable
    ? CABLING_DEMO_ENDPOINTS.find((item) => item.cable?.id === start.cable?.id && item.endpoint.id !== start.endpoint.id)
    : undefined;
  const state = start.trace_state;
  const issues = state === "complete" ? [] : [
    {
      code: state === "unsupported" ? "unsupported-endpoint" : "marked-connected",
      endpoint: start.endpoint,
      message: state === "unsupported"
        ? "Demo endpoint kind is indexed but not traversable."
        : "Endpoint is marked connected without a cable reference.",
    },
  ];
  return HttpResponse.json({
    start: start.endpoint,
    state,
    summary: { hop_count: peer ? 1 : 0, cable_count: start.cable ? 1 : 0, issue_count: issues.length },
    segments: peer
      ? [{ index: 0, traversal: "cable", from: start.endpoint, to: peer.endpoint, cable: start.cable, note: null }]
      : [],
    issues,
    related_links: start.related_links,
    generated_at: iso(0),
  });
}

// ── Sites / tenants / roles ──────────────────────────────────────────────────
const SITE_AMS = {
  id: "site-ams-0000-0000-0000-000000000001",
  name: "Amsterdam Edge",
};
const SITE_FRA = {
  id: "site-fra-0000-0000-0000-000000000002",
  name: "Frankfurt DC",
};
const TENANT_CORE = { id: "ten-core-0000-0000-0000-000000000001", name: "Core Infrastructure" };
const TENANT_CORP = { id: "ten-corp-0000-0000-0000-000000000002", name: "Corporate IT" };

const ROLE_SERVER = {
  id: "role-srv-0000-0000-0000-000000000001",
  name: "Linux Server",
  custom_fields: { piping_icon: { name: "server" } },
};
const ROLE_JUMP = {
  id: "role-jump-0000-0000-0000-00000000002",
  name: "Jump Host",
  custom_fields: { piping_icon: { name: "monitor" } },
};
const ROLE_LAB = {
  id: "role-lab-0000-0000-0000-000000000003",
  name: "Lab Workstation",
  custom_fields: { piping_icon: { name: "laptop" } },
};
const ROLE_ROUTER = {
  id: "role-rtr-0000-0000-0000-000000000004",
  name: "Router",
  custom_fields: { piping_icon: { name: "router" } },
};
const ROLE_CAMERA = {
  id: "role-cam-0000-0000-0000-000000000005",
  name: "CCTV Recorder",
  custom_fields: { piping_icon: { name: "cctv" } },
};

const DEVICE_TYPE = { id: "dt-server-0000-0000-0000-00000000001", name: "Generic Server" };

// ── Connection targets (Connections workspace + target tree) ─────────────────
// target_kind identifies the backing inventory source: device or virtual_machine.
interface DemoHost {
  id: string;
  name: string;
  address: string;
  target_kind: "device" | "virtual_machine";
  status: string;
  device_id: string | null;
  device_name: string | null;
  vm_id: string | null;
  vm_name: string | null;
  tenant_id: string | null;
  tenant_name: string | null;
  site_id: string | null;
  site_name: string | null;
  region_id: string | null;
  region_name: string | null;
  role_id: string;
  role_name: string;
  role_custom_fields: Record<string, unknown> | null;
  platform_id: string | null;
  platform_name: string | null;
  device_type_id: string | null;
  device_type_name: string | null;
  cluster_id: string | null;
  cluster_name: string | null;
  // Drives the CCTV badge in the tree/cards and the CCTV console entry point.
  // The read model sets it from the device family, not from the protocol.
  is_cctv: boolean;
  family: string | null;
}

function device(
  id: string,
  name: string,
  address: string,
  site: { id: string; name: string },
  tenant: { id: string; name: string },
  role: { id: string; name: string; custom_fields: Record<string, unknown> },
): DemoHost {
  return {
    id,
    name,
    address,
    target_kind: "device",
    status: "active",
    device_id: id,
    device_name: name,
    vm_id: null,
    vm_name: null,
    tenant_id: tenant.id,
    tenant_name: tenant.name,
    site_id: site.id,
    site_name: site.name,
    region_id: null,
    region_name: null,
    role_id: role.id,
    role_name: role.name,
    role_custom_fields: role.custom_fields,
    platform_id: null,
    platform_name: null,
    device_type_id: DEVICE_TYPE.id,
    device_type_name: DEVICE_TYPE.name,
    cluster_id: null,
    cluster_name: null,
    is_cctv: false,
    family: null,
  };
}

function vm(
  id: string,
  name: string,
  address: string,
  site: { id: string; name: string },
  tenant: { id: string; name: string },
  role: { id: string; name: string; custom_fields: Record<string, unknown> },
): DemoHost {
  return {
    ...device(id, name, address, site, tenant, role),
    target_kind: "virtual_machine",
    device_id: null,
    device_name: null,
    vm_id: id,
    vm_name: name,
  };
}

// A Linux host carries the non-video `linux` family: it is not cctv, yet the
// neutral device-users plane manages its local accounts, so the workspace shows
// the Accounts entry beside its SSH button.
const H_WEB: DemoHost = {
  ...device("00000000-0000-4000-8001-000000000001", "web-prod-01", "192.0.2.11", SITE_AMS, TENANT_CORE, ROLE_SERVER),
  family: "linux",
};
const H_DB = device("00000000-0000-4000-8001-000000000002", "db-primary-01", "192.0.2.21", SITE_AMS, TENANT_CORE, ROLE_SERVER);
const H_APP = { ...vm("00000000-0000-4000-8001-000000000003", "app-node-02", "192.0.2.32", SITE_AMS, TENANT_CORE, ROLE_SERVER), site_id: null, site_name: null };
const H_JUMP = device("00000000-0000-4000-8001-000000000004", "win-rdp-jump", "192.0.2.41", SITE_AMS, TENANT_CORP, ROLE_JUMP);
const H_ROUTER = device("00000000-0000-4000-8001-000000000005", "edge-router-01", "192.0.2.1", SITE_AMS, TENANT_CORE, ROLE_ROUTER);

const H_FRA_WEB = device("00000000-0000-4000-8001-000000000011", "fra-web-01", "198.51.100.11", SITE_FRA, TENANT_CORE, ROLE_SERVER);
const H_FRA_DB = device("00000000-0000-4000-8001-000000000012", "fra-db-01", "198.51.100.21", SITE_FRA, TENANT_CORE, ROLE_SERVER);
const H_FRA_LAB = vm("00000000-0000-4000-8001-000000000013", "vnc-lab-01", "198.51.100.51", { id: "site-lab-0000-0000-0000-000000000003", name: "Remote Lab" }, TENANT_CORP, ROLE_LAB);
const H_FRA_JUMP = device("00000000-0000-4000-8001-000000000014", "fra-rdp-jump", "198.51.100.41", SITE_FRA, TENANT_CORP, ROLE_JUMP);

// Two CCTV devices from different families, because the capability matrix is
// per-family: the Sofia/XMeye recorder exposes everything (PTZ, archive, export,
// on-device accounts), while a generic ONVIF camera is view-only by construction.
// Demoing both is what makes that distinction visible instead of implied.
const H_NVR: DemoHost = {
  ...device("00000000-0000-4000-8001-000000000021", "nvr-ams-01", "192.0.2.90", SITE_AMS, TENANT_CORP, ROLE_CAMERA),
  is_cctv: true,
  family: "dvrip_sofia",
};
const H_CAM_ONVIF: DemoHost = {
  ...device("00000000-0000-4000-8001-000000000022", "cam-fra-entrance", "198.51.100.90", SITE_FRA, TENANT_CORP, ROLE_CAMERA),
  is_cctv: true,
  family: "onvif",
};

const HOSTS: DemoHost[] = [
  H_WEB, H_DB, H_APP, H_JUMP, H_ROUTER, H_NVR,
  H_FRA_WEB, H_FRA_DB, H_FRA_LAB, H_FRA_JUMP, H_CAM_ONVIF,
];

// The workspace sends the same flat query as the native targets endpoint.
// Keep its demo fixture here, rather than filtering a second time in the UI.
function demoConnectionTargetsResponse(request: Request): Response {
  const params = new URL(request.url).searchParams;
  const filters: Array<[string, keyof DemoHost]> = [
    ["status", "status"], ["target_kind", "target_kind"],
    ["region_id", "region_id"], ["site_id", "site_id"],
    ["tenant_id", "tenant_id"], ["role_id", "role_id"],
    ["platform_id", "platform_id"], ["device_type_id", "device_type_id"],
  ];
  const sortFields: Record<string, "name" | "status" | "target_kind" | "region_name" | "site_name" | "tenant_name" | "role_name" | "platform_name" | "device_type_name" | "address"> = {
    name: "name", status: "status", targetKind: "target_kind", region: "region_name",
    site: "site_name", tenant: "tenant_name", role: "role_name", platform: "platform_name",
    deviceType: "device_type_name", primaryIp: "address",
  };
  const sortKey = params.get("sort") || "name";
  const sortField = Object.hasOwn(sortFields, sortKey) ? sortFields[sortKey] : undefined;
  const order = params.get("order") || "asc";
  if (!sortField || (order !== "asc" && order !== "desc")) {
    return HttpResponse.json({ message: "Invalid sort" }, { status: 400 });
  }
  const numberParam = (key: string, fallback: number, max: number) => {
    const raw = params.get(key);
    if (raw === null) return fallback;
    const value = Number(raw);
    return raw.trim().length > 0 && Number.isSafeInteger(value) && value >= 0 ? Math.min(value, max) : null;
  };
  const requestedLimit = numberParam("limit", 50, 50_000);
  const offset = numberParam("offset", 0, 2_147_483_647);
  if (requestedLimit === null || offset === null) {
    return HttpResponse.json({ message: "Invalid pagination" }, { status: 400 });
  }
  const limit = Math.max(1, requestedLimit);
  const search = (params.get("search") ?? "").trim().toLowerCase();
  const matchesText = (value: string | null, query: string) => (value ?? "").toLowerCase().includes(query);
  const selected = filters.map(([param, field]) => ({
    field,
    values: (params.get(param) ?? "").split(",").map((value) => value.trim().toLowerCase()).filter(Boolean),
  }));
  const name = (params.get("name") ?? "").trim().toLowerCase();
  const address = (params.get("address") ?? "").trim().toLowerCase();
  const filtered = HOSTS.filter((host) =>
    (!search || [host.name, host.address, host.device_name, host.vm_name].some((value) => matchesText(value, search))) &&
    (!name || matchesText(host.name, name)) && (!address || matchesText(host.address, address)) &&
    selected.every(({ field, values }) => {
      const value = host[field];
      return values.length === 0 || typeof value === "string" && values.includes(value.toLowerCase());
    }),
  );
  filtered.sort((left, right) => {
    const a = left[sortField];
    const b = right[sortField];
    const comparison = a == null ? (b == null ? 0 : 1) : b == null ? -1 : a.localeCompare(b);
    return (order === "desc" ? -comparison : comparison) || left.id.localeCompare(right.id);
  });
  return HttpResponse.json({ items: filtered.slice(offset, offset + limit), total: filtered.length, limit, offset });
}

// Opt-in public density fixture; the ordinary walkthrough stays small.
if (new URLSearchParams(globalThis.location?.search).get("dense-map") === "1") {
  HOSTS.push(...Array.from({ length: 495 }, (_, offset) => {
    const index = offset + 5;
    return (index < 250 ? device : vm)(`dense-map-${index}`, `ams-${index < 250 ? "device" : "vm"}-${String(index).padStart(3, "0")}`, `192.0.${Math.floor(index / 250) + 2}.${index % 250 + 1}`, SITE_AMS, TENANT_CORE, ROLE_SERVER);
  }));
  const additionalSites = [
    { id: "dense-site-campus", name: "Amsterdam Campus", latitude: "52.3676", longitude: "4.9041" },
    { id: "dense-site-origin", name: "Equator Station", latitude: "0", longitude: "0" },
  ];
  for (const site of additionalSites) {
    insert("inventory/sites", { ...site, slug: site.id });
    HOSTS.push(device(`target-${site.id}`, `${site.name} gateway`, "203.0.113.10", site, TENANT_CORE, ROLE_SERVER));
  }
}

// ── Connection launch options ────────────────────────────────────────────────
// Shape captured from GET /api/v1/connections/targets/{id}/launch-options.
// Accounts carry clean labels + usernames so ConnectButtons renders e.g.
// "root", "RDP", "VNC", "WEB" — never the bracketed "[...]" auto-label fallback.
interface LaunchAccount {
  entry_id: string;
  kind: "shared" | "personal";
  label: string;
  color?: string;
  username?: string;
  uri?: string;
  source: {
    scope_type?: string;
    scope_id?: string;
    record_name?: string;
    credential_set_name?: string;
  };
  is_default: boolean;
  requires_input: boolean;
}
interface LaunchProtocol {
  protocol: "ssh" | "rdp" | "vnc" | "app" | "cctv";
  render: "auto" | "buttons" | "split";
  accounts: LaunchAccount[];
}

const emptyProto = (protocol: LaunchProtocol["protocol"]): LaunchProtocol => ({
  protocol,
  render: "auto",
  accounts: [],
});

function sshAccount(label: string, username: string, isDefault = false): LaunchProtocol {
  return {
    protocol: "ssh",
    render: "auto",
    accounts: [
      {
        entry_id: `config:connconf:ssh:${username}`,
        kind: "shared",
        label,
        color: "#22c55e",
        username,
        source: { scope_type: "role", record_name: `Linux Server · ${label}` },
        is_default: isDefault,
        requires_input: false,
      },
    ],
  };
}

function rdpAccount(label: string, username?: string): LaunchProtocol {
  return {
    protocol: "rdp",
    render: "auto",
    accounts: [
      {
        entry_id: `config:connconf:rdp:0`,
        kind: "shared",
        label,
        color: "#00BCD4",
        ...(username ? { username } : {}),
        source: { scope_type: "site", record_name: `Jump Host · ${label}` },
        is_default: false,
        requires_input: !username,
      },
    ],
  };
}

function vncAccount(label: string): LaunchProtocol {
  return {
    protocol: "vnc",
    render: "auto",
    accounts: [
      {
        entry_id: `config:connconf:vnc:0`,
        kind: "shared",
        label,
        color: "#f59e0b",
        source: { scope_type: "tenant", record_name: `Lab Workstation · ${label}` },
        is_default: false,
        requires_input: true,
      },
    ],
  };
}

// `requires_input` must stay false: a truthy value opens the ad-hoc credentials
// dialog instead of the console, and CCTV never mints a backend session.
function cctvAccount(label: string, recordName: string): LaunchProtocol {
  return {
    protocol: "cctv",
    render: "auto",
    accounts: [
      {
        entry_id: `config:connconf:cctv:0`,
        kind: "shared",
        label,
        color: "#8b5cf6",
        source: { scope_type: "role", record_name: recordName },
        is_default: false,
        requires_input: false,
      },
    ],
  };
}

function appAccount(label: string, uri: string): LaunchProtocol {
  return {
    protocol: "app",
    render: "auto",
    accounts: [
      {
        entry_id: `config:connconf:app:0`,
        kind: "shared",
        label,
        color: "#4CAF50",
        uri,
        source: { record_name: `Router · ${label}` },
        is_default: false,
        requires_input: false,
      },
    ],
  };
}

// Per-host launch options. Linux servers expose SSH (sometimes root + deploy),
// the jump hosts expose RDP, the lab box VNC, the router SSH + a WEB app link.
const LAUNCH_OPTIONS: Record<string, LaunchProtocol[]> = {
  [H_WEB.id]: [sshAccount("root", "root", true), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app")],
  [H_DB.id]: [
    { protocol: "ssh", render: "auto", accounts: [
      sshAccount("root", "root", true).accounts[0]!,
      { entry_id: "config:connconf:ssh:deploy", kind: "shared", label: "deploy", color: "#3b82f6", username: "deploy", source: { scope_type: "role", record_name: "Linux Server · deploy" }, is_default: false, requires_input: false },
    ] },
    emptyProto("rdp"), emptyProto("vnc"), emptyProto("app"),
  ],
  [H_APP.id]: [sshAccount("deploy", "deploy", true), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app")],
  [H_JUMP.id]: [emptyProto("ssh"), rdpAccount("RDP", "Administrator"), emptyProto("vnc"), emptyProto("app")],
  [H_ROUTER.id]: [
    sshAccount("admin", "admin", true),
    emptyProto("rdp"),
    emptyProto("vnc"),
    appAccount("WEB", "https://192.0.2.1"),
  ],
  [H_FRA_WEB.id]: [sshAccount("root", "root", true), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app")],
  [H_FRA_DB.id]: [sshAccount("root", "root", true), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app")],
  [H_FRA_LAB.id]: [emptyProto("ssh"), emptyProto("rdp"), vncAccount("VNC"), emptyProto("app")],
  [H_FRA_JUMP.id]: [emptyProto("ssh"), rdpAccount("RDP", "Administrator"), emptyProto("vnc"), emptyProto("app")],
  [H_NVR.id]: [
    emptyProto("ssh"), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app"),
    cctvAccount("CCTV", "CCTV Recorder · admin"),
  ],
  [H_CAM_ONVIF.id]: [
    emptyProto("ssh"), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app"),
    cctvAccount("CCTV", "CCTV Camera · viewer"),
  ],
};

const emptyLaunch = [emptyProto("ssh"), emptyProto("rdp"), emptyProto("vnc"), emptyProto("app")];

// ── Synthetic sessions ───────────────────────────────────────────────────────
const ACTIVE_SESSIONS = [
  {
    id: "sess-0000-0000-0000-000000000001",
    target_id: H_WEB.id,
    user_id: "u-admin",
    protocol: "ssh",
    status: "active",
    proxy_id: "px-0000-0000-0000-000000000001",
    proxy_node_id: null,
    proxy_node_name: null,
    proxy_node_slug: null,
    routing_source: "embedded_default",
    client_ip: "203.0.113.10",
    last_activity: iso(1),
    session_metadata: { entryId: "config:connconf:ssh:root" },
    started_at: iso(14),
    ended_at: null as string | null,
  },
] satisfies [unknown, ...unknown[]];

const DEMO_SESSIONS = [
  ...ACTIVE_SESSIONS,
  { ...ACTIVE_SESSIONS[0], id: "sess-0000-0000-0000-000000000002", target_id: H_FRA_LAB.id, protocol: "vnc", started_at: iso(25) },
  { ...ACTIVE_SESSIONS[0], id: "sess-0000-0000-0000-000000000003", target_id: H_JUMP.id, protocol: "rdp", status: "terminated", started_at: iso(120), last_activity: iso(90), ended_at: iso(89) },
  { ...ACTIVE_SESSIONS[0], id: "sess-0000-0000-0000-000000000004", target_id: H_DB.id, status: "disconnected", started_at: iso(180), last_activity: iso(150), ended_at: iso(149) },
];

function syntheticSession(session: typeof DEMO_SESSIONS[number]) {
  return {
    ...session,
    presence: { state: "unknown", generation: null, operator_attached: null, observer_count: null, observed_at: null, last_operator_activity_at: null, reconnect_deadline: null, ended_at: session.ended_at },
    observe: { allowed: false, reason: demoLanguageFromLocation() === "ru" ? "Наблюдение недоступно в демо." : "Observation is unavailable in the demo.", websocket_url: null, transport: null },
  };
}

function demoSessionList(url: URL): Response {
  const query = url.searchParams;
  const history = query.get("view") === "history";
  const search = (query.get("search") ?? "").trim().toLowerCase();
  const selected = (key: string) => (query.get(key) ?? "").split(",").filter(Boolean);
  const filters = ["target_id", "user_id", "protocol", "status"] as const;
  const matching = DEMO_SESSIONS.filter((session) => {
    const terminal = session.ended_at !== null || ["disconnected", "terminated", "failed"].includes(session.status);
    const active = session.ended_at === null && ["pending", "active"].includes(session.status);
    if (history ? !terminal : !active) return false;
    if (filters.some((key) => { const values = selected(key); return values.length > 0 && !values.includes(session[key]); })) return false;
    const host = HOSTS.find((item) => item.id === session.target_id);
    return !search || [session.id, session.target_id, session.user_id, session.protocol, session.status, host?.name, "admin", "admin@piping.demo"].some((value) => value?.toLowerCase().includes(search));
  });
  const sort = query.get("sort");
  const sortKeys = { host: "target_id", user: "user_id", protocol: "protocol", status: "status", started_at: "started_at", last_activity: "last_activity" } as const;
  const validSort = sort && Object.hasOwn(sortKeys, sort);
  const sortKey = validSort ? sortKeys[sort as keyof typeof sortKeys] : "started_at";
  const direction = !validSort || query.get("order") === "desc" ? -1 : 1;
  matching.sort((a, b) => direction * a[sortKey].localeCompare(b[sortKey]) || a.id.localeCompare(b.id));
  const opts = listOpts(url);
  const limit = Math.min(Math.max(Math.trunc(opts.limit ?? 50), 1), 1000);
  const offset = Math.max(Math.trunc(opts.offset ?? 0), 0);
  return HttpResponse.json({ items: matching.slice(offset, offset + limit).map(syntheticSession), total: matching.length, limit, offset });
}

function demoSessionHealth(sessionId: string): Response {
  return HttpResponse.json({
    session_id: sessionId,
    connection_type: "ssh",
    route_mode: "proxied",
    state: "degraded",
    reason_code: "proxy_target_latency",
    worst_leg_id: "proxy_target",
    proxy_node_name: "pxm-proxy01",
    updated_at: iso(0),
    stale_after_ms: 30_000,
    legs: [
      {
        id: "client_backend",
        state: "ok",
        latency_ms: 18,
        rx_bps: 2_048,
        tx_bps: 1_024,
        rx_bytes: 182_272,
        tx_bytes: 91_136,
        quality_label: "good",
        quality_score: 96,
        message: null,
      },
      {
        id: "backend_proxy",
        state: "ok",
        latency_ms: 42,
        rx_bps: 8_192,
        tx_bps: 4_096,
        rx_bytes: 724_992,
        tx_bytes: 348_160,
        quality_label: "good",
        quality_score: 91,
        message: null,
      },
      {
        id: "proxy_target",
        state: "degraded",
        latency_ms: 167,
        rx_bps: 512,
        tx_bps: 320,
        rx_bytes: 44_032,
        tx_bytes: 23_552,
        quality_label: "warning",
        quality_score: 61,
        message: "Target network latency is elevated",
      },
    ],
  });
}

// Inventory generic entities (Resources / Access Control / Settings screens) now
// live in the stateful store (`./store.ts`) and are served by the generic CRUD
// handlers below so create/edit/delete reflect in-session.

// Full capability grid (every Action × ResourceType from the OpenAPI enums) so
// the demo user is a full administrator. Without this the recent RBAC
// capability-gating hides almost every nav section/route/control (the SPA gates
// on `auth/me.capabilities`), leaving the demo looking empty/locked.
const DEMO_ACTIONS = [
  "CREATE", "READ", "UPDATE", "DELETE", "CONNECT", "OBSERVE", "EXECUTE",
  "EXECUTE_NATIVE", "DELEGATE_CREDENTIALS",
  "MANAGE_ROLES", "MANAGE_POLICIES", "VIEW_AUDIT", "SECRET",
  // CCTV carries purpose-built actions instead of the generic CRUD verbs; the
  // access-control editor and the token scope picker render from this list.
  // `DEVICE_USERS` is the neutral on-device account action (video recorders and
  // Linux hosts alike), governed on the connection target.
  "CCTV_VIEW", "CCTV_RECORDINGS", "CCTV_EXPORT", "DEVICE_USERS", "CCTV_SETTINGS",
  "CCTV_PTZ",
] as const satisfies readonly components["schemas"]["Action"][];
const DEMO_RESOURCE_TYPES = [
  "CONNECTION_TARGET", "CONNECTION_SESSION", "CONNECTION_FILTER_PRESET", "USER",
  "GROUP", "ROLE", "POLICY", "INVENTORY", "CONFIG_CONTEXT", "SECRET_VARIABLE",
  "AUDIT_EVENT", "KEYCLOAK", "WEBHOOK", "ASSISTANT",
  "ASSISTANT_CONVERSATION", "PLAYBOOK_PROJECT_REVISION", "PLAYBOOK_ASSIGNMENT",
  "PLAYBOOK_TARGET_SET", "PLAYBOOK_RUN_GROUP", "PLAYBOOK_RUN", "PLAYBOOK_SCHEDULE",
  "PLAYBOOK_ARTIFACT", "PLAYBOOK_SERVICE_PRINCIPAL",
  "PLAYBOOK_EXECUTION_ENVIRONMENT", "PLAYBOOK_PROJECT", "PLAYBOOK_JOB_TEMPLATE",
  "LICENSE", "SETTINGS", "BRANDING", "PROXY_NODE", "MONITORING", "BACKUP", "CCTV",
] as const satisfies readonly components["schemas"]["ResourceType"][];
const DEMO_CAPABILITIES = DEMO_RESOURCE_TYPES.flatMap((resource_type) =>
  DEMO_ACTIONS.map((action) => ({ action, resource_type })),
);

const SESSION_USER = {
  user_id: "u-admin",
  username: "admin",
  email: "admin@piping.demo",
  status: "active",
  auth_source: "local",
  expires_at: iso(-60 * 8),
  roles: ["Platform Admin"],
  capabilities: DEMO_CAPABILITIES,
};

const DEMO_PUBLIC_FILTER_PRESETS: Array<{
  id: string;
  name: string;
  filters: Record<string, string[]>;
  created_by: string;
  created_at: string;
  updated_at: string;
}> = [
  {
    id: "preset-public-active-0000-0000-000000000001",
    name: "Active infrastructure",
    filters: { status: ["active"] },
    created_by: "00000000-0000-0000-0000-000000000001",
    created_at: iso(80),
    updated_at: iso(80),
  },
];

// Instance branding — a single mutable record (not a list, so it lives here
// rather than in the store registry). Edits in the Appearance screen persist
// in-session and reset on reload like everything else.
const DEMO_BRANDING: {
  app_name: string;
  favicon_data_url: string | null;
  logo_data_url: string | null;
  theme: Record<string, unknown>;
  updated_at: string;
} = {
  app_name: "Piping",
  favicon_data_url: null,
  logo_data_url: null,
  theme: { mode: "default" },
  updated_at: iso(60 * 24),
};
const userPreferences = new Map<string, unknown>();
const instanceSettings = new Map<string, unknown>();

function effectiveSetting(key: string) {
  // Demo embed contract: the embedding portal names the demo language in
  // `?lang=`. That query is the single source of truth, so the mocked settings
  // API answers `app.language` with it — the app's own preference load then
  // resolves to the embedded language instead of an unset/stale default and
  // cannot override the explicit demo input.
  if (key === "app.language") {
    const demoLanguage = demoLanguageFromLocation();
    if (demoLanguage) {
      const value = { lng: demoLanguage };
      return {
        key,
        value,
        source: "instance",
        user_value: null,
        instance_value: value,
        updated_at: iso(0),
      };
    }
  }
  const hasUser = userPreferences.has(key);
  const hasInstance = instanceSettings.has(key);
  const userValue = hasUser ? userPreferences.get(key) : undefined;
  const instanceValue = hasInstance ? instanceSettings.get(key) : undefined;
  return {
    key,
    value: hasUser ? userValue : hasInstance ? instanceValue : null,
    source: hasUser ? "user" : hasInstance ? "instance" : "unset",
    user_value: hasUser ? userValue : null,
    instance_value: hasInstance ? instanceValue : null,
    updated_at: hasUser || hasInstance ? iso(0) : null,
  };
}

// Service Discovery preview, derived from the seeded HOSTS. Every host becomes a
// node-exporter scrape target/group so the SD screen renders a populated preview
// and non-zero target/group stat cards.
function buildSdPreview() {
  const hosts = HOSTS.map((h) => ({
    host_address: h.address,
    host_id: h.id,
    host_name: h.name,
    inspector_metrics: [],
    kind: h.target_kind,
    metric_sources: [],
    metrics: [],
    role_name: h.role_name,
    scrapes: [],
    site_name: h.site_name,
    status: h.status,
    tenant_name: h.tenant_name,
  }));
  const groups = hosts.map((h) => ({
    labels: {
      __address__: `${h.host_address}:9100`,
      job: "node",
      instance: h.host_name,
      site: h.site_name ?? "",
      role: h.role_name ?? "",
    },
    targets: [`${h.host_address}:9100`],
  }));
  return { groups, groups_count: groups.length, hosts, targets_count: groups.length };
}

/**
 * Demo mirror of `GET /api/v1/monitoring/coverage`: per-host scrape state with
 * the assignment behind each target. Mirrors the backend vocabulary
 * (`covered` / `no_scrape` / `duplicate` / `filtered` plus a stable reason) for
 * the demoed hosts and the seeded exporter assignments.
 */
function profileItems(profile: Record<string, unknown>): DemoRecord[] {
  const profileId = typeof profile.id === "string" ? profile.id : "";
  return ((profile.items ?? []) as DemoRecord[]).map((item, index) => {
    const exporter = findById("monitoring/exporters", String(item.exporter_id));
    return { ...item, id: item.id ?? `${profileId}-item-${index}`, profile_id: profileId, exporter_name: exporter?.name ?? "", exporter_slug: exporter?.slug ?? "", effective_job: item.job ?? exporter?.default_job ?? "", enabled: item.enabled !== false, sort_order: item.sort_order ?? index, labels: item.labels ?? {}, created_at: item.created_at ?? profile.created_at, updated_at: item.updated_at ?? profile.updated_at };
  });
}

function profileAssignmentItems(assignment: DemoRecord): DemoRecord[] {
  const profile = findById("monitoring/profiles", String(assignment.profile_id));
  if (!profile) return [];
  return profileItems(profile).map(item => ({ ...item, ...assignment, exporter_id: item.exporter_id, exporter_slug: item.exporter_slug, job: item.job, scheme: item.scheme, port: item.port, metrics_path: item.metrics_path, labels: item.labels, profile_slug: profile.slug, name: profile.name, enabled: assignment.enabled === true && profile.enabled === true && item.enabled === true && findById("monitoring/exporters", String(item.exporter_id))?.enabled === true }));
}

function coverageData(params: URLSearchParams, draft?: DemoRecord) {
  const hostId = params.get("host_id");
  const roleId = params.get("role_id");
  const siteId = params.get("site_id");
  const search = params.get("search")?.toLowerCase() ?? "";
  const hosts = HOSTS.filter(
    (host) =>
      (!hostId || host.id === hostId) &&
      (!roleId || host.role_id === roleId) &&
      (!siteId || host.site_id === siteId) &&
      (!search || host.name.toLowerCase().includes(search)),
  );
  const saved = rows("monitoring/profile-assignments");
  // A draft previews an unsaved row by replacing the row it edits, so an edit
  // never counts itself twice.
  const assignmentRows = draft ? [...saved.filter((row) => row.id !== draft.id), draft] : saved;
  const assignments = assignmentRows.flatMap(profileAssignmentItems);
  const exporterOf = (assignment: DemoRecord) =>
    findById("monitoring/exporters", String(assignment.exporter_id));
  const matches = (assignment: DemoRecord, host: DemoHost) => {
    switch (assignment.scope_type) {
      case "global": return true;
      case "role": return assignment.scope_id === host.role_id;
      case "site": return assignment.scope_id === host.site_id;
      case "tenant": return assignment.scope_id === host.tenant_id;
      case "connection_target": return assignment.scope_id === host.id;
      case "device": return host.target_kind === "device" && assignment.scope_id === host.id;
      case "virtual_machine": return host.target_kind === "virtual_machine" && assignment.scope_id === host.id;
      default: return false;
    }
  };
  const text = (value: unknown, fallback: string) =>
    typeof value === "string" && value.trim() !== "" ? value : fallback;
  const resolved = (assignment: DemoRecord, host: DemoHost) => {
    const exporter = exporterOf(assignment);
    const labels = { ...(exporter?.labels as Record<string, string> | undefined), ...(assignment.labels as Record<string, string> | undefined) };
    const job = text(labels.job, text(assignment.job, text(exporter?.default_job, text(assignment.exporter_slug, "piping"))));
    const scheme = text(assignment.scheme, text(exporter?.default_scheme, "http"));
    const metricsPath = text(assignment.metrics_path, text(exporter?.default_metrics_path, "/metrics"));
    const port = Number(assignment.port ?? exporter?.default_port ?? 9100);
    const baseTarget = text(assignment.target, host.address);
    return {
      assignment_id: String(assignment.id),
      assignment_name: (assignment.name as string | null) ?? null,
      exporter_slug: String(assignment.exporter_slug),
      assignment_scope_type: String(assignment.scope_type),
      assignment_scope_id: (assignment.scope_id as string | null) ?? null,
      job,
      target: /:\d+$/.test(baseTarget) ? baseTarget : `${baseTarget}:${port}`,
      labels: {
        __scheme__: scheme,
        __metrics_path__: metricsPath,
        job,
        piping_job: job,
        ...labels,
      },
    };
  };

  type Target = ReturnType<typeof resolved>;
  const producedBy = new Map<string, string[]>();
  const hostEvidence = hosts.map((host) => {
    const matched = assignments.filter((assignment) => matches(assignment, host));
    const enabled = matched.filter((assignment) => assignment.enabled === true);
    const active = host.status === "active";
    const targets: Target[] = active ? enabled.map((assignment) => resolved(assignment, host)) : [];
    for (const target of targets) {
      const key = `${target.target}\u0000${target.job}`;
      producedBy.set(key, [...(producedBy.get(key) ?? []), target.assignment_id]);
    }
    return { host, matched, active, targets };
  });

  const hostsPayload = hostEvidence.map(({ host, matched, active, targets }) => {
    if (!active) return { host_id: host.id, host_name: host.name, kind: host.target_kind, status: host.status, state: "filtered", reason: "status_filtered", scrapes: 0, targets: [] };
    const duplicate = targets.some((target) => (producedBy.get(`${target.target}\u0000${target.job}`)?.length ?? 0) > 1);
    if (duplicate) return { host_id: host.id, host_name: host.name, kind: host.target_kind, status: host.status, state: "duplicate", reason: null, scrapes: targets.length, targets };
    if (targets.length > 0) return { host_id: host.id, host_name: host.name, kind: host.target_kind, status: host.status, state: "covered", reason: null, scrapes: targets.length, targets };
    const reason = matched.length === 0 ? "no_assignment" : matched.every((assignment) => assignment.enabled !== true) ? "disabled_assignment" : "no_target";
    return { host_id: host.id, host_name: host.name, kind: host.target_kind, status: host.status, state: "no_scrape", reason, scrapes: 0, targets: [] };
  });

  const duplicatePairs = [...producedBy.entries()]
    .filter(([, assignmentIds]) => assignmentIds.length > 1)
    .map(([key, assignmentIds]) => {
      const [target, job] = key.split("\u0000");
      return {
        job: job ?? null,
        target: target ?? "",
        assignments: [...new Set(assignmentIds)].map((assignmentId) => ({
          assignment_id: assignmentId,
          exporter_slug: text(assignments.find((assignment) => assignment.id === assignmentId)?.exporter_slug, ""),
        })),
      };
    });

  const assignmentsPayload = assignmentRows
    .filter((assignment) => hostEvidence.some(({ host }) => matches(assignment, host)))
    .map((assignment) => {
      const matchedHosts = hostEvidence.filter(({ active, host }) => active && matches(assignment, host));
      const expanded = profileAssignmentItems(assignment);
      const targets = matchedHosts.flatMap(({ host }) => expanded.filter(item => item.enabled === true).map(item => resolved(item, host)));
      return {
        assignment_id: String(assignment.id),
        name: text(findById("monitoring/profiles", String(assignment.profile_id))?.name, ""),
        profile_slug: text(findById("monitoring/profiles", String(assignment.profile_id))?.slug, ""),
        scope_type: String(assignment.scope_type),
        scope_id: (assignment.scope_id as string | null) ?? null,
        matched_hosts: matchedHosts.length,
        targets: targets.length,
        duplicates: targets.filter((target) => (producedBy.get(`${target.target}\u0000${target.job}`)?.length ?? 0) > 1).length,
      };
    });

  const totals = {
    hosts_total: hostsPayload.length,
    covered: hostsPayload.filter((host) => host.state === "covered").length,
    no_scrape: hostsPayload.filter((host) => host.state === "no_scrape").length,
    duplicate: hostsPayload.filter((host) => host.state === "duplicate").length,
    filtered: hostsPayload.filter((host) => host.state === "filtered").length,
  };
  const draftMatchedHosts = draft
    ? hostEvidence
        .filter(({ host, active }) => active && matches(draft, host))
        .map(({ host }) => ({ host_id: host.id, host_name: host.name, kind: host.target_kind }))
    : [];
  return { hosts: hostsPayload, totals, assignments: assignmentsPayload, duplicates: duplicatePairs, draftMatchedHosts, total: hostsPayload.length, limit: 200, offset: 0 };
}

function buildMonitoringCoverage(request: Request) {
  return coverageData(new URL(request.url).searchParams);
}

function buildProfilePreview(id: string, request: Request): Response {
  const profile = findById("monitoring/profiles", id);
  if (!profile) return HttpResponse.json({ message: "Monitoring profile not found" }, { status: 404 });
  const params = new URL(request.url).searchParams;
  const scopeType = params.get("scope_type") ?? "global";
  const scopeId = params.get("scope_id");
  const saved = rows("monitoring/profile-assignments").find(assignment => assignment.profile_id === id && assignment.scope_type === scopeType && (assignment.scope_id ?? null) === scopeId);
  const draft: DemoRecord = { id: saved?.id ?? "profile-preview", profile_id: id, scope_type: scopeType, scope_id: scopeId, enabled: true, target: params.has("target") ? params.get("target")?.trim() || null : saved?.target ?? null };
  const data = coverageData(new URLSearchParams(), draft);
  const matching = new Set(data.draftMatchedHosts.map(host => host.host_id));
  const hosts = data.hosts.filter(host => matching.has(host.host_id)).map(host => ({ host_id: host.host_id, host_name: host.host_name, host_address: HOSTS.find(entry => entry.id === host.host_id)?.address ?? "", kind: host.kind, targets: host.targets.filter(target => target.assignment_id === draft.id).map(target => target.target) }));
  const targets = data.hosts.flatMap(host => host.targets.filter(target => target.assignment_id === draft.id));
  const jobs = new Set(profileAssignmentItems(draft).filter(item => item.enabled === true).map(item => item.effective_job));
  const conflicts: components["schemas"]["ProfileConflict"][] = [];
  for (const other of rows("monitoring/profile-assignments")) {
    if (other.profile_id === id || other.scope_type !== scopeType || (other.scope_id ?? null) !== scopeId) continue;
    if (draft.target && other.target && draft.target !== other.target) continue;
    const owner = findById("monitoring/profiles", String(other.profile_id));
    if (!owner) continue;
    const reported = new Set<string>();
    for (const item of profileAssignmentItems(other)) {
      const job = typeof item.effective_job === "string" ? item.effective_job : "";
      if (item.enabled !== true || !jobs.has(job) || reported.has(job)) continue;
      reported.add(job);
      const ownerId = typeof other.profile_id === "string" ? other.profile_id : "";
      const name = typeof owner.name === "string" ? owner.name : "";
      const slug = typeof owner.slug === "string" ? owner.slug : "";
      const target = typeof other.target === "string" ? other.target : null;
      conflicts.push({ profile_id: ownerId, profile_name: name, profile_slug: slug, job, target, message: `job ${job} and target ${target ?? "*"} are already assigned by profile ${slug}` });
    }
  }
  return HttpResponse.json({ hosts, targets_count: targets.length, groups: targets.map(target => ({ targets: [target.target], labels: target.labels })), conflicts });
}


// SD label-output toggles (SdLabelSettingsResponse). Mutable in-session.
const DEMO_SD_LABELS = {
  groups: {
    contacts: false,
    description: false,
    hardware: true,
    identifiers: false,
    location: true,
    network: true,
    role: true,
    status: true,
    tenant: true,
    device_statuses: ["active"],
    site_statuses: ["active"],
  } as Record<string, unknown>,
};

export const handlers = [
  http.get("/api/v1/rum/config", () =>
    HttpResponse.json({ enabled: true, default_enabled: true })),
  http.post("/api/v1/rum/report", () => new HttpResponse(null, { status: 204 })),
  // Auth — keep AppShell authenticated and the login screen consistent.
  http.get("/api/v1/auth/me", () => HttpResponse.json(SESSION_USER)),
  http.post("/api/v1/auth/login", () => HttpResponse.json(SESSION_USER)),
  http.post("/api/v1/auth/logout", () => new HttpResponse(null, { status: 204 })),
  http.post("/api/v1/auth/session-refresh", () => HttpResponse.json({ status: "ok" })),
  http.get("/api/v1/auth/providers", () =>
    HttpResponse.json({ local_enabled: true, oidc_enabled: false, ldap_enabled: false, ad_enabled: false, remember_device_ttl_seconds: 2592000 }),
  ),

  // Assistant — bootstrap only. The demo runs the assistant shell read-only
  // (no session_shell / chat_messages) so it never falls through to the portal
  // backend, whose public-docs `/assistant/bootstrap` returns a different shape
  // (no `capabilities`/`access`), which previously crashed the embedded demo.
  http.get("/api/v1/assistant/bootstrap", () =>
    HttpResponse.json({
      feature_status: "ready",
      access: {
        can_execute: false,
        can_open: true,
        can_update_preferences: false,
        can_view_conversations: false,
      },
      capabilities: {
        chat_messages: false,
        chat_streaming: false,
        mcp_tools: false,
        portal_context_packets: false,
        session_shell: false,
      },
      config: {
        api_key_configured: true,
        archive_retention_days: 30,
        base_url_configured: true,
        instance_enabled: true,
        max_sessions_per_user: 5,
        model: "demo-assistant",
        portal_context_enabled: false,
        provider: "Demo",
      },
    }),
  ),

  // Canonical settings.
  http.get("/api/v1/settings/effective", ({ request }) => {
    const url = new URL(request.url);
    const keys = (url.searchParams.get("keys") ?? "")
      .split(",")
      .map((key) => key.trim())
      .filter(Boolean);
    const allKeys = keys.length
      ? keys
      : [...new Set([...userPreferences.keys(), ...instanceSettings.keys()])].sort((a, b) =>
          a.localeCompare(b),
        );
    return HttpResponse.json({ settings: allKeys.map(effectiveSetting) });
  }),
  http.get("/api/v1/settings/effective/:key", ({ params }) =>
    HttpResponse.json(effectiveSetting(String(params.key))),
  ),
  http.put("/api/v1/settings/user/:key", async ({ params, request }) => {
    const body = await request.json().catch(() => undefined);
    const value =
      body && typeof body === "object" && "value" in body
        ? (body as { value: unknown }).value
        : null;
    userPreferences.set(String(params.key), value);
    return new HttpResponse(null, { status: 204 });
  }),
  http.delete("/api/v1/settings/user/:key", ({ params }) => {
    userPreferences.delete(String(params.key));
    return new HttpResponse(null, { status: 204 });
  }),
  http.get("/api/v1/admin/settings/defaults", () =>
    HttpResponse.json({
      settings: [...instanceSettings.entries()].map(([key, value]) => ({
        key,
        value,
        updated_at: iso(0),
        updated_by_user_id: "00000000-0000-0000-0000-000000000001",
      })),
    }),
  ),
  http.put("/api/v1/admin/settings/defaults/:key", async ({ params, request }) => {
    const body = await request.json().catch(() => undefined);
    const value =
      body && typeof body === "object" && "value" in body
        ? (body as { value: unknown }).value
        : null;
    const key = String(params.key);
    instanceSettings.set(key, value);
    return HttpResponse.json({
      key,
      value,
      updated_at: iso(0),
      updated_by_user_id: "00000000-0000-0000-0000-000000000001",
    });
  }),
  http.delete("/api/v1/admin/settings/defaults/:key", ({ params }) => {
    instanceSettings.delete(String(params.key));
    return new HttpResponse(null, { status: 204 });
  }),

  // Legacy user preference compatibility — override-only.
  http.get("/api/v1/user/settings/preferences/:key", ({ params }) =>
    HttpResponse.json({
      key: String(params.key),
      value: userPreferences.has(String(params.key))
        ? userPreferences.get(String(params.key))
        : null,
    }),
  ),
  http.put("/api/v1/user/settings/preferences/:key", async ({ params, request }) => {
    const body = await request.json().catch(() => undefined);
    const value =
      body && typeof body === "object" && "value" in body
        ? (body as { value: unknown }).value
        : null;
    userPreferences.set(String(params.key), value);
    return new HttpResponse(null, { status: 204 });
  }),

  // Connection targets (Connections workspace tree).
  http.get("/api/v1/connections/targets", ({ request }) => demoConnectionTargetsResponse(request)),
  http.get("/api/v1/connections/targets/:id", ({ params }) => {
    const host = HOSTS.find((target) => target.id === String(params.id));
    if (!host) return HttpResponse.json({ code: "NOT_FOUND", message: "Connection target not found" }, { status: 404 });
    const target: components["schemas"]["ConnectionTargetListRowResponse"] = { ...host, address_bound: true };
    return HttpResponse.json(target);
  }),
  http.get("/api/v1/connections/targets/:id/launch-options", ({ params }) => {
    const id = String(params.id);
    return HttpResponse.json({ host_id: id, protocols: LAUNCH_OPTIONS[id] ?? emptyLaunch });
  }),
  http.get("/api/v1/connections/targets/:id/effective-config", () =>
    HttpResponse.json({ data: {}, sources: [] }),
  ),
  // Assignment/inheritance resolution belongs to the backend, not a second
  // implementation in MSW. A successful list envelope crashes this consumer.
  http.get("/api/v1/connection-config/effective/:deviceId", () =>
    HttpResponse.json({ code: "NOT_IMPLEMENTED", message: "Effective configuration requires a backend-backed instance." }, { status: 501 }),
  ),

  // ── CCTV console ───────────────────────────────────────────────────────────
  // In a real install these 400 until device credentials are configured; the
  // demo answers them fully so the console reads as a working install.
  //
  // `/capabilities` is load-bearing: the console derives EVERY tab from it, so
  // without this handler the generic CRUD fallback returns a list envelope, the
  // permissions object comes back undefined, and the console renders an empty
  // tab strip. It must also stay ahead of `...crudHandlers()` below.
  //
  // The two demo devices deliberately differ, because the capability matrix is
  // per family: the Sofia recorder exposes everything, while the ONVIF camera is
  // view-only (its driver refuses user management, disk and recording status).
  http.get("/api/v1/cctv/:targetId/capabilities", ({ params }) => {
    const onvif = params.targetId === H_CAM_ONVIF.id;
    return HttpResponse.json({
      info: true,
      channels: true,
      snapshot: true,
      // Live MJPEG is a server-side transcode with no equivalent in a mocked
      // demo; reporting it unsupported puts the console on its stills path,
      // which is a real product mode rather than a demo-only shim.
      streaming: false,
      users: !onvif,
      user_update: !onvif,
      authority_catalog: !onvif,
      disk: !onvif,
      recording: !onvif,
      recording_search: !onvif,
      export: !onvif,
      ptz: !onvif,
      permissions: {
        view: true,
        recordings: !onvif,
        export: !onvif,
        users: !onvif,
        settings: !onvif,
        ptz: !onvif,
      },
    });
  }),
  http.get("/api/v1/cctv/:targetId/channels", ({ params }) =>
    HttpResponse.json({
      channels:
        params.targetId === H_CAM_ONVIF.id
          ? [{ index: 1, name: "Entrance", enabled: true, ptz_support: "unsupported", recording: false }]
          : [
              { index: 1, name: "Lobby", enabled: true, ptz_support: "supported", recording: true },
              { index: 2, name: "Parking P1", enabled: true, ptz_support: "supported", recording: true },
              { index: 3, name: "Warehouse A", enabled: true, ptz_support: "unsupported", recording: true },
              { index: 4, name: "Gate / Entrance", enabled: true, ptz_support: "supported", recording: true },
            ],
    }),
  ),
  // Binary endpoints answer with generated stand-in media (see ./cctv-media).
  http.get("/api/v1/cctv/:targetId/snapshot", ({ request }) => {
    const channel = Number(new URL(request.url).searchParams.get("channel") ?? 1);
    const bytes = cctvFrameBytes(Number.isFinite(channel) && channel > 0 ? channel : 1);
    return new HttpResponse(bytes, {
      headers: { "Content-Type": "image/jpeg", "Content-Length": String(bytes.byteLength) },
    });
  }),
  http.get("/api/v1/cctv/:targetId/playback", () => {
    const bytes = cctvClipBytes();
    return new HttpResponse(bytes, {
      headers: {
        "Content-Type": "video/mp4",
        "Content-Length": String(bytes.byteLength),
        "Content-Disposition": "inline",
      },
    });
  }),
  http.get("/api/v1/cctv/:targetId/export", () => {
    const bytes = cctvClipBytes();
    return new HttpResponse(bytes, {
      headers: {
        "Content-Type": "video/mp4",
        "Content-Length": String(bytes.byteLength),
        "Content-Disposition": 'attachment; filename="clip.mp4"',
      },
    });
  }),
  // PTZ answers 204 in production, but the demo must return a body: the client
  // unwraps the envelope and treats an undefined body as an error.
  http.post("/api/v1/cctv/:targetId/ptz", () => HttpResponse.json({ ok: true })),
  http.get("/api/v1/cctv/:targetId/authorities", () =>
    HttpResponse.json({
      authorities: [
        "all", "live", "playback", "config", "ptz", "export", "search", "shutdown",
      ],
    }),
  ),
  http.get("/api/v1/cctv/:targetId/info", () =>
    HttpResponse.json({
      model: "NVR-16",
      firmware: "V5.2.1 build 240118",
      serial: "NVR16-2400-000482",
      channels: 4,
      device_time: iso(1),
    }),
  ),
  http.get("/api/v1/device-fleet/:targetId/users", () =>
    HttpResponse.json({
      users: [
        { name: "admin", group: "Administrators", authorities: ["all", "playback", "config", "ptz"] },
        { name: "operator", group: "Operators", authorities: ["playback", "live"] },
        { name: "viewer", group: "Guests", authorities: ["live"] },
      ],
    }),
  ),
  http.get("/api/v1/cctv/:targetId/disk", () =>
    HttpResponse.json({
      disks: [
        { name: "HDD1", total_bytes: 4_000_000_000_000, used_bytes: 3_760_000_000_000, state: "ok", is_error: false },
        { name: "HDD2", total_bytes: 4_000_000_000_000, used_bytes: 1_240_000_000_000, state: "ok", is_error: false },
      ],
    }),
  ),
  http.get("/api/v1/cctv/:targetId/recording", () =>
    HttpResponse.json({
      active: true,
      oldest: iso(30 * 24 * 60),
      newest: iso(1),
      spans: [
        { start: iso(30 * 24 * 60), end: iso(18 * 24 * 60) },
        { start: iso(18 * 24 * 60), end: iso(6 * 24 * 60) },
        { start: iso(6 * 24 * 60), end: iso(1) },
      ],
    }),
  ),
  // Per-channel archive search backing the timeline. Continuous recording with
  // two gaps, so the timeline shows something more honest than a solid bar.
  http.get("/api/v1/cctv/:targetId/recordings", ({ request }) => {
    const url = new URL(request.url);
    const channel = Number(url.searchParams.get("channel") ?? 1) || 1;
    const dayStart = Date.parse(url.searchParams.get("start") ?? "") || NOW - 24 * 60 * 60_000;
    const at = (h: number, m = 0) => new Date(dayStart + (h * 60 + m) * 60_000).toISOString();
    return HttpResponse.json({
      spans: [
        { start: at(0), end: at(7, 12), channel },
        { start: at(7, 26), end: at(14, 3), channel },
        { start: at(14, 20), end: at(23, 59), channel },
      ],
    });
  }),
  // On-device account writes. Create/delete are preview-then-confirm; editing an
  // existing account is a direct PUT with no dry run, matching the product.
  http.post("/api/v1/device-fleet/:targetId/users:preview", async ({ request }) => {
    const body = (await request.json().catch(() => ({}))) as {
      op?: string;
      users?: { name?: string }[];
      usernames?: string[];
    };
    const existing = new Set(["admin", "operator", "viewer"]);
    const perItem =
      body.op === "delete"
        ? (body.usernames ?? []).map((name) => ({
            name,
            action: existing.has(name) ? "delete" : "missing",
            reason: existing.has(name) ? null : "no such account on the device",
          }))
        : (body.users ?? []).map((user) => ({
            name: user.name ?? "",
            action: existing.has(user.name ?? "") ? "skip" : "create",
            reason: existing.has(user.name ?? "") ? "account already exists" : null,
          }));
    return HttpResponse.json({ per_item: perItem, writes_enabled: true });
  }),
  http.post("/api/v1/device-fleet/:targetId/users:create", async ({ request }) => {
    const body = (await request.json().catch(() => ({}))) as { users?: { name?: string }[] };
    return HttpResponse.json({
      results: (body.users ?? []).map((user) => ({ name: user.name ?? "", ok: true, error: null })),
    });
  }),
  http.post("/api/v1/device-fleet/:targetId/users:delete", async ({ request }) => {
    const body = (await request.json().catch(() => ({}))) as { usernames?: string[] };
    return HttpResponse.json({
      results: (body.usernames ?? []).map((name) => ({ name, ok: true, error: null })),
    });
  }),
  http.put("/api/v1/device-fleet/:targetId/users", async ({ request }) => {
    const body = (await request.json().catch(() => ({}))) as { users?: { name?: string }[] };
    return HttpResponse.json({
      results: (body.users ?? []).map((user) => ({ name: user.name ?? "", ok: true, error: null })),
    });
  }),

  http.get("/api/v1/connections/filter-presets", () =>
    HttpResponse.json({ items: DEMO_PUBLIC_FILTER_PRESETS }),
  ),
  http.post("/api/v1/connections/filter-presets", async ({ request }) => {
    const body = await readBody(request);
    const now = iso(0);
    const preset = {
      id: `preset-public-${Date.now().toString(16)}`,
      name: typeof body.name === "string" ? body.name : "Saved filter",
      filters:
        body.filters && typeof body.filters === "object" && !Array.isArray(body.filters)
          ? (body.filters as Record<string, string[]>)
          : {},
      created_by: "00000000-0000-0000-0000-000000000001",
      created_at: now,
      updated_at: now,
    };
    DEMO_PUBLIC_FILTER_PRESETS.unshift(preset);
    return HttpResponse.json(preset, { status: 201 });
  }),
  http.put("/api/v1/connections/filter-presets/:id", async ({ params, request }) => {
    const body = await readBody(request);
    const preset = DEMO_PUBLIC_FILTER_PRESETS.find((item) => item.id === params.id);
    if (!preset) return new HttpResponse(null, { status: 404 });
    if (typeof body.name === "string") preset.name = body.name;
    if (body.filters && typeof body.filters === "object" && !Array.isArray(body.filters)) {
      preset.filters = body.filters as Record<string, string[]>;
    }
    preset.updated_at = iso(0);
    return HttpResponse.json(preset);
  }),
  http.delete("/api/v1/connections/filter-presets/:id", ({ params }) => {
    const index = DEMO_PUBLIC_FILTER_PRESETS.findIndex((item) => item.id === params.id);
    if (index >= 0) DEMO_PUBLIC_FILTER_PRESETS.splice(index, 1);
    return new HttpResponse(null, { status: index >= 0 ? 204 : 404 });
  }),

  // Audit target lookup (server-side search for audit filter pickers).
  http.get("/api/v1/audit/targets", ({ request }) => auditTargetsResponse(request)),
  http.get("*/api/v1/audit/targets", ({ request }) => auditTargetsResponse(request)),
  http.get("*/api/v1/audit/events/:id", ({ params }) => {
    const event = findById("audit/events", String(params.id));
    return event ? HttpResponse.json(event) : new HttpResponse(null, { status: 404 });
  }),
  http.get("*/api/v1/audit/events", ({ request }) => {
    const url = new URL(request.url);
    const opts = listOpts(url);
    const matches = rows("audit/events").filter(event => {
      const context = event.context as { outcome: string; state?: { domain: string; value: string }; correlation: Record<string, string> };
      const values: Record<string, unknown> = { ...event, outcome: context.outcome, state_domain: context.state?.domain, state_value: context.state?.value, ...context.correlation };
      for (const key of ["event_type", "actor_kind", "actor_id", "target_type", "target_id", "outcome", "state_domain", "state_value", "request_id", "session_id", "run_id", "run_group_id"]) {
        const filter = url.searchParams.get(key);
        if (filter && (typeof values[key] !== "string" || !filter.split(",").includes(values[key]))) return false;
      }
      if (!matchesAuditSearch(event, url.searchParams.get("q") ?? "")) return false;
      const created = typeof event.created_at === "string" ? event.created_at : "";
      const from = url.searchParams.get("created_from");
      const to = url.searchParams.get("created_to");
      return (!from || created >= from) && (!to || created <= to);
    });
    const sort = url.searchParams.get("sort");
    const direction = url.searchParams.get("order") === "desc" ? -1 : 1;
    if (sort && ["created_at", "event_type", "actor", "target_type", "outcome"].includes(sort)) {
      const value = (event: typeof matches[number]) => {
        if (sort === "outcome") return (event.context as { outcome: string }).outcome;
        const field = sort === "actor" ? event.actor_id : event[sort];
        return typeof field === "string" ? field : "";
      };
      matches.sort((a, b) => direction * value(a).localeCompare(value(b)));
    }
    const limit = opts.limit ?? 25;
    const offset = opts.offset ?? 0;
    return HttpResponse.json({ items: matches.slice(offset, offset + limit), total: matches.length, limit, offset });
  }),

  // Instance branding (Appearance / Administration). Backend always answers a
  // BrandingResponse; the store-less default keeps the demo app name + theme.
  http.get("/api/v1/instance/branding", () => HttpResponse.json(DEMO_BRANDING)),
  http.put("/api/v1/instance/branding", async ({ request }) => {
    const body = await readBody(request);
    Object.assign(DEMO_BRANDING, body, { updated_at: iso(0) });
    return HttpResponse.json(DEMO_BRANDING);
  }),

  // API reference (Swagger UI) — serve the REAL bundled OpenAPI contract as YAML
  // with the proper content-type so Swagger renders it (a valid `openapi: 3.1.0`
  // document with a version field). Registered before the generic CRUD/catch-all
  // handlers so the 2-segment `openapi/<file>` path isn't swallowed as a list.
  // Previously this fell through to the catch-all which returned a JSON envelope,
  // so Swagger reported "does not specify a valid version field".
  http.get("/api/v1/openapi/piping.openapi.yaml", () => openapiResponse()),
  http.get("*/api/v1/openapi/piping.openapi.yaml", () => openapiResponse()),

  // Connections / sessions.
  http.get("/api/v1/connections/sessions", ({ request }) =>
    demoSessionList(new URL(request.url)),
  ),
  http.post("/api/v1/connections/sessions", async ({ request }) => {
    // Read the requested host + protocol so the demo can (a) report the right
    // protocol back and (b) hand SSH a per-host relay URL the demo WS shim
    // (ssh-socket-shim.ts) intercepts to stream that host's canned shell transcript.
    let targetId = H_WEB.id;
    let protocol = "ssh";
    try {
      const body = (await request.clone().json()) as { target_id?: string; protocol?: string };
      if (typeof body.target_id === "string") targetId = body.target_id;
      if (typeof body.protocol === "string") protocol = body.protocol;
    } catch {
      // Body unreadable — fall back to defaults; the pane still opens.
    }
    const host = HOSTS.find((h) => h.id === targetId);
    const hostName = host?.name ?? "linux-host";
    const sessionId = crypto.randomUUID();
    // SSH gets a host-scoped relay path the WS handler matches on; RDP/VNC are
    // rendered as static mock desktops in the viewer (no live socket needed) so
    // any URL is fine for them.
    const websocketUrl =
      protocol === "ssh"
        ? `wss://demo.invalid/relay/ssh/${encodeURIComponent(hostName)}`
        : `wss://demo.invalid/relay/${protocol}/${encodeURIComponent(hostName)}`;
    return HttpResponse.json({
      session: syntheticSession({
        ...ACTIVE_SESSIONS[0],
        id: sessionId,
        target_id: targetId,
        protocol,
        status: "active",
        started_at: iso(0),
        last_activity: iso(0),
      }),
      proxy: {
        connection_id: sessionId,
        websocket_url: websocketUrl,
        token: "demo-token",
        token_ttl_seconds: 60,
      },
    });
  }),
  http.get("/api/v1/connections/sessions/:id/health", ({ params }) =>
    demoSessionHealth(String(params.id)),
  ),
  http.get("*/api/v1/connections/sessions/:id/health", ({ params }) =>
    demoSessionHealth(String(params.id)),
  ),
  http.post("/api/v1/connections/sessions/:id/disconnect", () => new HttpResponse(null, { status: 204 })),
  http.post("/api/v1/connections/sessions/:id/terminate", () => new HttpResponse(null, { status: 204 })),

  // Observability — the target inspector reads `data.target.metrics`; return a
  // well-formed envelope with no configured metrics so the section renders an
  // empty state rather than throwing on an undefined `target`.
  http.get("/api/v1/monitoring/targets/:id", ({ params }) =>
    HttpResponse.json({
      target: { id: String(params.id), metrics: [], inspector_metrics: [] },
      scrape_configs: [],
    }),
  ),
  http.get("/api/v1/monitoring/latest", () => HttpResponse.json({ items: [] })),
  http.get("/api/v1/monitoring/host-labels", ({ request }) =>
    HttpResponse.json({
      hosts: new URL(request.url).searchParams.getAll("host_id").map(host_id => ({
        host_id,
        metrics: [],
        updated_at: null,
        stale: true,
        alerts: [],
        alerts_updated_at: null,
        alerts_stale: true,
      })),
    } satisfies components["schemas"]["HostLabelsResponse"]),
  ),
  // Service Discovery preview — built from the seeded hosts so the SD screen's
  // discovery preview + the "targets / groups" stat cards show real numbers
  // (ServiceDiscoveryPreviewResponse: groups[], groups_count, hosts[], targets_count).
  http.get("/api/v1/monitoring/sd/preview", () => HttpResponse.json(buildSdPreview())),
  http.get("*/api/v1/monitoring/sd/preview", () => HttpResponse.json(buildSdPreview())),
  // Coverage view — the Monitoring section on host/role/site detail pages.
  http.get("/api/v1/monitoring/coverage", ({ request }) => HttpResponse.json(buildMonitoringCoverage(request))),
  http.get("*/api/v1/monitoring/coverage", ({ request }) => HttpResponse.json(buildMonitoringCoverage(request))),
  http.get("*/api/v1/monitoring/profiles/:id/preview", ({ params, request }) => buildProfilePreview(String(params.id), request)),
  // SD label settings — the "labels in output" toggle group. Mutable in-session.
  http.get("/api/v1/monitoring/sd/label-settings", () => HttpResponse.json(DEMO_SD_LABELS)),
  http.put("/api/v1/monitoring/sd/label-settings", async ({ request }) => {
    const body = await readBody(request);
    if (body.groups && typeof body.groups === "object") {
      Object.assign(DEMO_SD_LABELS.groups, body.groups as Record<string, unknown>);
    }
    return HttpResponse.json(DEMO_SD_LABELS);
  }),

  // License state (EntitlementState) — a healthy activated license so the
  // Administration → License screen renders seats/validity instead of empty.
  http.get("/api/v1/license/state", () =>
    HttpResponse.json({
      status: "active",
      seatLimit: 50,
      seatsUsed: 12,
      validFrom: iso(60 * 24 * 200),
      validUntil: iso(-60 * 24 * 165),
      version: 1,
    }),
  ),

  // Scope fields consume the native bare-array metadata contract. These demo
  // template classes have no Tag memberships; the other declared classes do.
  http.get("/api/v1/inventory/content-types", () =>
    HttpResponse.json([
      { id: DEMO_APPLICATION_CONTENT_TYPE, app_label: "inventory", model: "application", token: "inventory.application", label: "Application", supported_for_custom_fields: true },
      { id: "ct-application-role", app_label: "inventory", model: "applicationrole", token: "inventory.applicationrole", label: "Application Role", supported_for_custom_fields: false },
      { id: "ct-dataflow-group", app_label: "inventory", model: "dataflowgroup", token: "inventory.dataflowgroup", label: "Flow Group", supported_for_custom_fields: true },
      { id: "ct-address-group", app_label: "inventory", model: "addressgroup", token: "inventory.addressgroup", label: "Address Group", supported_for_custom_fields: true },
      { id: "ct-dataflow", app_label: "inventory", model: "dataflow", token: "inventory.dataflow", label: "Dataflow", supported_for_custom_fields: true },
      { id: "ct-wireless-lan-group", app_label: "wireless", model: "wirelesslangroup", token: "wireless.wirelesslangroup", label: "Wireless LAN Group", supported_for_custom_fields: true },
      { id: "ct-wireless-lan", app_label: "wireless", model: "wirelesslan", token: "wireless.wirelesslan", label: "Wireless LAN", supported_for_custom_fields: true },
      { id: "ct-wireless-link", app_label: "wireless", model: "wirelesslink", token: "wireless.wirelesslink", label: "Wireless Link", supported_for_custom_fields: true },
      { id: "ct-device", app_label: "dcim", model: "device", token: "dcim.device", label: "Device", supported_for_custom_fields: true },
      { id: "ct-vm", app_label: "virtualization", model: "virtualmachine", token: "virtualization.virtualmachine", label: "Virtual Machine", supported_for_custom_fields: true },
      { id: "ct-site", app_label: "dcim", model: "site", token: "dcim.site", label: "Site", supported_for_custom_fields: true },
      { id: "ct-rack", app_label: "dcim", model: "rack", token: "dcim.rack", label: "Rack", supported_for_custom_fields: false },
      { id: "ct-interface", app_label: "dcim", model: "interface", token: "dcim.interface", label: "Interface", supported_for_custom_fields: false },
      { id: "ct-region", app_label: "dcim", model: "region", token: "dcim.region", label: "Region", supported_for_custom_fields: true },
      { id: "ct-cluster", app_label: "virtualization", model: "cluster", token: "virtualization.cluster", label: "Cluster", supported_for_custom_fields: true },
      { id: "ct-tenant", app_label: "tenancy", model: "tenant", token: "tenancy.tenant", label: "Tenant", supported_for_custom_fields: true },
      { id: "ct-vm-type", app_label: "virtualization", model: "virtualmachinetype", token: "virtualization.virtualmachinetype", label: "Virtual Machine Type", supported_for_custom_fields: true },
      { id: "ct-rack-group", app_label: "dcim", model: "rackgroup", token: "dcim.rackgroup", label: "Rack Group", supported_for_custom_fields: true },
      { id: "ct-cable-bundle", app_label: "dcim", model: "cablebundle", token: "dcim.cablebundle", label: "Cable Bundle", supported_for_custom_fields: true },
      { id: "ct-fhrp-group", app_label: "ipam", model: "fhrpgroup", token: "ipam.fhrpgroup", label: "FHRP Group", supported_for_custom_fields: true },
      // Native component-template content types (migration 0013). These are the
      // only eligible InventoryItemTemplate component targets.
      { id: "ct-dcim-consoleporttemplate", app_label: "dcim", model: "consoleporttemplate", token: "dcim.consoleporttemplate", label: "Console Port Template", supported_for_custom_fields: false },
      { id: "ct-dcim-consoleserverporttemplate", app_label: "dcim", model: "consoleserverporttemplate", token: "dcim.consoleserverporttemplate", label: "Console Server Port Template", supported_for_custom_fields: false },
      { id: "ct-dcim-coolingintaketemplate", app_label: "dcim", model: "coolingintaketemplate", token: "dcim.coolingintaketemplate", label: "Cooling Intake Template", supported_for_custom_fields: false },
      { id: "ct-dcim-coolingoutflowtemplate", app_label: "dcim", model: "coolingoutflowtemplate", token: "dcim.coolingoutflowtemplate", label: "Cooling Outflow Template", supported_for_custom_fields: false },
      { id: "ct-dcim-frontporttemplate", app_label: "dcim", model: "frontporttemplate", token: "dcim.frontporttemplate", label: "Front Port Template", supported_for_custom_fields: false },
      { id: "ct-dcim-interfacetemplate", app_label: "dcim", model: "interfacetemplate", token: "dcim.interfacetemplate", label: "Interface Template", supported_for_custom_fields: false },
      { id: "ct-dcim-poweroutlettemplate", app_label: "dcim", model: "poweroutlettemplate", token: "dcim.poweroutlettemplate", label: "Power Outlet Template", supported_for_custom_fields: false },
      { id: "ct-dcim-powerporttemplate", app_label: "dcim", model: "powerporttemplate", token: "dcim.powerporttemplate", label: "Power Port Template", supported_for_custom_fields: false },
      { id: "ct-dcim-rearporttemplate", app_label: "dcim", model: "rearporttemplate", token: "dcim.rearporttemplate", label: "Rear Port Template", supported_for_custom_fields: false },
    ].map(type => ({ ...type, capabilities: {
      tags: type.model !== "applicationrole" && !type.model.endsWith("template"),
      custom_fields: type.supported_for_custom_fields,
      change_dates: !type.model.endsWith("template") || type.model.startsWith("cooling"),
    } })) satisfies components["schemas"]["ContentTypeResponse"][]),
  ),
  http.post("/api/v1/inventory/dataflows/endpoint-resolution", async ({ request }) =>
    demoDataflowEndpointResolution(await readBody(request)),
  ),
  http.post("/api/v1/inventory/dataflows/bulk-action", async ({ request }) => {
    const body = await readBody(request);
    const ids = [...new Set(Array.isArray(body.ids) ? body.ids.filter((id): id is string => typeof id === "string") : [])];
    const action = body.action;
    const flows = rows("inventory/dataflows");
    const target = ids.map(id => flows.find(flow => flow.id === id));
    if (!ids.length || target.some(flow => !flow) || !["set_status", "move", "delete"].includes(String(action))) {
      return HttpResponse.json({ detail: "Invalid bulk selection." }, { status: 400 });
    }
    if (action === "set_status" && !["draft", "active", "disabled"].includes(String(body.status))) {
      return HttpResponse.json({ detail: "Invalid status." }, { status: 400 });
    }
    if (action === "move" && body.group_id === undefined && body.application_id === undefined) {
      return HttpResponse.json({ detail: "Nothing to move." }, { status: 400 });
    }
    for (const flow of target) {
      if (!flow) continue;
      if (action === "delete") { remove("inventory/dataflows", flow.id); continue; }
      update("inventory/dataflows", flow.id, shapeDataflow({
        ...flow,
        ...(action === "set_status" ? { status: body.status } : {}),
        ...(action === "move" ? {
          ...(body.group_id !== undefined ? { group_id: body.group_id } : {}),
          ...(body.application_id !== undefined ? { application_id: body.application_id } : {}),
        } : {}),
      }));
    }
    return HttpResponse.json({ action, affected: ids.map(id => ({ id })) });
  }),
  http.delete("/api/v1/inventory/application-roles/:id", ({ params }) => {
    const id = String(params.id);
    return rows("inventory/applications").some(item => item.role_id === id)
      ? HttpResponse.json({ detail: "Application role is in use." }, { status: 409 })
      : (remove("inventory/application-roles", id), new HttpResponse(null, { status: 204 }));
  }),
  http.delete("/api/v1/inventory/applications/:id", ({ params }) => {
    const id = String(params.id);
    const referenced = rows("inventory/dataflows").some(item => item.application_id === id) || rows("inventory/dataflow-groups").some(item => item.application_id === id)
      || rows("inventory/contact-assignments").some(item => item.object_type === "inventory.application" && item.object_id === id);
    return referenced
      ? HttpResponse.json({ detail: "Application is referenced." }, { status: 409 })
      : (remove("inventory/applications", id), new HttpResponse(null, { status: 204 }));
  }),
  http.delete("/api/v1/inventory/dataflow-groups/:id", ({ params }) => {
    const id = String(params.id);
    const referenced = rows("inventory/dataflows").some(item => item.group_id === id) || rows("inventory/dataflow-groups").some(item => item.parent_id === id);
    return referenced
      ? HttpResponse.json({ detail: "Group is referenced." }, { status: 409 })
      : (remove("inventory/dataflow-groups", id), new HttpResponse(null, { status: 204 }));
  }),
  http.delete("/api/v1/inventory/address-groups/:id", ({ params }) => {
    const id = String(params.id);
    const referenced = rows("inventory/dataflows").some(flow => [...shapeEndpoints(flow.sources), ...shapeEndpoints(flow.destinations)]
      .some(endpoint => endpoint.object_type === "inventory.addressgroup" && endpoint.object_id === id));
    return referenced
      ? HttpResponse.json({ detail: "Address group is referenced." }, { status: 409 })
      : (remove("inventory/address-groups", id), new HttpResponse(null, { status: 204 }));
  }),
  http.get("/api/v1/inventory/prefixes/tree", ({ request }) =>
    demoPrefixTreeResponse(request),
  ),
  http.get("/api/v1/inventory/prefixes/:id/next-available-prefix/preview", ({ request }) =>
    demoPrefixPlanningPreview(request),
  ),
  http.get("/api/v1/inventory/cabling/endpoints", ({ request }) =>
    demoCablingEndpoints(request),
  ),
  http.get("/api/v1/inventory/cabling/trace", ({ request }) =>
    demoCablingTrace(request),
  ),

  // Permission vocabulary — the single source-of-truth dictionary the policy
  // editor (and token scope picker) render from. Representative full catalog:
  // every Action × every ResourceType, all enforced and PAT-grantable.
  http.get("/api/v1/access-control/vocabulary", () =>
    HttpResponse.json({
      actions: [...DEMO_ACTIONS],
      resources: DEMO_RESOURCE_TYPES.map((resource_type) => ({
        resource_type,
        status: "enforced",
        pat_grantable_actions: [...DEMO_ACTIONS],
        // Only CONNECTION_TARGET is attribute-scopable today; the policy editor
        // offers exactly these keys for an `attributes` constraint.
        constraint_attribute_keys:
          resource_type === "CONNECTION_TARGET"
            ? ["role_id", "site_id", "tenant_id", "platform_id", "cluster_id", "status", "protocol"]
            : [],
      })),
    }),
  ),

  http.get("/api/v1/playbooks/run-groups/:id", ({ params }) => {
    const group = findById("playbooks/run-groups", String(params.id));
    return group ? HttpResponse.json({ run_group: group, runs: rows("playbooks/runs").filter((run) => run.run_group_id === group.id) }) : new HttpResponse(null, { status: 404 });
  }),

  http.get("/api/v1/playbooks/projects/:id/revisions/:revisionId", ({ params }) => {
    const revision = demoRevisions.get(String(params.revisionId));
    return revision?.project_id === params.id && findById("playbooks/projects", String(params.id))
      ? HttpResponse.json(revision) : new HttpResponse(null, { status: 404 });
  }),
  http.post("/api/v1/playbooks/projects/:id/revisions", async ({ params, request }) => {
    const project = findById("playbooks/projects", String(params.id));
    if (!project) return new HttpResponse(null, { status: 404 });
    const body = await readBody(request);
    if (typeof body.base_revision_id !== "string" || !Array.isArray(body.changes)) return new HttpResponse(null, { status: 400 });
    if (project.source && typeof project.source === "object" && "kind" in project.source && project.source.kind !== "editable") return new HttpResponse(null, { status: 400 });
    if (project.current_revision_id !== body.base_revision_id) return new HttpResponse(null, { status: 409 });
    const base = demoRevisions.get(body.base_revision_id);
    if (!base || base.project_id !== project.id) return new HttpResponse(null, { status: 404 });
    const entries = new Map(base.entries.map((entry) => [entry.path, structuredClone(entry)]));
    for (const change of body.changes as unknown[]) {
      if (!change || typeof change !== "object" || !("operation" in change)) return new HttpResponse(null, { status: 400 });
      if (change.operation === "put" && "entry" in change && validEntry(change.entry)) entries.set(change.entry.path, structuredClone(change.entry));
      else if (change.operation === "delete" && "path" in change && typeof change.path === "string" && entries.has(change.path)) entries.delete(change.path);
      else if (change.operation === "rename" && "from" in change && "to" in change && typeof change.from === "string" && typeof change.to === "string") {
        const entry = entries.get(change.from);
        if (!entry || entries.has(change.to) || !validEntry({ ...entry, path: change.to })) return new HttpResponse(null, { status: 400 });
        entries.delete(change.from);
        entries.set(change.to, { ...entry, path: change.to });
      } else return new HttpResponse(null, { status: 400 });
    }
    const revision: components["schemas"]["ProjectRevision"] = { project_id: project.id, revision_id: crypto.randomUUID(), created_at: new Date().toISOString(), tree_sha256: "0".repeat(64), entries: [...entries.values()] };
    demoRevisions.set(revision.revision_id, revision);
    update("playbooks/projects", project.id, { current_revision_id: revision.revision_id });
    return HttpResponse.json(revision, { status: 201 });
  }),

  http.all("/api/v1/playbooks", automationRequest),
  http.all("/api/v1/playbooks/*", automationRequest),

  // ── Generic stateful CRUD over the in-memory store ─────────────────────────
  // Registered AFTER all specific handlers above (so sessions/branding win)
  // and BEFORE the catch-all. Navigating any covered area shows seeded data and
  // create/edit/delete mutate the store (reflected on the next refetch). A page
  // reload re-imports the store module → re-seeds → demo resets to a clean state.
  ...crudHandlers(),

  // Catch-all: any other GET still returns a valid envelope (stateful where the
  // path maps to a store key, empty otherwise); any write is a benign success so
  // the demo never errors on screens beyond the explicitly modelled ones.
  // Both the relative (`/api/v1/*`, same-origin) and absolute (`*/api/v1/*`)
  // forms are registered so nothing slips through to the network (404).
  http.get("/api/v1/*", catchAllGet),
  http.get("*/api/v1/*", catchAllGet),
  http.post("/api/v1/*", catchAllWrite),
  http.post("*/api/v1/*", catchAllWrite),
  http.put("/api/v1/*", catchAllWrite),
  http.put("*/api/v1/*", catchAllWrite),
  http.patch("/api/v1/*", catchAllWrite),
  http.patch("*/api/v1/*", catchAllWrite),
  http.delete("/api/v1/*", () => new HttpResponse(null, { status: 204 })),
  http.delete("*/api/v1/*", () => new HttpResponse(null, { status: 204 })),
];

// ─────────────────────────────────────────────────────────────────────────────
// Swagger spec response.
// ─────────────────────────────────────────────────────────────────────────────
function openapiResponse(): Response {
  return new HttpResponse(openapiYaml, {
    status: 200,
    headers: { "Content-Type": "application/yaml; charset=utf-8" },
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// Generic CRUD handler generation.
//
// Every store key under `/api/v1/<key>` gets a collection route (GET list,
// POST create) and an item route (GET detail, PUT/PATCH update, DELETE). The
// path after `/api/v1/` IS the store key, so the handlers extract it directly
// from the URL. We register one parameterised handler per *area prefix* so MSW's
// path matcher can isolate `{id}` cleanly even for multi-segment keys like
// `admin/access-control/users`.
// ─────────────────────────────────────────────────────────────────────────────

/** Path prefixes (under /api/v1/) whose sub-resources are store-backed. */
const CRUD_AREAS = [
  "inventory",
  "custom-fields",
  "custom-field-choice-sets",
  "admin/access-control",
  "admin",
  "connection-config",
  "config-contexts",
  "config-context-bindings",
  "connections/templates",
  "webhooks",
  "user/settings",
  "audit",
  "monitoring",
  "playbooks",
] as const;

/** Reserved single-segment endpoints under an area that are NOT plain CRUD. */
const RESERVED_SEGMENTS = new Set(["bulk", "export", "import", "templates"]);

/** Path segments after `/api/v1/`, e.g. ["inventory","sites","abc"]. */
function apiSegments(request: Request): string[] {
  const path = new URL(request.url).pathname.replace(/^.*\/api\/v1\//, "");
  return path.split("/").filter(Boolean);
}

function crudHandlers() {
  // Broad path handlers keyed off the URL: the store key IS the path remainder,
  // so we derive collection key + id from the segments. Specific handlers above
  // (sessions/branding/swagger) are registered earlier and win; the
  // catch-all below catches anything these don't.
  return [
    http.get("/api/v1/:a", ({ request }) => crudList(apiSegments(request).join("/"), request)),
    // GET: list (2-3 area segments) or detail (trailing id).
    http.get("/api/v1/:a/:b", ({ request }) => {
      const s = apiSegments(request);
      const collection = s.slice(0, 1).join("/");
      if (isCollection(collection)) {
        return crudListOrDetail(collection, s[1], s.join("/"), request);
      }
      return crudList(s.join("/"), request);
    }),
    http.get("/api/v1/:a/:b/:c", ({ request }) => {
      const s = apiSegments(request);
      return crudListOrDetail(s.slice(0, 2).join("/"), s[2], s.join("/"), request);
    }),
    http.get("/api/v1/:a/:b/:c/:d", ({ request }) => {
      const s = apiSegments(request);
      return crudListOrDetail(s.slice(0, 3).join("/"), s[3], s.join("/"), request);
    }),
    // POST: create on collection.
    http.post("/api/v1/:a", ({ request }) => crudCreate(apiSegments(request).join("/"), request)),
    http.post("/api/v1/:a/:b", ({ request }) => crudCreate(apiSegments(request).join("/"), request)),
    http.post("/api/v1/:a/:b/:c", ({ request }) => {
      const s = apiSegments(request);
      return crudCreate(s.join("/"), request, s.slice(0, 2).join("/"));
    }),
    http.post("/api/v1/:a/:b/:c/:d", ({ request }) => {
      const s = apiSegments(request);
      return crudCreate(s.join("/"), request, s.slice(0, 3).join("/"));
    }),
    // PUT/PATCH: update item.
    http.put("/api/v1/:a/:b", ({ request }) => {
      const s = apiSegments(request);
      return crudUpdate(s.slice(0, 1).join("/"), s[1], request);
    }),
    http.patch("/api/v1/:a/:b", ({ request }) => {
      const s = apiSegments(request);
      return crudUpdate(s.slice(0, 1).join("/"), s[1], request);
    }),
    http.put("/api/v1/:a/:b/:c", ({ request }) => {
      const s = apiSegments(request);
      return crudUpdate(s.slice(0, 2).join("/"), s[2], request);
    }),
    http.patch("/api/v1/:a/:b/:c", ({ request }) => {
      const s = apiSegments(request);
      return crudUpdate(s.slice(0, 2).join("/"), s[2], request);
    }),
    http.put("/api/v1/:a/:b/:c/:d", ({ request }) => {
      const s = apiSegments(request);
      return crudUpdate(s.slice(0, 3).join("/"), s[3], request);
    }),
    http.patch("/api/v1/:a/:b/:c/:d", ({ request }) => {
      const s = apiSegments(request);
      return crudUpdate(s.slice(0, 3).join("/"), s[3], request);
    }),
    // DELETE: remove item.
    http.delete("/api/v1/:a/:b", ({ request }) => {
      const s = apiSegments(request);
      return crudDelete(s.slice(0, 1).join("/"), s[1]);
    }),
    http.delete("/api/v1/:a/:b/:c", ({ request }) => {
      const s = apiSegments(request);
      return crudDelete(s.slice(0, 2).join("/"), s[2]);
    }),
    http.delete("/api/v1/:a/:b/:c/:d", ({ request }) => {
      const s = apiSegments(request);
      return crudDelete(s.slice(0, 3).join("/"), s[3]);
    }),
  ];
}

function inCrudArea(key: string): boolean {
  return CRUD_AREAS.some((a) => key === a || key.startsWith(`${a}/`));
}

/** Pass non-CRUD paths through to the catch-all by returning `passthrough`-like. */
function notHandled(): Response {
  // Returning an empty valid envelope keeps any unexpected screen from breaking.
  return HttpResponse.json({ items: [], total: 0, limit: 1000, offset: 0 });
}

function crudList(key: string, request: Request): Response {
  if (!inCrudArea(key)) return notHandled();
  return listResponse(key, new URL(request.url));
}

function crudListOrDetail(
  collectionKey: string,
  idOrSub: string | undefined,
  fullKey: string,
  request: Request,
): Response {
  if (!idOrSub) return notHandled();
  // If the full path is itself a registered collection (e.g.
  // `admin/access-control/users`, `user/settings/api-tokens`), this is a LIST
  // request — NOT a detail read of the 2-segment parent area. Without this the
  // trailing segment ("users") is mistaken for an :id, the handler returns a
  // single object instead of `{ items: [...] }`, and the views crash on
  // `data.items` being undefined ("can't access property map, e.items is undefined").
  if (isCollection(fullKey)) return listResponse(fullKey, new URL(request.url));
  // `/api/v1/inventory/templates/{id}` → detail; reserved sub-collections
  // (e.g. `.../bulk`) are not plain detail reads.
  if (RESERVED_SEGMENTS.has(idOrSub)) {
    // Could be a nested collection (e.g. .../runs) — try it as a store key.
    if (inCrudArea(fullKey)) return listResponse(fullKey, new URL(request.url));
    return notHandled();
  }
  if (!inCrudArea(collectionKey)) return notHandled();
  const found = findById(collectionKey, idOrSub);
  return found
    ? HttpResponse.json(shapeRecord(collectionKey, found))
    : HttpResponse.json({ code: "NOT_FOUND", message: "Resource not found" }, { status: 404 });
}

async function crudCreate(key: string, request: Request, fallbackKey?: string): Promise<Response> {
  // Sub-action POSTs (enable/disable/revoke/default/...) just succeed.
  const lastSeg = key.split("/").pop() ?? "";
  if (NON_CRUD_POST_SEGMENTS.has(lastSeg)) {
    return new HttpResponse(null, { status: 204 });
  }
  const targetKey = inCrudArea(key) ? key : fallbackKey && inCrudArea(fallbackKey) ? fallbackKey : null;
  if (!targetKey) return new HttpResponse(null, { status: 204 });
  const body = await readBody(request);
  return HttpResponse.json(shapeRecord(targetKey, insert(targetKey, normalizeCreate(targetKey, body))), { status: 201 });
}

async function crudUpdate(key: string, id: string | undefined, request: Request): Promise<Response> {
  if (!id || NON_CRUD_POST_SEGMENTS.has(id)) return new HttpResponse(null, { status: 204 });
  if (!inCrudArea(key)) return new HttpResponse(null, { status: 204 });
  const body = await readBody(request);
  return HttpResponse.json(shapeRecord(key, update(key, id, key === "monitoring/profile-assignments" || key === "monitoring/profiles" || key === "inventory/contact-assignments" ? normalizeCreate(key, body) : body)));
}

function crudDelete(key: string, id: string | undefined): Response {
  if (key === "monitoring/profiles" && rows("monitoring/profile-assignments").some(assignment => assignment.profile_id === id)) return HttpResponse.json({ message: "Remove profile assignments first" }, { status: 409 });
  if (id && inCrudArea(key)) remove(key, id);
  return new HttpResponse(null, { status: 204 });
}

/** POST tail segments that are actions, not resource creates. */
const NON_CRUD_POST_SEGMENTS = new Set([
  "enable",
  "disable",
  "revoke",
  "default",
  "test-connection",
  "test-fire",
  "secret",
  "password",
  "disconnect",
  "terminate",
  "preview-run",
  "invalidate",
]);

/**
 * Light per-area shaping so a freshly created row carries the fields its list
 * table reads (e.g. access-control rows need snake_case `auth_source`). Most
 * areas need nothing beyond the raw body + generated id.
 */
function normalizeCreate(key: string, body: Record<string, unknown>): Record<string, unknown> {
  if (key === "inventory/contact-assignments" && typeof body.object_type === "string") return {
    ...body, content_type: body.object_type === "inventory.application" ? DEMO_APPLICATION_CONTENT_TYPE : null,
  };
  if (key === "inventory/wireless_lans" || key === "inventory/wireless_links") {
    return { status: key === "inventory/wireless_lans" ? "active" : "connected", ...body };
  }
  if (key === "monitoring/profiles") {
    return { description: null, metric_set_id: null, query_source_id: null, alert_source_id: null, enabled: true, ...body };
  }
  if (key === "monitoring/profile-assignments") {
    return { enabled: true, weight: 0, target: null, scope_id: null, ...body };
  }
  if (key === "admin/access-control/users") {
    return {
      username: body.username ?? "new-user",
      email: body.email ?? "",
      status: body.status ?? "active",
      auth_source: body.auth_source ?? "local",
      external_id: body.external_id ?? null,
    };
  }
  if (key === "connection-config/records") {
    // `ConnConfRecordResponse.description` is a plain `string`: the column is
    // NOT NULL DEFAULT '' and the insert binds `unwrap_or_default()`, so the
    // real API cannot answer null. The UI sends null for an empty description,
    // and echoing that back made `draftFromRecord` produce a draft whose
    // `description.trim()` threw before the update mutation ever ran - Save
    // Changes issued no PUT and reported nothing.
    return {
      ...body,
      enabled: body.enabled ?? true,
      description: typeof body.description === "string" ? body.description : "",
    };
  }
  if (key === "connection-config/assignments") {
    return { ...body, enabled: body.enabled ?? true };
  }
  if (key === "admin/access-control/role-assignments") {
    return {
      role_id: body.role_id ?? "",
      user_id: body.user_id ?? null,
      group_id: body.group_id ?? null,
    };
  }
  return body;
}

// ─────────────────────────────────────────────────────────────────────────────
// Stateful catch-all. Any GET whose path maps to a registered store area returns
// that area's live (mutated) rows; everything else returns a valid empty
// envelope. Writes always succeed so no screen breaks.
// ─────────────────────────────────────────────────────────────────────────────
function catchAllGet({ request }: { request: Request }): Response {
  const url = new URL(request.url);
  const key = url.pathname.replace(/^.*\/api\/v1\//, "");
  if (key && inCrudArea(key)) return listResponse(key, url);
  return HttpResponse.json({ items: [], total: 0, limit: 1000, offset: 0 });
}

function catchAllWrite(): Response {
  return new HttpResponse(null, { status: 204 });
}
