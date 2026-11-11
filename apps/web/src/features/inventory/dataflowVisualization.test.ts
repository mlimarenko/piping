import { describe, expect, it } from "vitest";
import { buildDataflowOverview, buildDataflowVisualization, dataflowRelations, fitDataflowGraph, layoutDataflowTopology } from "./dataflowVisualization";
import { dataflowEndpointKey } from "./dataflowEndpoints";
import type { Dataflow } from "./dataflows-api";

const a: Dataflow["sources"][number] = { object_type: "dcim.device", object_id: "a", label: "Same", address_mode: "selected", ipaddresses: ["b", "a"] };
const b: Dataflow["sources"][number] = { ...a, object_id: "b" };
const flow: Dataflow = {
  id: "first", name: "First", description: "", status: "active", effective_status: "active", blocked_by_group_id: null,
  transport: "tcp", destination_ports: [443], source_ports: null, icmp_types: null, allow_cross_vrf: false,
  sources: [a], destinations: [b], application_id: null, group_id: null, comments: "", owner_id: null, tenant_id: null,
  tags: [], inherited_tags: [], effective_tags: [], custom_field_data: {},
};
const withFlow = (change: Partial<Dataflow>): Dataflow => ({ ...flow, ...change });

describe("dataflow overview aggregation", () => {
  it("groups directed relationships by endpoint type and conserves every original flow", () => {
    const service = { ...b, object_type: "ipam.service" as const, label: "API" };
    const site = { ...a, object_type: "dcim.site" as const, label: "Amsterdam" };
    const flows = [
      flow,
      withFlow({ id: "second", sources: [site], destinations: [service] }),
      withFlow({ id: "third", sources: [{ ...a, object_id: "c" }], destinations: [service] }),
      withFlow({ id: "reverse", sources: [service], destinations: [a] }),
      withFlow({ id: "self", destinations: [a] }),
    ];
    const model = buildDataflowOverview(flows);
    expect(model.totalFlows).toBe(5);
    expect(model.totalRelationships).toBe(4);
    expect(model.relationships.map(item => item.flows.length)).toEqual([2, 1, 1, 1]);
    const devicesToServices = model.relationships.find(item => item.sourceType === "dcim.device" && item.destinationType === "ipam.service");
    expect(devicesToServices?.flows.map(item => item.id)).toEqual(["third"]);
    const devicesToDevices = model.relationships.find(item => item.sourceType === "dcim.device" && item.destinationType === "dcim.device");
    expect(devicesToDevices?.flows.map(item => item.id)).toEqual(["first", "self"]);
    expect(model.relationships.flatMap(item => item.flows)).toHaveLength(flows.length);
    expect(new Set(model.relationships.flatMap(item => item.flows)).size).toBe(flows.length);
  });

  it("counts a multi-endpoint flow once per relationship and once per object", () => {
    const model = buildDataflowOverview([withFlow({ sources: [{ ...a, object_id: "a1" }, { ...a, object_id: "a2" }], destinations: [b] })]);
    expect(model.totalFlows).toBe(1);
    expect(model.relationships).toHaveLength(1);
    expect(model.relationships[0]!.flows).toHaveLength(1);
    const totals = new Map(model.objects.map(item => [item.node.endpoint.object_id, item]));
    expect(totals.get("a1")?.outgoing).toBe(1);
    expect(totals.get("a2")?.outgoing).toBe(1);
    expect(totals.get("b")?.incoming).toBe(1);
  });

  it("ranks every connected object with directional counts and a visible denominator", () => {
    const service = { ...b, object_type: "ipam.service" as const, label: "API" };
    const flows = Array.from({ length: 6 }, (_, index) => withFlow({ id: `hub-${index}`, sources: [{ ...a, object_id: "hub", label: "Hub" }], destinations: [service] }));
    const model = buildDataflowOverview([...flows, flow]);
    expect(model.totalObjects).toBe(4);
    const hub = model.objects.find(item => item.node.endpoint.object_id === "hub")!;
    expect(hub.node.endpoint.label).toBe("Hub");
    expect(hub.outgoing).toBe(6);
    expect(hub.incoming).toBe(0);
    expect(hub.total).toBe(6);
    expect(model.objects.map(item => item.total).reduce((sum, value) => sum + value, 0)).toBe(14);
    expect(model.objects.find(item => item.node.key === dataflowEndpointKey(b))?.incoming).toBe(1);
  });
});

