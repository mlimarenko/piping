import { apiClient, unwrap } from "@/shared/api/client";
import { collectAllPages } from "@/shared/api/pagination";
import type { paths } from "@/shared/api/schema.gen";

type GetBody<Path extends keyof paths> = paths[Path] extends {
  get: { responses: { 200: { content: { "application/json": infer Body } } } };
} ? Body : never;
type PostBody<Path extends keyof paths> = paths[Path] extends {
  post: { requestBody: { content: { "application/json": infer Body } } };
} ? Body : never;
type PostCreated<Path extends keyof paths> = paths[Path] extends {
  post: { responses: { 201: { content: { "application/json": infer Body } } } };
} ? Body : never;
type PostResult<Path extends keyof paths> = paths[Path] extends {
  post: { responses: { 200: { content: { "application/json": infer Body } } } };
} ? Body : never;
type PutBody<Path extends keyof paths> = paths[Path] extends {
  put: { requestBody: { content: { "application/json": infer Body } } };
} ? Body : never;
type PutResult<Path extends keyof paths> = paths[Path] extends {
  put: { responses: { 200: { content: { "application/json": infer Body } } } };
} ? Body : never;
type CollectionItem<Body> = Body extends { items: Array<infer Item> } ? Item : never;

export type ApplicationRole = CollectionItem<GetBody<"/api/v1/inventory/application-roles">>;
export type ApplicationRoleCreate = PostBody<"/api/v1/inventory/application-roles">;
export type ApplicationRoleUpdate = PutBody<"/api/v1/inventory/application-roles/{id}">;

export type Application = CollectionItem<GetBody<"/api/v1/inventory/applications">>;
export type ApplicationCreate = PostBody<"/api/v1/inventory/applications">;
export type ApplicationUpdate = PutBody<"/api/v1/inventory/applications/{id}">;

export type DataflowGroup = CollectionItem<GetBody<"/api/v1/inventory/dataflow-groups">>;
export type DataflowGroupCreate = PostBody<"/api/v1/inventory/dataflow-groups">;
export type DataflowGroupUpdate = PutBody<"/api/v1/inventory/dataflow-groups/{id}">;

export type AddressGroup = CollectionItem<GetBody<"/api/v1/inventory/address-groups">>;
export type AddressGroupCreate = PostBody<"/api/v1/inventory/address-groups">;
export type AddressGroupUpdate = PutBody<"/api/v1/inventory/address-groups/{id}">;

export type DataflowBulkAction = PostBody<"/api/v1/inventory/dataflows/bulk-action">;
export type DataflowBulkResult = PostResult<"/api/v1/inventory/dataflows/bulk-action">;

/** A group and every descendant; used by descendant-inclusive flow filters. */
export function dataflowGroupDescendants(groups: DataflowGroup[], id: string): Set<string> {
  const children = new Map<string, string[]>();
  for (const group of groups) {
    if (!group.parent_id) continue;
    children.set(group.parent_id, [...(children.get(group.parent_id) ?? []), group.id]);
  }
  const result = new Set<string>([id]);
  const queue = [...(children.get(id) ?? [])];
  while (queue.length) {
    const next = queue.shift() as string;
    if (result.has(next)) continue;
    result.add(next);
    queue.push(...(children.get(next) ?? []));
  }
  return result;
}

export type SiteAddressMode = "default" | "selected" | "all" | "oob";

export async function listApplicationRoles(search?: string): Promise<ApplicationRole[]> {
  return collectAllPages<ApplicationRole>(async (limit, offset) =>
    unwrap(await apiClient.GET("/api/v1/inventory/application-roles", { params: { query: { limit, offset, search } } })), 100);
}
export async function createApplicationRole(body: ApplicationRoleCreate): Promise<ApplicationRole> {
  return unwrap(await apiClient.POST("/api/v1/inventory/application-roles", { body }));
}
export async function updateApplicationRole(id: string, body: ApplicationRoleUpdate): Promise<ApplicationRole> {
  return unwrap(await apiClient.PUT("/api/v1/inventory/application-roles/{id}", { params: { path: { id } }, body }));
}
export async function deleteApplicationRole(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/inventory/application-roles/{id}", { params: { path: { id } } });
}

