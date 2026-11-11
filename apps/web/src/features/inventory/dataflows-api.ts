import { apiClient, unwrap } from "@/shared/api/client";
import type { components, paths } from "@/shared/api/schema.gen";
import { collectAllPages } from "@/shared/api/pagination";

export type Dataflow = components["schemas"]["DataflowResponse"];
export type DataflowCreate = components["schemas"]["DataflowCreatePayload"];
export type DataflowUpdate = components["schemas"]["DataflowUpdatePayload"];
export type DataflowPolicy = components["schemas"]["DataflowPolicyResponse"];
export type DataflowEndpoint = components["schemas"]["DataflowEndpointInput"];
export type DataflowEndpointResponse = components["schemas"]["DataflowEndpointResponse"];
export type DataflowEndpointResolution = components["schemas"]["DataflowEndpointResolutionResponse"];
export type DataflowStatus = components["schemas"]["DataflowStatus"];
export type DataflowTransport = components["schemas"]["DataflowTransport"];
export type DataflowAddressMode = components["schemas"]["DataflowAddressMode"];
export type DataflowBulkAction = paths["/api/v1/inventory/dataflows/bulk-action"] extends {
  post: { requestBody: { content: { "application/json": infer Body } } };
} ? Body : never;
export type DataflowBulkResult = paths["/api/v1/inventory/dataflows/bulk-action"] extends {
  post: { responses: { 200: { content: { "application/json": infer Body } } } };
} ? Body : never;
export type DataflowPolicyHost = {
  object_type: Extract<DataflowEndpoint["object_type"], "dcim.device" | "virtualization.virtualmachine">;
  object_id: string;
};

export function dataflowEndpoints(flow: Pick<Dataflow, "sources" | "destinations">): DataflowEndpointResponse[] {
  return [...flow.sources, ...flow.destinations];
}

export async function resolveDataflowEndpoint(body: DataflowEndpoint): Promise<DataflowEndpointResolution> {
  return unwrap(await apiClient.POST("/api/v1/inventory/dataflows/endpoint-resolution", { body }));
}


export async function listDataflows(): Promise<Dataflow[]> {
  return collectAllPages<Dataflow>(async (limit, offset) =>
    unwrap(await apiClient.GET("/api/v1/inventory/dataflows", { params: { query: { limit, offset } } })), 100);
}

export async function getDataflow(id: string): Promise<Dataflow> {
  return unwrap(await apiClient.GET("/api/v1/inventory/dataflows/{id}", { params: { path: { id } } }));
}

export async function createDataflow(body: DataflowCreate): Promise<Dataflow> {
  return unwrap(await apiClient.POST("/api/v1/inventory/dataflows", { body }));
}

export async function updateDataflow(id: string, body: DataflowUpdate): Promise<Dataflow> {
  return unwrap(await apiClient.PUT("/api/v1/inventory/dataflows/{id}", { params: { path: { id } }, body }));
}

export async function deleteDataflow(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/inventory/dataflows/{id}", { params: { path: { id } } });
}

export async function bulkDataflows(body: DataflowBulkAction): Promise<DataflowBulkResult> {
  return unwrap(await apiClient.POST("/api/v1/inventory/dataflows/bulk-action", { body }));
}

export async function getDataflowPolicy(host: DataflowPolicyHost): Promise<DataflowPolicy> {
  return unwrap(await apiClient.GET("/api/v1/inventory/dataflows/firewall-policy", {
    params: { query: { host_type: host.object_type, host_id: host.object_id } },
  }));
}
