import type { Dataflow } from "./dataflows-api";
import { dataflowEndpointKey } from "./dataflowEndpoints";

export type DataflowView = "list" | "overview" | "topology" | "matrix";
export type FlowNode = { key: string; endpoint: Dataflow["source"]; grouped: boolean; members: string[] };
export type FlowEdge = { key: string; source: string; destination: string; flows: Dataflow[] };
export type FlowGraph = { nodes: FlowNode[]; edges: FlowEdge[]; flows: Dataflow[] };

export function fitDataflowGraph(layout: { width: number; height: number }, viewport: { width: number; height: number }) {
  return {
    zoom: Math.min(1, viewport.width / layout.width, viewport.height / layout.height),
    left: Math.max(0, (layout.width - viewport.width) / 2),
    top: Math.max(0, (layout.height - viewport.height) / 2),
  };
}

export function layoutDataflowEdges(edges: FlowEdge[], layout: ReturnType<typeof layoutDataflowGraph>) {
  const placed: { x: number; y: number }[] = [];
  let overflow = 0;
  const occupied = (badge: { x: number; y: number }) => placed.some(other => Math.abs(other.x - badge.x) < 70 && Math.abs(other.y - badge.y) < 36)
    || [...layout.positions.values()].some(node => Math.abs(node.x - badge.x) < layout.nodeWidth / 2 + 35 && Math.abs(node.y - badge.y) < layout.nodeHeight / 2 + 22);
  const columns = Math.max(1, Math.floor((layout.width - 10) / 70));
  const connections = edges.map(edge => {
    const source = layout.positions.get(edge.source)!, destination = layout.positions.get(edge.destination)!;
    const self = edge.source === edge.destination;
    const dx = destination.x - source.x, dy = destination.y - source.y;
    const clip = Math.max(1, Math.abs(dx) / 108, Math.abs(dy) / 42);
    const start = { x: source.x + dx / clip, y: source.y + dy / clip };
    const end = { x: destination.x - dx / clip, y: destination.y - dy / clip };
    const length = Math.max(1, Math.hypot(dx, dy)), bend = dx === 0 ? 90 : 45;
    const cx = (start.x + end.x) / 2 - dy / length * bend, cy = (start.y + end.y) / 2 + dx / length * bend;
    const path = self ? `M ${source.x - 70} ${source.y - 36} C ${source.x - 100} ${source.y - 120}, ${source.x + 100} ${source.y - 120}, ${source.x + 70} ${source.y - 38}` : `M ${start.x} ${start.y} Q ${cx} ${cy} ${end.x} ${end.y}`;
    const connection = { x: self ? source.x : (start.x + 2 * cx + end.x) / 4, y: self ? source.y - 80 : (start.y + 2 * cy + end.y) / 4 };
    const badge = { ...connection };
    for (let attempt = 0; attempt < 12 && occupied(badge); attempt++) {
      badge.x = Math.max(35, Math.min(layout.width - 35, connection.x + (attempt % 2 ? -1 : 1) * (Math.floor(attempt / 2) + 1) * 70));
      badge.y = Math.max(20, Math.min(layout.height - 20, connection.y + Math.floor(attempt / 4) * 38));
    }
    if (occupied(badge)) {
      // Dedicated overflow rows cannot intersect cards or the bounded local search.
      badge.x = 35 + overflow % columns * 70;
      badge.y = layout.height + 20 + Math.floor(overflow / columns) * 40;
      overflow++;
    }
    placed.push(badge);
    return { edge, path, connection, badge };
  });
  return { edges: connections, height: connections.reduce((height, item) => Math.max(height, item.badge.y + 20), layout.height) };
}

export function dataflowNeighborhood(graph: FlowGraph, requested: string | null, page: number, pageSize = 8) {
  const degrees = new Map<string, number>();
  for (const edge of graph.edges) for (const key of new Set([edge.source, edge.destination])) degrees.set(key, (degrees.get(key) ?? 0) + edge.flows.length);
  const center = graph.nodes.find(node => node.key === requested) ?? [...graph.nodes].sort((a, b) => (degrees.get(b.key) ?? 0) - (degrees.get(a.key) ?? 0) || a.key.localeCompare(b.key))[0];
  const incident = graph.edges.filter(edge => edge.source === center?.key || edge.destination === center?.key);
  const neighborKeys = new Set(incident.flatMap(edge => [edge.source, edge.destination]));
  neighborKeys.delete(center?.key ?? "");
  const neighbors = graph.nodes.filter(node => neighborKeys.has(node.key));
  const pages = Math.max(1, Math.ceil(neighbors.length / pageSize));
  const current = Math.min(Math.max(0, page), pages - 1);
  const nodes = center ? [center, ...neighbors.slice(current * pageSize, (current + 1) * pageSize)] : [];
  const visible = new Set(nodes.map(node => node.key));
  const edges = incident.filter(edge => visible.has(edge.source) && visible.has(edge.destination));
  return { center, nodes, edges, incident, neighbors: neighbors.length, pages, page: current };
}