export async function listApplications(search?: string): Promise<Application[]> {
  return collectAllPages<Application>(async (limit, offset) =>
    unwrap(await apiClient.GET("/api/v1/inventory/applications", { params: { query: { limit, offset, search } } })), 100);
}
export async function getApplication(id: string): Promise<Application> {
  return unwrap(await apiClient.GET("/api/v1/inventory/applications/{id}", { params: { path: { id } } }));
}
export async function createApplication(body: ApplicationCreate): Promise<Application> {
  return unwrap(await apiClient.POST("/api/v1/inventory/applications", { body }));
}
export async function updateApplication(id: string, body: ApplicationUpdate): Promise<Application> {
  return unwrap(await apiClient.PUT("/api/v1/inventory/applications/{id}", { params: { path: { id } }, body }));
}
export async function deleteApplication(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/inventory/applications/{id}", { params: { path: { id } } });
}

export async function listDataflowGroups(search?: string): Promise<DataflowGroup[]> {
  return collectAllPages<DataflowGroup>(async (limit, offset) =>
    unwrap(await apiClient.GET("/api/v1/inventory/dataflow-groups", { params: { query: { limit, offset, search } } })), 100);
}
export async function getDataflowGroup(id: string): Promise<DataflowGroup> {
  return unwrap(await apiClient.GET("/api/v1/inventory/dataflow-groups/{id}", { params: { path: { id } } }));
}
export async function createDataflowGroup(body: DataflowGroupCreate): Promise<DataflowGroup> {
  return unwrap(await apiClient.POST("/api/v1/inventory/dataflow-groups", { body }));
}
export async function updateDataflowGroup(id: string, body: DataflowGroupUpdate): Promise<DataflowGroup> {
  return unwrap(await apiClient.PUT("/api/v1/inventory/dataflow-groups/{id}", { params: { path: { id } }, body }));
}
export async function deleteDataflowGroup(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/inventory/dataflow-groups/{id}", { params: { path: { id } } });
}

export async function listAddressGroups(search?: string): Promise<AddressGroup[]> {
  return collectAllPages<AddressGroup>(async (limit, offset) =>
    unwrap(await apiClient.GET("/api/v1/inventory/address-groups", { params: { query: { limit, offset, search } } })), 100);
}
export async function getAddressGroup(id: string): Promise<AddressGroup> {
  return unwrap(await apiClient.GET("/api/v1/inventory/address-groups/{id}", { params: { path: { id } } }));
}
export async function createAddressGroup(body: AddressGroupCreate): Promise<AddressGroup> {
  return unwrap(await apiClient.POST("/api/v1/inventory/address-groups", { body }));
}
export async function updateAddressGroup(id: string, body: AddressGroupUpdate): Promise<AddressGroup> {
  return unwrap(await apiClient.PUT("/api/v1/inventory/address-groups/{id}", { params: { path: { id } }, body }));
}
export async function deleteAddressGroup(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/inventory/address-groups/{id}", { params: { path: { id } } });
}

export async function bulkDataflows(body: DataflowBulkAction): Promise<DataflowBulkResult> {
  return unwrap(await apiClient.POST("/api/v1/inventory/dataflows/bulk-action", { body }));
}

// 201 is the documented create status; the alias documents that the body is
// the created resource without repeating the generated component name.
export type ApplicationCreated = PostCreated<"/api/v1/inventory/applications">;
export type DataflowGroupCreated = PostCreated<"/api/v1/inventory/dataflow-groups">;
export type AddressGroupCreated = PostCreated<"/api/v1/inventory/address-groups">;
export type ApplicationRoleCreated = PostCreated<"/api/v1/inventory/application-roles">;
export type ApplicationPutResult = PutResult<"/api/v1/inventory/applications/{id}">;