describe("selected-object topology relations", () => {
  it("pages every unique neighbor and keeps the selected object's full flow list", () => {
    const neighbors = Array.from({ length: 12 }, (_, index) => ({ ...a, object_id: `host-${index}`, label: `Host ${index}` }));
    const flows = neighbors.map((source, index) => withFlow({ id: `out-${index}`, sources: [b], destinations: [source] }));
    const graph = buildDataflowVisualization(flows).exact;
    const first = dataflowRelations(graph, dataflowEndpointKey(b), 0);
    expect(first.center?.key).toBe(dataflowEndpointKey(b));
    expect(first.incoming).toHaveLength(0);
    expect(first.outgoing).toHaveLength(5);
    expect(first.pages).toBe(3);
    expect(first.incidentFlows).toHaveLength(12);
    const reached = new Set<string>();
    for (let page = 0; page < first.pages; page++) {
      const current = dataflowRelations(graph, dataflowEndpointKey(b), page);
      expect(current.outgoing.length).toBeLessThanOrEqual(5);
      current.outgoing.forEach(item => reached.add(item.node.key));
      expect(current.incidentFlows).toHaveLength(12);
    }
    expect(reached.size).toBe(12);
    for (const node of graph.nodes) expect(dataflowRelations(graph, node.key, 0).center?.key).toBe(node.key);
  });

  it("keeps reciprocal directions distinct and self links independent of paging", () => {
    const flows = [
      flow,
      withFlow({ id: "reverse", sources: [b], destinations: [a] }),
      withFlow({ id: "self", sources: [a], destinations: [a] }),
      withFlow({ id: "parallel", sources: [b], destinations: [a] }),
    ];
    const graph = buildDataflowVisualization(flows).exact;
    const relations = dataflowRelations(graph, dataflowEndpointKey(a), 0);
    expect(relations.incoming.map(item => item.edge.key)).toHaveLength(1);
    expect(relations.incoming[0]!.edge.flows.map(item => item.id)).toEqual(["reverse", "parallel"]);
    expect(relations.outgoing.map(item => item.edge.key)).toHaveLength(1);
    expect(relations.incoming.map(item => item.key)).not.toEqual(relations.outgoing.map(item => item.key));
    expect(relations.selfEdges.flatMap(edge => edge.flows).map(item => item.id)).toEqual(["self"]);
    expect(relations.incidentFlows.map(item => item.id).sort()).toEqual(["first", "parallel", "reverse", "self"]);
  });

  it("pages a hub with 18 neighbours and 44 flows without dropping off-page incident flows", () => {
    const hub = { ...a, object_id: "hub", label: "Hub" };
    const peers = Array.from({ length: 18 }, (_, index) => ({ ...b, object_id: `peer-${index}`, label: `Peer ${index}` }));
    const flows = Array.from({ length: 44 }, (_, index) => {
      const peer = peers[index % peers.length]!;
      if (index % 11 === 0) return withFlow({ id: `self-${index}`, sources: [hub], destinations: [hub] });
      if (index % 3 === 0) return withFlow({ id: `in-${index}`, sources: [peer], destinations: [hub] });
      return withFlow({ id: `out-${index}`, sources: [hub], destinations: [peer] });
    });
    const graph = buildDataflowVisualization(flows).exact;
    const first = dataflowRelations(graph, dataflowEndpointKey(hub), 0);
    expect(first.neighbors).toBe(18);
    expect(first.pages).toBe(4);
    expect(first.incidentFlows).toHaveLength(44);
    expect(first.selfEdges.flatMap(edge => edge.flows)).toHaveLength(4);
    const reached = new Set<string>();
    for (let page = 0; page < first.pages; page++) {
      const current = dataflowRelations(graph, dataflowEndpointKey(hub), page);
      for (const item of [...current.incoming, ...current.outgoing]) reached.add(item.node.key);
      expect(current.incidentFlows).toHaveLength(44);
    }
    expect(reached.size).toBe(18);
  });

  it("defaults to the most connected object deterministically", () => {
    const flows = Array.from({ length: 4 }, (_, index) => withFlow({ id: `hub-${index}`, sources: [{ ...a, object_id: "hub" }], destinations: [{ ...b, object_id: `leaf-${index}` }] }));
    const graph = buildDataflowVisualization([...flows, flow]).exact;
    const relations = dataflowRelations(graph, null, 0);
    expect(relations.center?.endpoint.label).toBe("Same");
    expect(relations.center?.endpoint.object_id).toBe("hub");
  });
});

