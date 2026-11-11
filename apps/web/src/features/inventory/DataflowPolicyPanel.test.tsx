import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import DataflowPolicyPanel from "./DataflowPolicyPanel";
import en from "@/shared/i18n/locales/en";
import type { DataflowPolicy } from "./dataflows-api";

const getPolicy = vi.hoisted(() => vi.fn());
vi.mock("./dataflows-api", async original => ({ ...(await original()), getDataflowPolicy: getPolicy }));
vi.mock("./DataflowEndpointPicker", () => ({
  default: ({ label, onChange }: { label: string; onChange: (value: unknown) => void }) => <button type="button" onClick={() => onChange({
    object_type: "dcim.device", object_id: "host-1", address_mode: "default", ipaddresses: [],
  })}>{label}</button>,
}));

const policy: DataflowPolicy = {
  host: { object_type: "dcim.device", object_id: "host-1", label: "Web host" },
  apply_ready: true,
  diagnostics: [],
  rules: [
    { direction: "egress", family: "ipv4", transport: "tcp", source: "192.0.2.1/32", destination: "192.0.2.2/32", destination_ports: [443], source_ports: [1024, 2048], icmp_types: null, vrf_id: null, dataflow_ids: ["flow-1"] },
    { direction: "ingress", family: "ipv4", transport: "icmp", source: "192.0.2.2/32", destination: "192.0.2.1/32", destination_ports: null, source_ports: null, icmp_types: [0, 8], vrf_id: null, dataflow_ids: ["flow-2"] },
  ],
};

describe("dataflow host policy rules", () => {
  it("renders source ports and ICMP types with null as unrestricted/absent", async () => {
    getPolicy.mockResolvedValue(policy);
    render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><DataflowPolicyPanel /></QueryClientProvider>);
    fireEvent.click(screen.getByRole("button", { name: en.dataflows.host }));
    fireEvent.click(screen.getByRole("button", { name: en.dataflows.fetchPolicy }));
    await waitFor(() => expect(screen.getByText("1024, 2048")).toBeInTheDocument());
    expect(screen.getByText("0, 8")).toBeInTheDocument();
    expect(screen.getByText("443")).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: en.dataflows.sourcePorts })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: en.dataflows.icmpTypes })).toBeInTheDocument();
  });
});
