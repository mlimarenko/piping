import type { CustomMapPoint } from "./CustomMap";
import { hasFiniteLngLat } from "./mapLibreObjects";
import { invert, project } from "./mapProjection";

/** Minimum separation of marker centers in screen pixels. */
export const GROUP_DISTANCE = 72;
/** MapLibre renders repeated worlds; the offline SVG renders one world only. */
export type MapWorld = "wrapped" | "unwrapped";

export interface MapGroup {
  point: CustomMapPoint;
  members: CustomMapPoint[];
}

function distance(a: readonly [number, number], b: readonly [number, number], world: MapWorld): number {
  const dx = a[0] - b[0];
  return Math.hypot(world === "wrapped" ? dx - Math.round(dx) : dx, a[1] - b[1]);
}

/** Unwrap the shortest containing longitude arc before averaging its members. */
function center(positions: readonly [number, number][], world: MapWorld): [number, number] {
  let start = 0;
  if (world === "wrapped") {
    const xs = positions.map(([x]) => ((x % 1) + 1) % 1).sort((a, b) => a - b);
    let largestGap = -1;
    for (let index = 0; index < xs.length; index++) {
      const next = index + 1 < xs.length ? xs[index + 1]! : xs[0]! + 1;
      const gap = next - xs[index]!;
      if (gap > largestGap) {
        largestGap = gap;
        start = xs[(index + 1) % xs.length]!;
      }
    }
  }
  let x = 0;
  let y = 0;
  for (const position of positions) {
    let longitude = position[0];
    if (world === "wrapped") {
      longitude = ((longitude % 1) + 1) % 1;
      if (longitude < start) longitude += 1;
    }
    x += longitude;
    y += position[1];
  }
  x /= positions.length;
  return [world === "wrapped" ? x % 1 : x, y / positions.length];
}

/** Connected components in projected screen space; panning does not change groups. */
export function groupMapPoints(points: readonly CustomMapPoint[], scale: number, world: MapWorld): MapGroup[] {
  const valid = points.filter(hasFiniteLngLat);
  const positions = valid.map(point => project(point.lng, point.lat));
  const parent = valid.map((_, index) => index);
  const root = (index: number): number => {
    while (parent[index] !== index) index = parent[index]!;
    return index;
  };
  for (let i = 0; i < valid.length; i++) {
    for (let j = 0; j < i; j++) {
      const a = positions[i]!;
      const b = positions[j]!;
      if (distance(a, b, world) * scale < GROUP_DISTANCE) parent[root(i)] = root(j);
    }
  }
  // A component's centroid can overlap another component even when no pair of
  // its original points does (for example, a ring surrounding a single site).
  // Merge those displayed markers too, until every rendered center is separated.
  let merged = true;
  while (merged) {
    merged = false;
    const centers = new Map<number, [number, number][]>();
    positions.forEach((position, index) => {
      const key = root(index);
      const members = centers.get(key) ?? [];
      members.push(position);
      centers.set(key, members);
    });
    const entries = [...centers.entries()].map(([key, members]) => [key, center(members, world)] as const);
    outer: for (let i = 0; i < entries.length; i++) {
      for (let j = 0; j < i; j++) {
        const [aRoot, a] = entries[i]!;
        const [bRoot, b] = entries[j]!;
        if (distance(a, b, world) * scale < GROUP_DISTANCE) {
          parent[aRoot] = bRoot;
          merged = true;
          break outer;
        }
      }
    }
  }
  const components = new Map<number, number[]>();
  valid.forEach((_, index) => {
    const key = root(index);
    const component = components.get(key) ?? [];
    component.push(index);
    components.set(key, component);
  });
  const usedIds = new Set(valid.map(point => point.id));
  return [...components.values()].map(indices => {
    const members = indices.map(index => valid[index]!);
    if (members.length === 1) return { point: members[0]!, members };
    let id = `cluster:${JSON.stringify(members.map(point => point.id).sort())}`;
    while (usedIds.has(id)) id = `:${id}`;
    const [x, y] = center(indices.map(index => positions[index]!), world);
    const [lng, lat] = invert(x, y);
    return { point: { id, lat, lng, labelCount: members.length, clusterMemberCount: members.length }, members };
  });
}

/** True when zooming can make at least one pair of members visibly separate. */
export function canSeparateGroup(members: readonly CustomMapPoint[], maxScale: number, world: MapWorld): boolean {
  const positions = members.map(point => project(point.lng, point.lat));
  return positions.some(a => positions.some(b => distance(a, b, world) * maxScale >= GROUP_DISTANCE));
}