describe("readable topology geometry", () => {
  const hubs = (incomingCount: number, outgoingCount: number) => {
    const inputs = Array.from({ length: incomingCount }, (_, index) => ({ ...b, object_id: `in-${index}`, label: `Input ${index}` }));
    const outputs = Array.from({ length: outgoingCount }, (_, index) => ({ ...b, object_id: `out-${index}`, label: `Output ${index}` }));
    const flows = [...inputs.map((source, index) => withFlow({ id: `i-${index}`, sources: [source], destinations: [a] })), ...outputs.map((destination, index) => withFlow({ id: `o-${index}`, sources: [a], destinations: [destination] }))];
    return dataflowRelations(buildDataflowVisualization(flows).exact, dataflowEndpointKey(a), 0);
  };
  const reciprocal = (count: number) => {
    const peers = Array.from({ length: count }, (_, index) => ({ ...b, object_id: `peer-${index}`, label: `Peer ${index}` }));
    const flows = peers.flatMap((peer, index) => [
      withFlow({ id: `in-${index}`, sources: [peer], destinations: [a] }),
      withFlow({ id: `out-${index}`, sources: [a], destinations: [peer] }),
    ]);
    return dataflowRelations(buildDataflowVisualization(flows).exact, dataflowEndpointKey(a), 0);
  };

  it.each([1129, 1609])("uses the full available width without the old fixed grid cap at %i", width => {
    const relations = reciprocal(5);
    const layout = layoutDataflowTopology(relations, width, 72);
    expect(layout.mode).toBe("lanes");
    expect(layout.width).toBe(width);
    expect(layout.center?.x).toBe(width / 2);
    const items = [...layout.incoming, ...layout.outgoing];
    expect(items).toHaveLength(10);
    for (const item of items) {
      expect(item.x - item.width / 2).toBeGreaterThanOrEqual(0);
      expect(item.x + item.width / 2).toBeLessThanOrEqual(layout.width);
      expect(item.y - item.height / 2).toBeGreaterThanOrEqual(0);
      expect(item.y + item.height / 2).toBeLessThanOrEqual(layout.height);
      for (const other of items) if (other !== item) {
        expect(Math.abs(item.x - other.x) >= item.width || Math.abs(item.y - other.y) >= item.height).toBe(true);
      }
    }
  });

  it("keeps long measured cards on separate rows and points arrows at the correct card edge", () => {
    const relations = hubs(5, 0);
    const layout = layoutDataflowTopology(relations, 1609, 264);
    expect(layout.nodeHeight).toBe(264);
    expect(layout.incoming).toHaveLength(5);
    const center = layout.center!;
    const first = layout.incoming[0]!;
    expect(first.path.endsWith(`${center.x - center.width / 2} ${center.y}`)).toBe(true);
    for (let index = 0; index < layout.incoming.length; index++) for (const other of layout.incoming.slice(index + 1)) {
      expect(Math.abs(layout.incoming[index]!.y - other.y)).toBeGreaterThanOrEqual(layout.nodeHeight);
    }
  });

  it("separates incoming and outgoing lanes around the selected object", () => {
    const layout = layoutDataflowTopology(hubs(2, 2), 1129, 72);
    expect(layout.incoming.every(item => item.x < layout.center!.x)).toBe(true);
    expect(layout.outgoing.every(item => item.x > layout.center!.x)).toBe(true);
    expect(layout.headers.incoming.x).toBeLessThan(layout.headers.outgoing.x);
    expect(layout.outgoing[0]!.path.endsWith(`${layout.outgoing[0]!.x - layout.nodeWidth / 2} ${layout.outgoing[0]!.y}`)).toBe(true);
  });

  it("keeps the single-row internal-flow control inside the fitted graph bounds", () => {
    const graph = buildDataflowVisualization([withFlow({ id: "self", sources: [a], destinations: [a] })]).exact;
    const relations = dataflowRelations(graph, dataflowEndpointKey(a), 0);
    const layout = layoutDataflowTopology(relations, 1129, 72);
    expect(layout.self).not.toBeNull();
    expect(layout.self!.height).toBeGreaterThanOrEqual(40);
    expect(layout.self!.y + layout.self!.height / 2).toBeLessThanOrEqual(layout.height);
    expect(layout.self!.x - layout.self!.width / 2).toBeGreaterThanOrEqual(0);
    expect(layout.self!.x + layout.self!.width / 2).toBeLessThanOrEqual(layout.width);
    expect(fitDataflowGraph(layout, { width: 1129, height: 560 }).zoom).toBe(1);
  });

  it("switches to a single readable column below the lane threshold", () => {
    const layout = layoutDataflowTopology(hubs(5, 5), 390, 72);
    expect(layout.mode).toBe("stack");
    expect(layout.nodeWidth).toBeLessThanOrEqual(240);
    expect(layout.nodeWidth).toBeGreaterThanOrEqual(200);
    expect(layout.incoming).toHaveLength(0);
    expect(fitDataflowGraph(layout, { width: 390, height: 560 })).toEqual({ zoom: 1, left: 0, top: 0 });
  });

  it("fits normal lanes at 100 percent and never shrinks readable text below the floor", () => {
    const normal = layoutDataflowTopology(hubs(5, 5), 1609, 72);
    expect(fitDataflowGraph(normal, { width: 1609, height: 560 })).toEqual({ zoom: 1, left: 0, top: 0 });
    const tall = { width: 1609, height: 1400, mode: "lanes" as const };
    const fit = fitDataflowGraph(tall, { width: 1609, height: 560 });
    expect(fit.zoom).toBe(0.85);
    expect(fit.top).toBeGreaterThan(0);
    expect(tall.width * fit.zoom).toBeLessThanOrEqual(1609);
  });
});

