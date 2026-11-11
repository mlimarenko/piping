import type { Dataflow } from "./dataflows-api";
import { dataflowEndpointKey } from "./dataflowEndpoints";

export const FLOW_STATUSES = ["active", "draft", "disabled"] as const;

export function aggregateFlowStatuses(flows: readonly Dataflow[]) {
  const counts = { active: 0, draft: 0, disabled: 0 };
  for (const flow of flows) counts[flow.effective_status] += 1;
  const present = FLOW_STATUSES.filter(status => counts[status] > 0);
  return { counts, status: present.length === 1 ? present[0]! : "mixed" as const };
}

export type DataflowView = "list" | "overview" | "topology" | "matrix";
export type FlowNode = { key: string; endpoint: Dataflow["sources"][number]; grouped: boolean; members: string[] };
export type FlowEdge = { key: string; source: string; destination: string; flows: Dataflow[] };
export type FlowGraph = { nodes: FlowNode[]; edges: FlowEdge[]; flows: Dataflow[] };

export type OverviewRelationship = {
  key: string;
  sourceType: Dataflow["sources"][number]["object_type"];
  destinationType: Dataflow["sources"][number]["object_type"];
  flows: Dataflow[];
};
export type OverviewObject = { node: FlowNode; incoming: number; outgoing: number; total: number };
export type DataflowOverviewModel = {
  relationships: OverviewRelationship[];
  objects: OverviewObject[];
  totalFlows: number;
  totalObjects: number;
  totalRelationships: number;
};

export type TopologyDirection = "incoming" | "outgoing";
export type TopologyNeighbor = { key: string; direction: TopologyDirection; node: FlowNode; edge: FlowEdge };
export type DataflowRelations = {
  center?: FlowNode;
  incoming: TopologyNeighbor[];
  outgoing: TopologyNeighbor[];
  selfEdges: FlowEdge[];
  incidentEdges: FlowEdge[];
  visibleEdges: FlowEdge[];
  incidentFlows: Dataflow[];
  neighbors: number;
  pages: number;
  page: number;
};

export type TopologyPlacement = { key: string; x: number; y: number; width: number; height: number };
export type TopologyLaneItem = TopologyPlacement & { neighbor: TopologyNeighbor; path: string };
export type TopologyLayout = {
  mode: "lanes" | "stack";
  width: number;
  height: number;
  nodeWidth: number;
  nodeHeight: number;
  center: TopologyPlacement | null;
  incoming: TopologyLaneItem[];
  outgoing: TopologyLaneItem[];
  self: TopologyPlacement | null;
  headers: { incoming: { x: number; y: number }; outgoing: { x: number; y: number } };
};

const NODE_WIDTH = 220;
const LANE_MIN_WIDTH = NODE_WIDTH * 3 + 128 + 32;
const LANE_HEADER_HEIGHT = 56;
const ROW_GAP = 16;
const FIT_FLOOR = 0.85;

function flowEndpointPairs(flow: Dataflow) {
  return flow.sources.flatMap(source => flow.destinations.map(destination => ({ source, destination })));
}

function flowDegrees(graph: FlowGraph) {
  const degrees = new Map<string, number>();
  for (const edge of graph.edges) for (const key of new Set([edge.source, edge.destination])) {
    degrees.set(key, (degrees.get(key) ?? 0) + edge.flows.length);
  }
  return degrees;
}

/** Fit scales the whole layout down to the viewport, but never below a readable floor. */
export function fitDataflowGraph(layout: { width: number; height: number; mode?: "lanes" | "stack" }, viewport: { width: number; height: number }) {
  if (layout.mode === "stack" || layout.width <= 0 || layout.height <= 0 || viewport.width <= 0 || viewport.height <= 0) {
    return { zoom: 1, left: 0, top: 0 };
  }
  const zoom = Math.max(FIT_FLOOR, Math.min(1, viewport.width / layout.width, viewport.height / layout.height));
  return {
    zoom,
    left: Math.max(0, (layout.width * zoom - viewport.width) / 2),
    top: Math.max(0, (layout.height * zoom - viewport.height) / 2),
  };
}

function connector(from: { x: number; y: number }, to: { x: number; y: number }) {
  const middle = (from.x + to.x) / 2;
  return `M ${from.x} ${from.y} C ${middle} ${from.y}, ${middle} ${to.y}, ${to.x} ${to.y}`;
}

