import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { EndpointLabel, EndpointSummary, FlowStatus, FlowStatusLine, FlowStatusSummary, FlowTraffic, endpointDisplayName, endpointSummaryText } from "./DataflowConnection";
import type { Dataflow } from "./dataflows-api";
import { changeAppLanguage } from "@/shared/i18n";

const flow = {
  id: "flow", name: "Application access", description: "", comments: "", status: "active", effective_status: "active",
  blocked_by_group_id: null, transport: "tcp", destination_ports: [443, 8443], source_ports: null, icmp_types: null,
  allow_cross_vrf: false, application_id: null, group_id: null, owner_id: null, tenant_id: null,
  tags: [], inherited_tags: [], effective_tags: [], custom_field_data: {},
  sources: [{ object_type: "dcim.region", object_id: "region", label: "Region A", address_mode: "default", ipaddresses: [] }],
  destinations: [{ object_type: "tenancy.tenant", object_id: "tenant", label: "Tenant B", address_mode: "default", ipaddresses: [] }],
} satisfies Dataflow;

describe("dataflow connection vocabulary", () => {
  it("counts effective statuses and keeps zero-count statuses discoverable", () => {
    const { rerender } = render(<FlowStatusSummary flows={[flow, { ...flow, id: "second" }, { ...flow, id: "draft", status: "active", effective_status: "draft" }]} />);
    const summary = screen.getByLabelText("Status");
    expect(within(summary).getAllByRole("term").map(term => term.textContent)).toEqual(["Active", "Draft", "Disabled", "Mixed statuses"]);
    expect(within(summary).getAllByRole("definition").map(count => count.textContent)).toEqual(["2", "1", "0"]);
    rerender(<FlowStatusSummary flows={[{ ...flow, status: "active", effective_status: "disabled" }]} />);
    expect(within(summary).getAllByRole("definition").map(count => count.textContent)).toEqual(["0", "0", "1"]);
  });

  it("uses the actual edge line patterns in the status legend", () => {
    render(<>
      <FlowStatusSummary flows={[]} />
      <svg aria-label="Graph edge samples">
        <FlowStatusLine status="active" d="M 0 0 H 24" markerEnd="url(#arrow)" strokeWidth={3} />
        <FlowStatusLine status="draft" d="M 0 12 H 24" />
        <FlowStatusLine status="disabled" d="M 0 24 H 24" />
        <FlowStatusLine status="mixed" d="M 0 36 H 24" />
      </svg>
    </>);
    const legend = [...screen.getByLabelText("Status").querySelectorAll("path")];
    const edges = [...screen.getByLabelText("Graph edge samples").querySelectorAll("path")];
    expect(edges.map(edge => edge.getAttribute("stroke-dasharray"))).toEqual([null, "2 4", "7 5", "7 3 2 3"]);
    expect(legend.map(edge => edge.getAttribute("stroke-dasharray"))).toEqual(edges.map(edge => edge.getAttribute("stroke-dasharray")));
    expect(edges[0]).toHaveAttribute("marker-end", "url(#arrow)");
    expect(edges[0]).toHaveAttribute("stroke-width", "3");
  });

  it("bounds a multi-endpoint side to a readable summary with the full list as a title", () => {
    const sources = [...flow.sources, ...flow.sources.map(endpoint => ({ ...endpoint, object_id: `${endpoint.object_id}-2`, label: "Region B" })), ...flow.sources.map(endpoint => ({ ...endpoint, object_id: `${endpoint.object_id}-3`, label: "Region C" }))];
    render(<EndpointSummary endpoints={sources} />);
    expect(screen.getByText("Region A +2")).toHaveAttribute("title", "Region A, Region B, Region C");
  });

  it("keeps unresolved and global endpoints distinct", () => {
    expect(endpointSummaryText([{ object_type: "dcim.device", object_id: "x", address_mode: "default", ipaddresses: [], label: null }], key => key === "dataflows.unresolved" ? "Unresolved object" : key)).toBe("Unresolved object");
    expect(endpointSummaryText([{ object_type: "internet", address_mode: "default", ipaddresses: [] }], key => key === "dataflows.endpointTypes.internet" ? "Internet / any IP" : key)).toBe("Internet / any IP");
  });

  it("qualifies the same tag UUID and label by device or virtual-machine scope", () => {
    const tag = { object_type: "extras.tag" as const, object_id: "tag-1", label: "Core network", address_mode: "default" as const, ipaddresses: [] };
    const t = (key: string) => key === "dataflows.tagScopeDevice" ? "Devices" : key === "dataflows.tagScopeVm" ? "Virtual machines" : key === "dataflows.endpointTypes.extras_tag" ? "Tag" : key;
    expect(endpointDisplayName({ ...tag, tag_object_type: "dcim.device" }, t)).toBe("Core network · Devices");
    expect(endpointDisplayName({ ...tag, tag_object_type: "virtualization.virtualmachine" }, t)).toBe("Core network · Virtual machines");
    const { rerender } = render(<EndpointLabel endpoint={{ ...tag, tag_object_type: "dcim.device" }} side="Source" />);
    expect(screen.getByText("Source · Tag · Devices")).toBeInTheDocument();
    rerender(<EndpointLabel endpoint={{ ...tag, tag_object_type: "virtualization.virtualmachine" }} side="Source" />);
    expect(screen.getByText("Source · Tag · Virtual machines")).toBeInTheDocument();
  });

  it("renders effective status instead of the stored status", () => {
    render(<FlowStatus flow={{ ...flow, status: "active", effective_status: "disabled" }} />);
    expect(screen.getByText("Disabled")).toBeInTheDocument();
    expect(screen.queryByText("Active")).not.toBeInTheDocument();
  });

  it("renders transport, destination ports, source ports and ICMP types explicitly", async () => {
    const { rerender } = render(<FlowTraffic flow={{ ...flow, source_ports: [1024, 2048] }} />);
    expect(screen.getByText("TCP")).toBeInTheDocument();
    expect(screen.getByText("443, 8443")).toBeInTheDocument();
    expect(screen.getByText("Source ports: 1024, 2048")).toBeInTheDocument();
    rerender(<FlowTraffic flow={{ ...flow, transport: "tcp_udp", destination_ports: null, source_ports: null }} />);
    expect(screen.getByText("TCP + UDP")).toBeInTheDocument();
    expect(screen.getByText("Any ports")).toBeInTheDocument();
    rerender(<FlowTraffic flow={{ ...flow, transport: "icmp", destination_ports: null, icmp_types: [0, 8] }} />);
    expect(screen.getByText("ICMP")).toBeInTheDocument();
    expect(screen.getByText("ICMP types: 0, 8")).toBeInTheDocument();
    expect(screen.queryByText("Any ports")).not.toBeInTheDocument();
    await changeAppLanguage("ru");
    try {
      render(<FlowTraffic flow={{ ...flow, transport: "icmpv6", icmp_types: [128] }} />);
      expect(screen.getByText("ICMPv6")).toBeInTheDocument();
    } finally {
      await changeAppLanguage("en");
    }
  });
});
