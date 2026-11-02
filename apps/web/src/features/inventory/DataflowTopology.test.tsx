import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import DataflowTopology from "./DataflowTopology";
import DataflowMatrix from "./DataflowMatrix";
import { buildDataflowVisualization } from "./dataflowVisualization";
import type { Dataflow } from "./dataflows-api";

const endpoint = { object_type: "dcim.device", object_id: "host-1", label: "Shared host", address_mode: "selected", ipaddresses: ["ip-b", "ip-a"] } as const;
const flow: Dataflow = { id: "a", name: "First", description: "", status: "active", transport: "tcp", destination_ports: [443], allow_cross_vrf: false, source: { ...endpoint, ipaddresses: [...endpoint.ipaddresses] }, destination: { ...endpoint, object_id: "host-2", label: "Database", ipaddresses: [] } };

describe("selector topology", () => {
  it("keeps every dense overview edge badge clear of node cards and other badges", () => {
    const types: Dataflow["source"]["object_type"][] = ["dcim.device", "dcim.devicerole", "dcim.site", "ipam.ipaddress", "ipam.prefix", "virtualization.virtualmachine"];
    const sources = types.map((object_type, index) => ({ ...flow.source, object_type, object_id: `source-${index}` }));
    const services = Array.from({ length: 11 }, (_, index) => ({ ...flow.destination, object_type: "ipam.service" as const, object_id: `service-${index}`, label: `Service ${index}` }));
    const flows = Array.from({ length: 24 }, (_, index) => ({ ...flow, id: `flow-${index}`, source: sources[index % sources.length]!, destination: services[index % services.length]! }));
    const graph = buildDataflowVisualization(flows).overview;
    expect(graph.nodes).toHaveLength(17);
    const { container } = render(<DataflowTopology flows={flows} graph={graph} grouped onInspect={vi.fn()} />);
    const boxes = [...container.querySelectorAll("svg foreignObject")].map(element => ({
      x: Number(element.getAttribute("x")), y: Number(element.getAttribute("y")),
      width: Number(element.getAttribute("width")), height: Number(element.getAttribute("height")),
    }));
    const badges = boxes.filter(box => box.width === 60);
    expect(badges).toHaveLength(graph.edges.length);
    for (const badge of badges) for (const other of boxes) {
      if (badge === other) continue;
      expect(badge.x + badge.width <= other.x || other.x + other.width <= badge.x || badge.y + badge.height <= other.y || other.y + other.height <= badge.y).toBe(true);
    }
    const svgHeight = Number(screen.getByRole("region", { name: "Scrollable connection topology" }).querySelector("svg")?.getAttribute("height"));
    expect(badges.every(badge => badge.y >= 0 && badge.y + badge.height <= svgHeight)).toBe(true);
  });
  it("summarizes large port selections in the shared drilldown", () => {
    render(<DataflowTopology flows={[{ ...flow, destination_ports: Array.from({ length: 256 }, (_, index) => 10000 + index) }]} onInspect={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Shared host → Database: 1 flows" }));
    expect(within(screen.getByRole("button", { name: "Inspect First" })).getByText("256 ports")).toBeInTheDocument();
  });
  it("shares exact selectors independently of labels and address order, not types or address intent", () => {
    render(<DataflowTopology flows={[flow,
      { ...flow, id: "b", name: "Second", source: { ...flow.source, label: "Renamed", ipaddresses: ["ip-a", "ip-b"] } },
      { ...flow, id: "c", source: { ...flow.source, address_mode: "all", ipaddresses: [] } },
      { ...flow, id: "d", source: { ...flow.source, object_type: "virtualization.virtualmachine" } },
    ]} onInspect={vi.fn()} />);
    expect(screen.getAllByRole("button", { name: /^Highlight connections/ })).toHaveLength(4);
    expect(screen.getByText("4 nodes · 3 directed buckets · 4 flows")).toBeInTheDocument();
  });
  it("keeps parallel, reverse and self flows accessible through edges or node focus", () => {
    const inspect = vi.fn();
    render(<DataflowTopology flows={[flow, { ...flow, id: "b", name: "Parallel", status: "disabled" }, { ...flow, id: "c", name: "Reverse", source: flow.destination, destination: flow.source }, { ...flow, id: "d", name: "Self", destination: flow.source }]} onInspect={inspect} />);
    expect(screen.getByRole("button", { name: "Database → Shared host: 1 flows" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Shared host → Shared host: 1 flows" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Highlight connections for Shared host" }));
    const button = screen.getByRole("button", { name: "Inspect Parallel" });
    expect(within(button).getByText("TCP")).toBeInTheDocument();
    expect(within(button).getByText("443")).toBeInTheDocument();
    expect(within(button).getByText("Disabled")).toBeInTheDocument();
    for (const name of ["First", "Parallel", "Reverse", "Self"]) fireEvent.click(screen.getByRole("button", { name: `Inspect ${name}` }));
    expect(inspect).toHaveBeenCalledTimes(4);
  });
  it("supports keyboard pan, bounded zoom, fit and reset", () => {
    const { container } = render(<DataflowTopology flows={[flow]} onInspect={vi.fn()} />);
    const node = screen.getByRole("button", { name: "Highlight connections for Shared host" });
    fireEvent.click(node);
    expect(node).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "Zoom out" }));
    expect(screen.getAllByRole("status")[0]).toHaveTextContent("75%");
    fireEvent.click(screen.getByRole("button", { name: "Zoom out" }));
    expect(screen.getByRole("button", { name: "Zoom out" })).toBeDisabled();
    const canvas = screen.getByRole("region", { name: "Scrollable connection topology" });
    Object.defineProperties(canvas, {
      clientWidth: { value: 1129, configurable: true },
      clientHeight: { value: 560, configurable: true },
      scrollTo: { value: vi.fn((position: ScrollToOptions) => {
        canvas.scrollTop = position.top ?? 0;
        canvas.scrollLeft = position.left ?? 0;
      }), configurable: true },
    });
    const before = container.querySelector("svg > g")?.getAttribute("transform");
    fireEvent.keyDown(canvas, { key: "ArrowRight" });
    expect(container.querySelector("svg > g")?.getAttribute("transform")).not.toBe(before);
    fireEvent.click(screen.getByRole("button", { name: "Fit graph" }));
    expect(screen.getAllByRole("status")[0]).toHaveTextContent("100%");
    fireEvent.keyDown(canvas, { key: "+" });
    expect(screen.getAllByRole("status")[0]).toHaveTextContent("150%");
    canvas.scrollTop = 333;
    canvas.scrollLeft = 42;
    fireEvent.click(screen.getByRole("button", { name: "Reset topology" }));
    expect(node).toHaveAttribute("aria-pressed", "false");
    expect(canvas.scrollTop).toBe(0);
    expect(canvas.scrollLeft).toBe(0);
    expect(canvas).toHaveAttribute("tabindex", "0");
  });
  it.each([521, 1000])("renders %i exact flows in a bounded 2D canvas and finds the last original flow", count => {
    const flows = Array.from({ length: count }, (_, index) => ({ ...flow, id: `f-${index}`, name: `Flow ${index}`, source: { ...flow.source, object_id: `host-${index}`, label: `Host ${index}` } }));
    render(<DataflowTopology flows={flows} onInspect={vi.fn()} />);
    expect(screen.getAllByRole("button", { name: /^Highlight connections/ })).toHaveLength(9);
    expect(screen.getByRole("combobox", { name: "Explore endpoint" }).children).toHaveLength(count + 1);
    expect(screen.getAllByRole("button", { name: /^Inspect/ })).toHaveLength(20);
    expect(screen.getByText(`Canvas: 9 of ${count + 1} endpoints · 8 of ${count} incident edges · 8 flows`)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "More neighbors" }));
    expect(screen.getByRole("button", { name: "Previous neighbors" })).toBeEnabled();
    const canvas = screen.getByRole("region", { name: "Scrollable connection topology" });
    expect(canvas).toHaveClass("overflow-auto");
    fireEvent.change(screen.getByRole("textbox", { name: "Find endpoint by name or ID…" }), { target: { value: `Host ${count - 1}` } });
    fireEvent.click(screen.getByRole("button", { name: `Host ${count - 1} · host-${count - 1}` }));
    expect(screen.getByRole("button", { name: `Inspect Flow ${count - 1}` })).toBeInTheDocument();
  });
  it("keeps grouped overview and matrix bounded and drills through all 1000 originals", () => {
    const inspect = vi.fn();
    const flows = Array.from({ length: 1000 }, (_, index) => ({ ...flow, id: `f-${index}`, name: `Flow ${index}`, source: { ...flow.source, object_id: `host-${index}` }, destination: { ...flow.destination, object_type: "ipam.service" as const } }));
    const model = buildDataflowVisualization(flows);
    const view = render(<DataflowTopology flows={flows} graph={model.overview} grouped onInspect={inspect} />);
    expect(screen.getAllByRole("button", { name: /^Highlight connections/ })).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Device → Database: 1000 flows" }));
    expect(screen.getAllByRole("button", { name: /^Inspect/ })).toHaveLength(20);
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByRole("button", { name: "Inspect Flow 20" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Search flows, endpoint IDs, ports or status…" }), { target: { value: "host-999" } });
    fireEvent.click(screen.getByRole("button", { name: "Inspect Flow 999" }));
    expect(inspect.mock.calls[0]?.[0].id).toBe("f-999");
    view.unmount();
    render(<DataflowMatrix graph={model.matrix} onInspect={inspect} />);
    expect(screen.getAllByRole("cell")).toHaveLength(4);
    fireEvent.click(screen.getByRole("button", { name: "Device → Service: 1000 flows" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Search flows, endpoint IDs, ports or status…" }), { target: { value: "host-999" } });
    expect(screen.getByRole("button", { name: "Inspect Flow 999" })).toBeInTheDocument();
  });
  it("distinguishes unresolved and wildcard endpoints", () => {
    const flows = [flow, { ...flow, id: "reverse", source: { ...flow.destination, object_type: "internet" as const }, destination: flow.source }, { ...flow, id: "unresolved", source: { ...flow.source, object_type: "dcim.region" as const, label: null } }];
    render(<DataflowTopology flows={flows} onInspect={vi.fn()} />);
    expect(screen.getByText("Unresolved object")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^Highlight connections/ })).toHaveLength(4);
    expect(screen.getByRole("button", { name: "Highlight connections for Internet / any IP" })).toBeInTheDocument();
  });
  it("keeps reverse and self matrix cells distinct and drills down to their own originals", () => {
    const internet = { ...flow.destination, object_type: "internet" as const, object_id: null };
    const flows = [{ ...flow, destination: internet }, { ...flow, id: "reverse", name: "Reverse", source: internet, destination: flow.source }, { ...flow, id: "self", name: "Self", destination: flow.source }];
    render(<DataflowMatrix graph={buildDataflowVisualization(flows).matrix} onInspect={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Device → Internet / any IP: 1 flows" }));
    expect(screen.getByRole("button", { name: "Inspect First" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Inspect Reverse" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Internet / any IP → Device: 1 flows" }));
    expect(screen.getByRole("button", { name: "Inspect Reverse" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Inspect First" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Device → Device: 1 flows" }));
    expect(screen.getByRole("button", { name: "Inspect Self" })).toBeInTheDocument();
  });
});
