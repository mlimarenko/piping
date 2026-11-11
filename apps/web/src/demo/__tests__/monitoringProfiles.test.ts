import { setupServer } from "msw/node";
import { afterAll, beforeAll, expect, it, vi } from "vitest";
import { handlers } from "../handlers";
import type * as Client from "@/shared/api/client";
import { deleteProfile, deleteProfileAssignment, listAllProfiles, type MonitoringProfile } from "../../features/observability/api";

const transport = vi.hoisted(() => ({ GET: vi.fn(), DELETE: vi.fn() }));
vi.mock("@/shared/api/client", async original => ({ ...await original<typeof Client>(), apiClient: transport }));

const server = setupServer(...handlers);
const api = (path: string) => `${window.location.origin}/api/v1/monitoring/${path}`;
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

it("accepts successful empty DELETE responses through the typed profile API client", async () => {
  transport.DELETE.mockResolvedValue({ response: new Response(null, { status: 204 }) });
  try {
    await expect(deleteProfileAssignment("synthetic-assignment")).resolves.toBeUndefined();
    await expect(deleteProfile("synthetic-profile")).resolves.toBeUndefined();
  } finally { transport.DELETE.mockReset(); }
});

it("collects a clamped profile catalog to completion while retaining the enabled filter", async () => {
  const now="2026-01-01T00:00:00Z";
  const profiles:MonitoringProfile[]=Array.from({length:51},(_,index)=>({id:`profile-${index}`,name:`Profile ${index}`,slug:`profile-${index}`,enabled:true,items:[],item_count:0,assignment_count:0,created_at:now,updated_at:now}));
  transport.GET
    .mockResolvedValueOnce({data:{items:profiles.slice(0,50),total:51,limit:50,offset:0},response:new Response(null,{status:200})})
    .mockResolvedValueOnce({data:{items:profiles.slice(50),total:51,limit:50,offset:50},response:new Response(null,{status:200})});
  try {
    expect(await listAllProfiles({enabled:"true"})).toEqual(profiles);
    expect(transport.GET).toHaveBeenNthCalledWith(1,"/api/v1/monitoring/profiles",{params:{query:{enabled:"true",limit:200,offset:0}}});
    expect(transport.GET).toHaveBeenNthCalledWith(2,"/api/v1/monitoring/profiles",{params:{query:{enabled:"true",limit:200,offset:50}}});
  } finally {transport.GET.mockReset();}
});

it("excludes the same profile and other scopes from preview conflicts and identifies the actual conflicting owner", async () => {
  const seeded=await(await fetch(api("profiles/profile-node/preview?scope_type=global"))).json();
  expect(seeded.conflicts).toEqual([]);
  const created=await fetch(api("profiles"),{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({name:"Synthetic conflict owner",slug:"synthetic-conflict-owner",enabled:true,items:[{exporter_id:"exp-node-0000-0000-0000-000000000001",job:"node",enabled:true}]})});
  const profile=await created.json();
  const assigned=await fetch(api("profile-assignments"),{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({profile_id:profile.id,scope_type:"global",enabled:true})});
  const assignment=await assigned.json();
  const preview=await(await fetch(api("profiles/profile-node/preview?scope_type=global"))).json();
  expect(preview.conflicts).toEqual([expect.objectContaining({profile_id:profile.id,profile_slug:"synthetic-conflict-owner",job:"node"})]);
  expect((await fetch(api(`profile-assignments/${assignment.id}`),{method:"DELETE"})).status).toBe(204);
  expect((await fetch(api(`profiles/${profile.id}`),{method:"DELETE"})).status).toBe(204);
});

it("creates three-exporter profiles, previews three targets per matching host and restricts deletion after assignment", async () => {
  const exporters = await (await fetch(api("exporters"))).json();
  const selected = exporters.items.filter((item: { slug: string }) => ["node", "process", "blackbox"].includes(item.slug));
  expect(selected).toHaveLength(3);
  const created = await fetch(api("profiles"), { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ name: "Synthetic triple", slug: "synthetic-triple", enabled: true, items: selected.map((item: { id: string }) => ({ exporter_id: item.id, enabled: true })) }) });
  expect(created.status).toBe(201);
  const profile = await created.json();
  expect(profile.item_count).toBe(3);
  expect(profile.assignment_count).toBe(0);
  const preview = await (await fetch(api(`profiles/${profile.id}/preview?scope_type=global&target=synthetic.example`))).json();
  expect(preview.hosts.length).toBeGreaterThan(0);
  expect(preview.hosts.every((host: { targets: string[] }) => host.targets.length === 3)).toBe(true);
  expect(preview.hosts[0].targets).toEqual(expect.arrayContaining(["synthetic.example:9100", "synthetic.example:9256", "synthetic.example:9115"]));
  const assignmentResponse = await fetch(api("profile-assignments"), { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ profile_id: profile.id, scope_type: "global", enabled: true }) });
  expect(assignmentResponse.status).toBe(201);
  const assignment = await assignmentResponse.json();
  expect(assignment.profile_name).toBe("Synthetic triple");
  const assignments = await (await fetch(api(`profile-assignments?profile_id=${profile.id}`))).json();
  expect(assignments.items.map((item: { id: string }) => item.id)).toEqual([assignment.id]);
  expect((await (await fetch(api(`profiles/${profile.id}`))).json()).assignment_count).toBe(1);
  expect((await fetch(api(`profiles/${profile.id}`), { method: "DELETE" })).status).toBe(409);
  expect((await fetch(api(`profile-assignments/${assignment.id}`), { method: "DELETE" })).status).toBe(204);
  expect((await fetch(api(`profiles/${profile.id}`), { method: "DELETE" })).status).toBe(204);
});
