import { describe, expect, it } from "vitest";
import type { ConnectionTarget } from "@/shared/api/types";
import { DEFAULT_TREE_CONFIG } from "./treeConfig";
import { resolveTreeGroup } from "./treeGroups";

const config = { ...DEFAULT_TREE_CONFIG, mode: "custom" as const, order: ["region", "site"] as const };
const treeConfig = { ...config, order: [...config.order] };
const host = (id: string, region: string, site: string, siteId: string): ConnectionTarget => ({
  id: id as ConnectionTarget["id"], name: id, targetKind: "device", region, site,
  siteId: siteId as ConnectionTarget["siteId"],
});

describe("resolveTreeGroup", () => {
  it("restores a renamed unique group after its original anchor disappears", () => {
    const result = resolveTreeGroup([host("remaining", "North", "Renamed", "site-1")], treeConfig, "site", "region=North|site=Old|", "removed", "site-1");
    expect(result?.name).toBe("Renamed");
    expect(result?.hosts?.map((item) => item.id)).toEqual(["remaining"]);
  });

  it("uses a surviving anchor for ambiguous scoped groups and otherwise refuses to choose", () => {
    const hosts = [host("north", "North", "Renamed", "site-1"), host("south", "South", "Renamed", "site-1")];
    expect(resolveTreeGroup(hosts, treeConfig, "site", "region=North|site=Old|", "south", "site-1")?.id).toBe("region=South|site=Renamed|");
    expect(resolveTreeGroup(hosts, treeConfig, "site", "region=North|site=Old|", "removed", "site-1")).toBeNull();
  });

  it("does not infer an identity from a renamed path without a native FK", () => {
    expect(resolveTreeGroup([host("anchor", "North", "Renamed", "site-1")], treeConfig, "site", "region=North|site=Old|", "anchor", null)).toBeNull();
  });
});
