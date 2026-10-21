import { describe, expect, it } from "vitest";
import type { ConnectionTarget, Resource } from "@/shared/api/types";
import { groupWorkspaceLocations, projectWorkspaceTargets } from "./workspace-map-model";

function target(id: string, siteId?: string): ConnectionTarget {
  return {
    id: id as ConnectionTarget["id"],
    name: id,
    targetKind: "device",
    siteId: siteId as ConnectionTarget["siteId"],
  };
}

function site(id: string, latitude?: string, longitude?: string): Resource {
  return { id, name: id, type: "site", latitude, longitude };
}

describe("projectWorkspaceTargets", () => {
  it("merges only exact-coordinate Sites deterministically without losing distinct geography", () => {
    const sites = [site("a", "0", "0"), site("b", "0", "0"), site("c", "0.1", "0.2"), site("d", "30", "40")];
    const located = projectWorkspaceTargets(sites.map(entry => target(`target-${entry.id}`, entry.id)), sites).located;
    const groups = groupWorkspaceLocations(located);
    expect(groups).toHaveLength(3);
    expect(groups[0]).toMatchObject({ id: "a", lat: 0, lng: 0 });
    expect(groups[0]?.sites.map(entry => entry.id)).toEqual(["a", "b"]);
    expect(groups[0]?.targets.map(entry => entry.id)).toEqual(["target-a", "target-b"]);
    expect(groupWorkspaceLocations([...located].reverse())).toEqual(groups);
    expect(located).toHaveLength(4);
  });

  it("keeps Amsterdam and Frankfurt at their own actual coordinates", () => {
    const sites = [site("ams", "52.37", "4.9"), site("fra", "50.11", "8.68")];
    const groups = groupWorkspaceLocations(projectWorkspaceTargets([target("ams-device", "ams"), target("fra-vm", "fra")], sites).located);
    expect(groups).toHaveLength(2);
    expect(groups.find(group => group.targets.some(entry => entry.id === "fra-vm"))).toMatchObject({ id: "fra", lat: 50.11, lng: 8.68 });
    expect(groups.find(group => group.id === "ams")).toMatchObject({ lat: 52.37, lng: 4.9 });
  });
  it("inherits coordinates by Site ID for both devices and virtual machines", () => {
    const assignedSite = site("hq", " 51.5 ", "-0.12");
    const device = { ...target("device", "hq"), site: "Outdated label" };
    const vm: ConnectionTarget = { ...target("vm", "hq"), targetKind: "virtual_machine" };

    const result = projectWorkspaceTargets([device, vm], [site("other", "10", "20"), assignedSite]);

    expect(result).toEqual({
      located: [device, vm].map((entry) => ({ target: entry, site: assignedSite, lat: 51.5, lng: -0.12 })),
      unplaced: [],
    });
    expect(result.located[0]?.target).toBe(device);
    expect(result.located[0]?.site).toBe(assignedSite);
  });

  it("distinguishes absent assignments from unavailable Sites without matching labels", () => {
    const unassigned = { ...target("unassigned"), site: "hq" };
    const unavailable = target("unavailable", "missing");
    const wrongType = target("wrong-type", "device-resource");

    expect(projectWorkspaceTargets([unassigned, unavailable, wrongType], [
      site("hq", "1", "2"),
      { ...site("device-resource", "1", "2"), type: "device" },
    ])).toEqual({
      located: [],
      unplaced: [
        { target: unassigned, reason: "no-site" },
        { target: unavailable, reason: "site-unavailable" },
        { target: wrongType, reason: "site-unavailable" },
      ],
    });
  });

  it.each([
    [undefined, "10"], ["10", undefined], ["", "10"], ["10", " "],
    ["invalid", "10"], ["10", "invalid"], ["NaN", "10"], ["10", "Infinity"],
    ["91", "10"], ["-91", "10"], ["10", "181"], ["10", "-181"],
  ])("leaves unusable coordinates (%s, %s) unplaced", (latitude, longitude) => {
    const entry = target("entry", "hq");
    expect(projectWorkspaceTargets([entry], [site("hq", latitude, longitude)])).toEqual({
      located: [],
      unplaced: [{ target: entry, reason: "missing-coordinates" }],
    });
  });

  it.each([["0", "0"], ["-90", "-180"], ["90", "180"]])(
    "accepts zero and inclusive WGS-84 bounds (%s, %s)", (latitude, longitude) => {
      const entry = target("entry", "hq");
      const assignedSite = site("hq", latitude, longitude);
      expect(projectWorkspaceTargets([entry], [assignedSite])).toEqual({
        located: [{ target: entry, site: assignedSite, lat: Number(latitude), lng: Number(longitude) }],
        unplaced: [],
      });
    },
  );

  it("projects only supplied filtered targets and preserves order within each partition", () => {
    const entries = [target("excluded", "hq"), target("z", "hq"), target("unassigned"),
      target("a", "hq"), target("missing", "missing")];
    const filtered = entries.filter((entry) => entry.id !== "excluded");
    const result = projectWorkspaceTargets(filtered, [site("unused", "20", "30"), site("hq", "1", "2")]);

    expect(result.located.map(({ target: entry }) => entry.id)).toEqual(["z", "a"]);
    expect(result.unplaced.map(({ target: entry }) => entry.id)).toEqual(["unassigned", "missing"]);
    expect(result.located.length + result.unplaced.length).toBe(filtered.length);
    expect(projectWorkspaceTargets([], [site("hq", "1", "2")])).toEqual({ located: [], unplaced: [] });
  });

  it("does not mutate input arrays or their source objects", () => {
    const targets = Object.freeze([
      Object.freeze(target("located", "hq")),
      Object.freeze(target("unassigned")),
      Object.freeze(target("invalid", "invalid")),
    ]);
    const sites = Object.freeze([
      Object.freeze(site("hq", "1", "2")),
      Object.freeze(site("invalid", "", "2")),
    ]);
    const before = structuredClone({ targets, sites });

    projectWorkspaceTargets(targets, sites);

    expect({ targets, sites }).toEqual(before);
  });
});
