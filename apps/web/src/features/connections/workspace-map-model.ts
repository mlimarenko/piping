import type { ConnectionTarget, Resource } from "@/shared/api/types";
import { parseLat, parseLng } from "@/shared/forms/coordUtils";

export interface LocatedWorkspaceTarget {
  target: ConnectionTarget;
  site: Resource;
  lat: number;
  lng: number;
}

export interface UnplacedWorkspaceTarget {
  target: ConnectionTarget;
  reason: "no-site" | "site-unavailable" | "missing-coordinates";
}

export interface WorkspaceMapLocation {
  id: string;
  name: string;
  lat: number;
  lng: number;
  sites: Resource[];
  targets: ConnectionTarget[];
}

export function groupWorkspaceLocations(located: readonly LocatedWorkspaceTarget[]): WorkspaceMapLocation[] {
  const bySite = new Map<string, WorkspaceMapLocation>();
  for (const { target, site, lat, lng } of located) {
    const existing = bySite.get(site.id);
    if (existing) existing.targets.push(target);
    else bySite.set(site.id, { id: site.id, name: site.name, lat, lng, sites: [site], targets: [target] });
  }
  const groups: WorkspaceMapLocation[] = [];
  const byCoordinates = new Map<string, WorkspaceMapLocation>();
  // Only genuinely co-located Sites share a marker; distinct geography stays plotted.
  for (const site of [...bySite.values()].sort((a, b) => b.targets.length - a.targets.length || a.id.localeCompare(b.id))) {
    const coordinates = `${site.lat},${site.lng}`;
    const group = byCoordinates.get(coordinates);
    if (group) { group.sites.push(...site.sites); group.targets.push(...site.targets); }
    else { byCoordinates.set(coordinates, site); groups.push(site); }
  }
  return groups;
}

export function projectWorkspaceTargets(
  targets: readonly ConnectionTarget[],
  sites: readonly Resource[],
): { located: LocatedWorkspaceTarget[]; unplaced: UnplacedWorkspaceTarget[] } {
  const sitesById = new Map(sites.filter((site) => site.type === "site").map((site) => [site.id, site]));
  const located: LocatedWorkspaceTarget[] = [];
  const unplaced: UnplacedWorkspaceTarget[] = [];

  for (const target of targets) {
    if (!target.siteId) {
      unplaced.push({ target, reason: "no-site" });
      continue;
    }
    const site = sitesById.get(target.siteId);
    if (!site) {
      unplaced.push({ target, reason: "site-unavailable" });
      continue;
    }
    const lat = parseLat(site.latitude);
    const lng = parseLng(site.longitude);
    if (lat === null || lng === null) {
      unplaced.push({ target, reason: "missing-coordinates" });
      continue;
    }
    located.push({ target, site, lat, lng });
  }

  return { located, unplaced };
}
