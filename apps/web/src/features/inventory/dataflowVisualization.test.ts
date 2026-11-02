import { describe, expect, it } from "vitest";
import { buildDataflowVisualization, dataflowNeighborhood, fitDataflowGraph, layoutDataflowEdges, layoutDataflowGraph } from "./dataflowVisualization";
import { dataflowEndpointKey } from "./dataflowEndpoints";
import type { Dataflow } from "./dataflows-api";

const a: Dataflow["source"] = { object_type: "dcim.device", object_id: "a", label: "Same", address_mode: "selected", ipaddresses: ["b", "a"] };
const b: Dataflow["source"] = { ...a, object_id: "b" };
const flow: Dataflow = { id: "first", name: "First", source: a, destination: b, transport: "tcp", destination_ports: [443], status: "active", allow_cross_vrf: false, description: "" };
describe("dataflow visualization conservation", () => {
  it.each([340, 700, 1129])("keeps dense edge controls reachable within the SVG at %i CSS pixels", width => {
    const flows = Array.from({ length: 40 }, (_, index) => ({ ...flow, id: `flow-${index}`, source: { ...a, object_id: `source-${index % 3}` }, destination: { ...b, object_type: "ipam.service" as const, object_id: `service-${index % 11}` } }));
    const graph = buildDataflowVisualization(flows).exact;
    const layout = layoutDataflowGraph(graph.nodes, width, true);
    const connections = layoutDataflowEdges(graph.edges, layout);
    expect(connections.edges.map(item => item.edge.key)).toEqual(graph.edges.map(edge => edge.key));
    const badges = connections.edges.map(item => item.badge);
    for (const badge of badges) {
      expect(badge.x - 30).toBeGreaterThanOrEqual(0);
      expect(badge.x + 30).toBeLessThanOrEqual(layout.width);
      expect(badge.y - 16).toBeGreaterThanOrEqual(0);
      expect(badge.y + 16).toBeLessThanOrEqual(connections.height);
      for (const node of layout.positions.values()) expect(Math.abs(badge.x - node.x) >= layout.nodeWidth / 2 + 30 || Math.abs(badge.y - node.y) >= layout.nodeHeight / 2 + 16).toBe(true);
      for (const other of badges) if (other !== badge) expect(Math.abs(badge.x - other.x) >= 60 || Math.abs(badge.y - other.y) >= 32).toBe(true);
    }
  });
  it.each([700, 800, 900])("keeps eight grouped node buttons non-overlapping at %i CSS pixels", width => {
    const types: Dataflow["source"]["object_type"][] = ["dcim.device", "ipam.service", "virtualization.virtualmachine", "dcim.site", "ipam.ipaddress", "dcim.devicerole", "ipam.prefix", "internet"];
    const endpoints = types.map((object_type, index) => ({ object_type, object_id: object_type === "internet" ? null : `object-${index}`, label: object_type, address_mode: "default" as const, ipaddresses: [] }));
    const flows = Array.from({ length: 4 }, (_, index) => ({ ...flow, id: `flow-${index}`, source: endpoints[index * 2]!, destination: endpoints[index * 2 + 1]! }));
    const graph = buildDataflowVisualization(flows).overview;
    expect(graph.nodes).toHaveLength(8);
    const layout = layoutDataflowGraph(graph.nodes, width, true);
    const positions = [...layout.positions.values()];
    for (let index = 0; index < positions.length; index++) for (const other of positions.slice(index + 1)) {
      const position = positions[index]!;
      expect(Math.abs(position.x - other.x) >= layout.nodeWidth || Math.abs(position.y - other.y) >= layout.nodeHeight).toBe(true);
    }
  });
  it("fits and centers a tall graph rather than merely restoring 100 percent zoom", () => {
    const layout = { width: 1129, height: 790 };
    const viewport = { width: 1129, height: 520 };
    const fit = fitDataflowGraph(layout, viewport);
    expect(fit.zoom * layout.height).toBeLessThanOrEqual(viewport.height);
    expect(fit.zoom).toBeLessThan(1);
    const top = layout.height / 2 - layout.height * fit.zoom / 2 - fit.top;
    const bottom = layout.height / 2 + layout.height * fit.zoom / 2 - fit.top;
    expect(top).toBeGreaterThanOrEqual(0);
    expect(bottom).toBeLessThanOrEqual(viewport.height);
    expect(fitDataflowGraph({ width: 340, height: 300 }, { width: 340, height: 560 }).zoom).toBe(1);
  });
  it("keeps fixed readable card geometry at desktop and mobile widths without overlap", () => {
    const graph = buildDataflowVisualization(Array.from({ length: 5 }, (_, index) => ({ ...flow, id: String(index), source: { ...a, object_id: String(index) } }))).exact;
    for (const width of [340, 1129]) {
      const layout = layoutDataflowGraph(graph.nodes, width, true);
      expect(layout.width).toBe(width);
      expect(layout.nodeWidth).toBeGreaterThanOrEqual(180);
      expect(layout.nodeHeight).toBeGreaterThanOrEqual(64);
      const positions = [...layout.positions.values()];
      for (const position of positions) {
        expect(position.x - layout.nodeWidth / 2).toBeGreaterThanOrEqual(0);
        expect(position.x + layout.nodeWidth / 2).toBeLessThanOrEqual(width);
        expect(position.y - layout.nodeHeight / 2).toBeGreaterThanOrEqual(0);
        expect(position.y + layout.nodeHeight / 2).toBeLessThanOrEqual(layout.height);
      }
      for (let index = 0; index < positions.length; index++) for (const other of positions.slice(index + 1)) {
        const position = positions[index]!;
        expect(Math.abs(position.x - other.x) >= layout.nodeWidth || Math.abs(position.y - other.y) >= layout.nodeHeight).toBe(true);
      }
    }
  });
  it("pages every directed neighbor without losing parallel, reverse, self or off-page flows", () => {
    const flows = Array.from({ length: 521 }, (_, index) => ({ ...flow, id: String(index), source: { ...a, object_id: `host-${index}` } }));
    const graph = buildDataflowVisualization([...flows, { ...flow, id: "self", source: b }, { ...flow, id: "reverse", source: b, destination: flows[0]!.source }, { ...flows[0]!, id: "parallel" }]).exact;
    const first = dataflowNeighborhood(graph, null, 0);
    expect(first.center?.key).toBe(dataflowEndpointKey(b));
    expect(first.nodes.length).toBeLessThanOrEqual(9);
    expect(first.incident.flatMap(edge => edge.flows)).toHaveLength(524);
    const reached = new Set<string>();
    for (let page = 0; page < first.pages; page++) {
      const nearby = dataflowNeighborhood(graph, first.center!.key, page);
      expect(nearby.nodes.length).toBeLessThanOrEqual(9);
      nearby.edges.flatMap(edge => edge.flows).forEach(item => reached.add(item.id));
    }
    expect(reached.size).toBe(524);
    for (const node of graph.nodes) expect(dataflowNeighborhood(graph, node.key, 0).center?.key).toBe(node.key);
  });
  it.each([0, 1, 521, 1000])("assigns each of %i originals exactly once per visualization without changing input", count => {
    const flows = Array.from({ length: count }, (_, index) => ({ ...flow, id: String(index), source: { ...a, object_id: `host-${index}` }, destination_ports: Array.from({ length: 256 }, (_, port) => 10000 + port) }));
    const before = JSON.stringify(flows);
    for (const graph of Object.values(buildDataflowVisualization(flows))) {
      const originals = graph.edges.flatMap(edge => edge.flows);
      expect(originals).toHaveLength(count);
      expect(new Set(originals.map(item => item.id)).size).toBe(count);
      for (const item of originals) expect(flows).toContain(item);
    }
    expect(JSON.stringify(flows)).toBe(before);
  });
  it("preserves directed, self and parallel buckets independently of duplicate labels", () => {
    const flows: Dataflow[] = [flow, { ...flow, id: "parallel", status: "disabled", source: { ...a, label: "Rename", ipaddresses: ["a", "b", "b"] } }, { ...flow, id: "reverse", source: b, destination: a }, { ...flow, id: "self", destination: a }, { ...flow, id: "other-intent", source: { ...a, address_mode: "all" } }, { ...flow, id: "other-type", source: { ...a, object_type: "virtualization.virtualmachine" } }];
    const model = buildDataflowVisualization(flows);
    expect(model.exact.nodes).toHaveLength(4);
    expect(model.exact.edges).toHaveLength(5);
    const parallel = model.exact.edges.find(edge => edge.source === dataflowEndpointKey(a) && edge.destination === dataflowEndpointKey(b));
    expect(parallel?.flows.map(item => item.id)).toEqual(["first", "parallel"]);
    expect(parallel?.flows.map(item => item.status)).toEqual(["active", "disabled"]);
    expect(model.exact.edges.some(edge => edge.source === edge.destination)).toBe(true);
    expect(model.matrix.nodes).toHaveLength(2);
  });
  it("retains native logical selector identities without guessing physical roles or sites", () => {
    const flows = [flow, { ...flow, id: "region", source: { ...a, object_type: "dcim.region" as const, label: "Region A" } }, { ...flow, id: "role", source: { ...a, object_type: "dcim.devicerole" as const, label: "Database role" } }];
    const model = buildDataflowVisualization(flows);
    expect(model.overview.nodes.filter(node => node.grouped)).toHaveLength(1);
    expect(model.overview.nodes.filter(node => !node.grouped).map(node => node.endpoint.label)).toEqual(["Database role", "Region A"]);
    expect(model.overview.nodes.find(node => node.grouped)?.members).toHaveLength(2);
  });
  it("preserves eleven named service destinations while aggregating hundreds of sites and hosts", () => {
    const flows = Array.from({ length: 521 }, (_, index) => ({ ...flow, id: String(index), source: { ...a, object_type: index < 419 ? "dcim.site" as const : "dcim.device" as const, object_id: `source-${index}` }, destination: { ...b, object_type: "ipam.service" as const, object_id: `service-${index % 11}`, label: `Service ${index % 11}` } }));
    const graph = buildDataflowVisualization(flows).overview;
    expect(graph.nodes).toHaveLength(13);
    expect(graph.nodes.filter(node => node.endpoint.object_type === "ipam.service" && !node.grouped)).toHaveLength(11);
    expect(graph.nodes.find(node => node.endpoint.object_type === "dcim.site")?.members).toHaveLength(419);
    expect(graph.nodes.find(node => node.endpoint.object_type === "dcim.device")?.members).toHaveLength(102);
    expect(graph.edges.flatMap(edge => edge.flows)).toHaveLength(521);
    const layout = layoutDataflowGraph(graph.nodes, 1129, true);
    expect(layout.nodeWidth).toBe(200);
    expect(layout.positions.size).toBe(13);
  });
});