describe("dataflow visualization conservation", () => {
  it.each([0, 1, 521, 1000])("keeps every one of %i originals in every graph and every edge bucket unique", count => {
    const flows = Array.from({ length: count }, (_, index) => withFlow({
      id: String(index), sources: [{ ...a, object_id: `host-${index}` }],
      destination_ports: Array.from({ length: 256 }, (_, port) => 10000 + port),
    }));
    const before = JSON.stringify(flows);
    const model = buildDataflowVisualization(flows);
    for (const graph of [model.exact, model.matrix]) {
      const originals = graph.edges.flatMap(edge => edge.flows);
      expect(new Set(originals.map(item => item.id)).size).toBe(count);
      for (const edge of graph.edges) expect(new Set(edge.flows.map(item => item.id)).size).toBe(edge.flows.length);
      for (const item of originals) expect(flows).toContain(item);
    }
    expect(JSON.stringify(flows)).toBe(before);
  });

  it("materializes the full source times destination combinations and deduplicates flow buckets", () => {
    const sources = [{ ...a, object_id: "s1" }, { ...a, object_id: "s2" }];
    const destinations = [{ ...b, object_id: "d1" }, { ...b, object_id: "d2" }];
    const graph = buildDataflowVisualization([withFlow({ sources, destinations })]).exact;
    expect(graph.nodes).toHaveLength(4);
    expect(graph.edges).toHaveLength(4);
    expect(graph.edges.every(edge => edge.flows.length === 1)).toBe(true);
    expect(graph.edges.flatMap(edge => edge.flows)).toHaveLength(4);
  });

  it("preserves directed, self, parallel and selector-identity buckets independently of duplicate labels", () => {
    const flows: Dataflow[] = [
      flow,
      withFlow({ id: "parallel", status: "disabled", sources: [{ ...a, label: "Rename", ipaddresses: ["a", "b", "b"] }] }),
      withFlow({ id: "reverse", sources: [b], destinations: [a] }),
      withFlow({ id: "self", destinations: [a] }),
      withFlow({ id: "other-intent", sources: [{ ...a, address_mode: "all" }] }),
      withFlow({ id: "other-type", sources: [{ ...a, object_type: "virtualization.virtualmachine" }] }),
      withFlow({ id: "tag-scope", sources: [{ ...a, object_type: "extras.tag", object_id: "tag", tag_object_type: "dcim.device" }] }),
    ];
    const model = buildDataflowVisualization(flows);
    expect(model.exact.nodes).toHaveLength(5);
    expect(model.exact.edges).toHaveLength(6);
    const parallel = model.exact.edges.find(edge => edge.source === dataflowEndpointKey(a) && edge.destination === dataflowEndpointKey(b));
    expect(parallel?.flows.map(item => item.id)).toEqual(["first", "parallel"]);
    expect(parallel?.flows.map(item => item.status)).toEqual(["active", "disabled"]);
    expect(model.exact.edges.some(edge => edge.source === edge.destination)).toBe(true);
    expect(model.matrix.nodes).toHaveLength(3);
  });
});