// Coordinates are CSS pixels, never a square viewBox scaled down to fit.
export function layoutDataflowGraph(nodes: FlowNode[], availableWidth: number, grouped: boolean, center?: string) {
  const nodeWidth = 200, nodeHeight = 72;
  const width = Math.max(320, availableWidth);
  const columns = Math.max(1, Math.floor((width - 40) / 240));
  const others = center ? nodes.filter(node => node.key !== center) : nodes;
  const rows = Math.ceil(others.length / columns);
  const ellipseCandidate = grouped && width >= 700 && nodes.length > 2 && nodes.length <= 8;
  const ellipsePositions = ellipseCandidate ? nodes.map((_, index) => {
    const angle = index * Math.PI * 2 / nodes.length - Math.PI / 2;
    return { x: width / 2 + Math.cos(angle) * (width / 2 - 140), y: 260 + Math.sin(angle) * 150 };
  }) : [];
  const ellipse = ellipseCandidate && ellipsePositions.every((position, index) => ellipsePositions.slice(index + 1).every(other =>
    Math.abs(position.x - other.x) >= nodeWidth || Math.abs(position.y - other.y) >= nodeHeight));
  const height = ellipse ? 520 : Math.max(300, (center ? 250 : 40) + rows * 150);
  const positions = new Map(nodes.map((node, index) => {
    if (ellipse) {
      return [node.key, ellipsePositions[index]!];
    }
    if (node.key === center) return [node.key, { x: width / 2, y: 120 }];
    const slot = others.findIndex(item => item.key === node.key);
    const row = Math.floor(slot / columns);
    const countInRow = Math.min(columns, others.length - row * columns);
    return [node.key, { x: width / 2 + (slot % columns - (countInRow - 1) / 2) * 240, y: (center ? 300 : 120) + row * 150 }];
  }));
  return { width, height, positions, nodeWidth, nodeHeight };
}

function bucketGraph(flows: Dataflow[], keyFor: (endpoint: Dataflow["source"]) => string): FlowGraph {
  const nodes = new Map<string, FlowNode>();
  const edges = new Map<string, FlowEdge>();
  const register = (endpoint: Dataflow["source"]) => {
    const exact = dataflowEndpointKey(endpoint);
    const key = keyFor(endpoint);
    const node = nodes.get(key) ?? { key, endpoint, grouped: key !== exact, members: [] };
    if (!node.members.includes(exact)) node.members.push(exact);
    nodes.set(key, node);
    return key;
  };
  for (const flow of flows) {
    const source = register(flow.source);
    const destination = register(flow.destination);
    const key = JSON.stringify([source, destination]);
    const edge: FlowEdge = edges.get(key) ?? { key, source, destination, flows: [] };
    edge.flows.push(flow);
    edges.set(key, edge);
  }
  return { nodes: [...nodes.values()].sort((a, b) => a.key.localeCompare(b.key)), edges: [...edges.values()].sort((a, b) => a.key.localeCompare(b.key)), flows };
}

// Group only selector facts. Display grouping never infers host membership.
export function buildDataflowVisualization(flows: Dataflow[]) {
  const exact = bucketGraph(flows, dataflowEndpointKey);
  const counts = new Map<string, number>();
  for (const node of exact.nodes) counts.set(node.endpoint.object_type, (counts.get(node.endpoint.object_type) ?? 0) + 1);
  const overview = bucketGraph(flows, endpoint => {
    const type = endpoint.object_type;
    const limit = type === "ipam.service" ? 24 : 6;
    return ["dcim.device", "virtualization.virtualmachine", "ipam.ipaddress", "ipam.prefix"].includes(type) || (counts.get(type) ?? 0) > limit ? `type:${type}` : dataflowEndpointKey(endpoint);
  });
  const matrix = bucketGraph(flows, endpoint => `type:${endpoint.object_type}`);
  return { exact, overview, matrix };
}
