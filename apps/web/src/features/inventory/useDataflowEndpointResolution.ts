import { useQuery } from "@tanstack/react-query";
import { dataflowEndpointInput } from "./dataflowEndpoints";
import { resolveDataflowEndpoint } from "./dataflows-api";
import type { DataflowEndpoint } from "./dataflows-api";

export function useDataflowEndpointResolution(value: DataflowEndpoint, enabled = true) {
  const endpoint = dataflowEndpointInput(value);
  return useQuery({ queryKey: ["inventory", "dataflow-resolution", endpoint], queryFn: () => resolveDataflowEndpoint(endpoint),
    enabled: enabled && (value.object_type === "all" || value.object_type === "internet" || !!value.object_id) && (value.address_mode !== "selected" || !!value.ipaddresses?.length), staleTime: 30_000 });
}