/** Dense topology places the selected object between separate incoming and outgoing lanes. */
export function layoutDataflowTopology(relations: DataflowRelations, availableWidth: number, measuredHeight = 72): TopologyLayout {
  const nodeHeight = Math.max(72, measuredHeight);
  const width = Math.max(320, availableWidth);
  const stack = width < LANE_MIN_WIDTH;
  if (stack) {
    return {
      mode: "stack",
      width,
      height: 0,
      nodeWidth: Math.min(240, width - 24),
      nodeHeight,
      center: null,
      incoming: [],
      outgoing: [],
      self: null,
      headers: { incoming: { x: 0, y: 0 }, outgoing: { x: 0, y: 0 } },
    };
  }
  const canvasWidth = Math.max(width, LANE_MIN_WIDTH);
  const padding = 16;
  const laneGap = 64;
  const rowStep = nodeHeight + ROW_GAP;
  const rows = Math.max(1, relations.incoming.length, relations.outgoing.length);
  const centerX = canvasWidth / 2;
  const centerY = LANE_HEADER_HEIGHT + ((rows - 1) * rowStep) / 2 + nodeHeight / 2;
  const leftX = padding + NODE_WIDTH / 2;
  const rightX = canvasWidth - padding - NODE_WIDTH / 2;
  const incoming = relations.incoming.map((neighbor, index) => {
    const y = LANE_HEADER_HEIGHT + index * rowStep + nodeHeight / 2;
    return {
      key: `${neighbor.direction}:${neighbor.edge.key}`,
      neighbor,
      x: leftX,
      y,
      width: NODE_WIDTH,
      height: nodeHeight,
      path: connector({ x: leftX + NODE_WIDTH / 2, y }, { x: centerX - NODE_WIDTH / 2, y: centerY }),
    };
  });
  const outgoing = relations.outgoing.map((neighbor, index) => {
    const y = LANE_HEADER_HEIGHT + index * rowStep + nodeHeight / 2;
    return {
      key: `${neighbor.direction}:${neighbor.edge.key}`,
      neighbor,
      x: rightX,
      y,
      width: NODE_WIDTH,
      height: nodeHeight,
      path: connector({ x: centerX + NODE_WIDTH / 2, y: centerY }, { x: rightX - NODE_WIDTH / 2, y }),
    };
  });
  const lanesBottom = LANE_HEADER_HEIGHT + (rows - 1) * rowStep + nodeHeight + 16;
  const self = relations.selfEdges.length
    ? { key: "self", x: centerX, y: centerY + nodeHeight / 2 + 44, width: 200, height: 40 }
    : null;
  const height = Math.max(220, lanesBottom, self ? self.y + self.height / 2 + 16 : 0) + padding;
  return {
    mode: "lanes",
    width: canvasWidth,
    height,
    nodeWidth: NODE_WIDTH,
    nodeHeight,
    center: { key: relations.center?.key ?? "center", x: centerX, y: centerY, width: NODE_WIDTH, height: nodeHeight },
    incoming,
    outgoing,
    self,
    headers: {
      incoming: { x: leftX, y: LANE_HEADER_HEIGHT / 2 },
      outgoing: { x: rightX, y: LANE_HEADER_HEIGHT / 2 },
    },
  };
}

/** Groups displayed relationships by endpoint type; display grouping never claims policy membership. */
export function buildDataflowOverview(flows: Dataflow[]): DataflowOverviewModel {
  const graph = buildDataflowVisualization(flows).exact;
  const relationships = new Map<string, OverviewRelationship>();
  const incoming = new Map<string, Set<string>>();
  const outgoing = new Map<string, Set<string>>();
  const incident = new Map<string, Set<string>>();
  const add = (map: Map<string, Set<string>>, key: string, id: string) => {
    const ids = map.get(key) ?? new Set<string>();
    ids.add(id);
    map.set(key, ids);
  };
  for (const flow of flows) {
    for (const { source, destination } of flowEndpointPairs(flow)) {
      const key = JSON.stringify([source.object_type, destination.object_type]);
      const relationship = relationships.get(key) ?? { key, sourceType: source.object_type, destinationType: destination.object_type, flows: [] };
      if (!relationship.flows.some(item => item.id === flow.id)) relationship.flows.push(flow);
      relationships.set(key, relationship);
      add(outgoing, dataflowEndpointKey(source), flow.id);
      add(incoming, dataflowEndpointKey(destination), flow.id);
    }
    for (const endpoint of [...flow.sources, ...flow.destinations]) add(incident, dataflowEndpointKey(endpoint), flow.id);
  }
  const label = (node: FlowNode) => node.endpoint.label ?? node.endpoint.object_type;
  return {
    relationships: [...relationships.values()].sort((left, right) => right.flows.length - left.flows.length || left.key.localeCompare(right.key)),
    objects: graph.nodes.map(node => ({
      node,
      incoming: incoming.get(node.key)?.size ?? 0,
      outgoing: outgoing.get(node.key)?.size ?? 0,
      total: incident.get(node.key)?.size ?? 0,
    })).sort((left, right) => right.total - left.total || label(left.node).localeCompare(label(right.node)) || left.node.key.localeCompare(right.node.key)),
    totalFlows: flows.length,
    totalObjects: graph.nodes.length,
    totalRelationships: relationships.size,
  };
}

