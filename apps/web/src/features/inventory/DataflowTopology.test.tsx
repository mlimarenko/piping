import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import DataflowTopology from "./DataflowTopology";
import type { Dataflow } from "./dataflows-api";

const endpoint = { object_type: "dcim.device", object_id: "host-1", label: "Shared host", address_mode: "selected", ipaddresses: ["ip-b", "ip-a"] } as const;
const flow: Dataflow = { id: "a", name: "First", description: "", status: "active", transport: "tcp", destination_ports: [443], allow_cross_vrf: false, source: { ...endpoint, ipaddresses: [...endpoint.ipaddresses] }, destination: { ...endpoint, object_id: "host-2", label: "Database", ipaddresses: [] } };

describe("selector topology", () => {
  it("summarizes large valid port selections instead of overflowing connection cards", () => {
    render(<DataflowTopology flows={[{ ...flow, destination_ports: Array.from({ length: 256 }, (_, index) => 10000 + index) }]} onInspect={vi.fn()} />);
    expect(within(screen.getByRole("button", { name: "Inspect First" })).getByText("256 ports")).toBeInTheDocument();
  });
  it("shares exact selectors independently of labels and address order, but not different types or address intent", () => {
    render(<DataflowTopology flows={[flow,
      { ...flow, id: "b", name: "Second", source: { ...flow.source, label: "Renamed", ipaddresses: ["ip-a", "ip-b"] } },
      { ...flow, id: "c", name: "Other intent", source: { ...flow.source, address_mode: "all", ipaddresses: [] } },
      { ...flow, id: "d", name: "Other type", source: { ...flow.source, object_type: "virtualization.virtualmachine" } },
    ]} onInspect={vi.fn()} />);
    expect(screen.getAllByRole("button", { name: /^Highlight connections/ })).toHaveLength(4);
    expect(screen.getAllByRole("button", { name: /^Inspect/ })).toHaveLength(4);
  });

  it("keeps parallel, reverse and self flows selectable and exposes their protocol, ports and status", () => {
    const inspect = vi.fn();
    render(<DataflowTopology flows={[flow, { ...flow, id: "b", name: "Parallel", status: "disabled" }, { ...flow, id: "c", name: "Reverse", source: flow.destination, destination: flow.source }, { ...flow, id: "d", name: "Self", destination: flow.source }]} onInspect={inspect} />);
    expect(screen.getAllByRole("button", { name: /^Highlight connections/ })).toHaveLength(2);
    const button = screen.getByRole("button", { name: "Inspect Parallel" });
    expect(within(button).getByText("TCP")).toBeInTheDocument();
    expect(within(button).getByText("443")).toBeInTheDocument();
    expect(within(button).getByText("Disabled")).toBeInTheDocument();
    for (const name of ["First", "Parallel", "Reverse", "Self"]) fireEvent.click(screen.getByRole("button", { name: `Inspect ${name}` }));
    expect(inspect).toHaveBeenCalledTimes(4);
  });

  it("highlights an endpoint without resolving membership and supports bounded zoom and reset", () => {
    render(<DataflowTopology flows={[flow]} onInspect={vi.fn()} />);
    const node = screen.getByRole("button", { name: "Highlight connections for Shared host" });
    fireEvent.click(node);
    expect(node).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "Zoom out" }));
    expect(screen.getByRole("status")).toHaveTextContent("75%");
    fireEvent.click(screen.getByRole("button", { name: "Zoom out" }));
    expect(screen.getByRole("button", { name: "Zoom out" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Reset topology" }));
    expect(screen.getByRole("status")).toHaveTextContent("100%");
    expect(node).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByRole("region", { name: "Scrollable connection topology" })).toHaveAttribute("tabindex", "0");
  });

  it("keeps unresolved endpoints and inventory scopes compact without fabricating hosts", () => {
    render(<DataflowTopology flows={[{ ...flow, source: { ...flow.source, object_type: "dcim.region", label: null }, destination: { ...flow.destination, object_type: "internet", object_id: null, label: null } }]} onInspect={vi.fn()} />);
    expect(screen.getByText("Unresolved object")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^Highlight connections/ })).toHaveLength(2);
    expect(screen.getByText("Internet / any IP", { selector: "span[title]" })).toBeInTheDocument();
  });
});
