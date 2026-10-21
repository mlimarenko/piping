import { describe, expect, it } from "vitest";
import {
  inventoryEntityParams,
  inventoryResourceEditPath,
  inventoryResourceHref,
  inventoryResourceNewPath,
  isInventoryEntityKey,
  isRailEntity,
  parseInventoryRailEntity,
} from "./resourceLinks";

describe("resourceLinks", () => {
  it("builds a canonical edit href for a navigable entity", () => {
    expect(inventoryResourceHref("Sites", "site-1")).toBe("/inventory/resources/Sites/site-1/edit");
  });

  it("builds a canonical edit href for interface deep-links", () => {
    expect(inventoryResourceHref("Interfaces", "iface-1")).toBe(
      "/inventory/resources/Interfaces/iface-1/edit",
    );
    expect(isInventoryEntityKey("Interfaces")).toBe(true);
  });

  it("treats cables as a navigable inventory entity", () => {
    expect(inventoryResourceHref("Cables", "cable-1")).toBe(
      "/inventory/resources/Cables/cable-1/edit",
    );
    expect(isInventoryEntityKey("Cables")).toBe(true);
  });

  it("returns null when the resource id is missing", () => {
    expect(inventoryResourceHref("Sites", undefined)).toBeNull();
    expect(inventoryResourceHref("Sites", "")).toBeNull();
  });

  it("builds the canonical create and edit paths with an encoded return URL", () => {
    expect(inventoryResourceNewPath("Virtual Machines")).toBe(
      "/inventory/resources/Virtual%20Machines/new",
    );
    expect(inventoryResourceEditPath("Sites", "site-1", "/connections?x=1")).toBe(
      "/inventory/resources/Sites/site-1/edit?return=%2Fconnections%3Fx%3D1",
    );
  });

  it("parses the active rail entity from the list URL", () => {
    expect(parseInventoryRailEntity(new URLSearchParams("entity=Devices"))).toBe("Devices");
    expect(parseInventoryRailEntity(new URLSearchParams(inventoryEntityParams("Tenants")))).toBe(
      "Tenants",
    );
  });

  it("parses pseudo-workspaces as rail entries but never as editable entities", () => {
    for (const rail of ["config-contexts", "ipam-workspace", "cabling-workstation"] as const) {
      expect(isRailEntity(rail)).toBe(true);
      expect(isInventoryEntityKey(rail)).toBe(false);
      expect(parseInventoryRailEntity(new URLSearchParams(inventoryEntityParams(rail)))).toBe(rail);
    }
  });

  it("rejects an invalid or absent entity", () => {
    expect(parseInventoryRailEntity(new URLSearchParams("entity=Nope"))).toBeUndefined();
    expect(parseInventoryRailEntity(new URLSearchParams(""))).toBeUndefined();
  });

  it("entity params carry only the entity", () => {
    const params = inventoryEntityParams("Tenants");
    expect(params.get("entity")).toBe("Tenants");
    expect(params.get("resource")).toBeNull();
    expect(params.get("mode")).toBeNull();
  });
});