/** Pages unique neighbors; both directions of a reciprocal object stay distinct and self links never page away. */
export function dataflowRelations(graph: FlowGraph, requested: string | null, page: number, pageSize = 5): DataflowRelations {
  const degrees = flowDegrees(graph);
  const center = graph.nodes.find(node => node.key === requested) ?? [...graph.nodes].sort((a, b) => (degrees.get(b.key) ?? 0) - (degrees.get(a.key) ?? 0) || a.key.localeCompare(b.key))[0];
  const incidentEdges = center ? graph.edges.filter(edge => edge.source === center.key || edge.destination === center.key) : [];
  const selfEdges = center ? incidentEdges.filter(edge => edge.source === center.key && edge.destination === center.key) : [];
  const neighborKeys = [...new Set(incidentEdges.flatMap(edge => [edge.source, edge.destination]))].filter(key => key !== center?.key);
  const byKey = new Map(graph.nodes.map(node => [node.key, node]));
  const pages = Math.max(1, Math.ceil(neighborKeys.length / pageSize));
  const current = Math.min(Math.max(0, page), pages - 1);
  const visible = new Set(neighborKeys.slice(current * pageSize, (current + 1) * pageSize));
  const incoming: TopologyNeighbor[] = [];
  const outgoing: TopologyNeighbor[] = [];
  for (const edge of incidentEdges) {
    if (edge.source === edge.destination) continue;
    if (edge.destination === center?.key && visible.has(edge.source)) {
      const node = byKey.get(edge.source);
      if (node) incoming.push({ key: `${edge.key}:incoming`, direction: "incoming", node, edge });
    }
    if (edge.source === center?.key && visible.has(edge.destination)) {
      const node = byKey.get(edge.destination);
      if (node) outgoing.push({ key: `${edge.key}:outgoing`, direction: "outgoing", node, edge });
    }
  }
  const visibleEdges = [...incoming.map(item => item.edge), ...outgoing.map(item => item.edge), ...selfEdges];
  const incidentFlows = [...new Map(incidentEdges.flatMap(edge => edge.flows).map(flow => [flow.id, flow])).values()];
  return { center, incoming, outgoing, selfEdges, incidentEdges, visibleEdges, incidentFlows, neighbors: neighborKeys.length, pages, page: current };
}

/** Kept name for Matrix and exact topology; the obsolete grouped graph is not part of the model. */
function bucketGraph(flows: Dataflow[], keyFor: (endpoint: Dataflow["sources"][number]) => string): FlowGraph {
  const nodes = new Map<string, FlowNode>();
  const edges = new Map<string, FlowEdge>();
  const edgeFlows = new Map<string, Set<string>>();
  const register = (endpoint: Dataflow["sources"][number]) => {
    const exact = dataflowEndpointKey(endpoint);
    const key = keyFor(endpoint);
    const node = nodes.get(key) ?? { key, endpoint, grouped: key !== exact, members: [] };
    if (!node.members.includes(exact)) node.members.push(exact);
    nodes.set(key, node);
    return key;
  };
  for (const flow of flows) {
    for (const { source: sourceEndpoint, destination: destinationEndpoint } of flowEndpointPairs(flow)) {
      const source = register(sourceEndpoint);
      const destination = register(destinationEndpoint);
      const key = JSON.stringify([source, destination]);
      const edge: FlowEdge = edges.get(key) ?? { key, source, destination, flows: [] };
      const seen = edgeFlows.get(key) ?? new Set<string>();
      if (!seen.has(flow.id)) {
        edge.flows.push(flow);
        seen.add(flow.id);
      }
      edgeFlows.set(key, seen);
      edges.set(key, edge);
    }
  }
  return { nodes: [...nodes.values()].sort((a, b) => a.key.localeCompare(b.key)), edges: [...edges.values()].sort((a, b) => a.key.localeCompare(b.key)), flows };
}

export function buildDataflowVisualization(flows: Dataflow[]) {
  const exact = bucketGraph(flows, dataflowEndpointKey);
  const matrix = bucketGraph(flows, endpoint => `type:${endpoint.object_type}`);
  return { exact, matrix };
}
