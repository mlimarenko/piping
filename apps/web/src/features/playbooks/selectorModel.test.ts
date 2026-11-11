import { describe, expect, it } from "vitest";

import { emptySelectorModel, modelToSelector, selectorToModel } from "./selectorModel";

describe("selectorModel", () => {
  it("round-trips loaded selector semantics, including nonstandard known keys", () => {
    const raw = { target_kind: ["custom"], source_kind: ["device"], role: "Router", sort_direction: "custom", address_bound: null, extension: { nested: true } };
    const model = selectorToModel(raw);
    expect(modelToSelector(model)).toEqual(raw);
    const site = "22222222-2222-4222-8222-222222222222";
    model.facets.site = [site];
    expect(modelToSelector(model)).toEqual({ ...raw, site_ids: [site] });
  });

  it("retains canonical defaults for a new selector", () => {
    expect(modelToSelector(emptySelectorModel())).toMatchObject({ target_kind: ["connection_target"], source_kind: ["device", "virtual_machine"], address_bound: true });
  });

  it("preserves catalog UUID predicates while editing another facet", () => {
    const tagIds = ["11111111-1111-4111-8111-111111111111"];
    const role = "22222222-2222-4222-8222-222222222222";
    const model = selectorToModel({ tag_ids: tagIds });
    model.facets.role = [role];

    expect(modelToSelector(model)).toMatchObject({
      tag_ids: tagIds,
      role_ids: [role],
    });
  });

  it("removes additional operators only after an explicit removal", () => {
    const model = selectorToModel({ role: "named filter", include_target_ids: ["11111111-1111-4111-8111-111111111111"] });
    expect(modelToSelector(model)).toHaveProperty("role", "named filter");
    model.passthrough = {};
    expect(modelToSelector(model)).not.toHaveProperty("role");
    expect(modelToSelector(model)).not.toHaveProperty("include_target_ids");
  });

  it("retains an explicit empty catalog predicate", () => {
    expect(modelToSelector(selectorToModel({ tag_ids: [] }))).toHaveProperty("tag_ids", []);
  });

  it("does not silently drop a retired predicate before backend validation", () => {
    for (const tagSlugs of [[], ["linux"], null]) {
      expect(modelToSelector(selectorToModel({ tag_slugs: tagSlugs })))
        .toHaveProperty("tag_slugs", tagSlugs);
    }
  });

  it("round-trips a dynamic selector without dropping advanced keys or legacy name operators", () => {
    const regionId = "11111111-1111-4111-8111-111111111111";
    const roleId = "22222222-2222-4222-8222-222222222222";
    const raw = {
      target_kind: ["connection_target"],
      source_kind: ["device", "virtual_machine"],
      address_bound: true,
      sort: "name",
      sort_direction: "asc",
      region_ids: [regionId],
      role_ids: [roleId],
      status: ["active"],
      // legacy name operators preserved in passthrough
      region: ["aeza-de"],
      role: ["hypervisor", "switch"],
      // advanced key the builder does not surface
      tags: ["prod"],
    };
    const model = selectorToModel(raw);
    expect(model.facets.region).toEqual([regionId]);
    expect(model.facets.role).toEqual([roleId]);
    expect(model.facets.status).toEqual(["active"]);
    expect(model.addressBound).toBe(true);
    expect(model.passthrough).toEqual({
      region: ["aeza-de"],
      role: ["hypervisor", "switch"],
      tags: ["prod"],
    });

    const out = modelToSelector(model);
    expect(out.region_ids).toEqual([regionId]);
    expect(out.role_ids).toEqual([roleId]);
    expect(out.region).toEqual(["aeza-de"]);
    expect(out.role).toEqual(["hypervisor", "switch"]);
    expect(out.tags).toEqual(["prod"]);
    expect(out.target_kind).toEqual(["connection_target"]);
    expect(out.source_kind).toEqual(["device", "virtual_machine"]);
    expect(out.address_bound).toBe(true);
  });

  it("omits empty facets from the serialized selector", () => {
    const model = emptySelectorModel();
    model.facets.status = [];
    const out = modelToSelector(model);
    expect(out.region_ids).toBeUndefined();
    expect(out.role_ids).toBeUndefined();
    expect(out.status).toBeUndefined();
    expect(out.address_bound).toBe(true);

    const edited = selectorToModel({ region_ids: ["11111111-1111-4111-8111-111111111111"] });
    edited.facets.region = [];
    expect(modelToSelector(edited).region_ids).toBeUndefined();
  });

  it("falls back to defaults for a non-object selector", () => {
    const model = selectorToModel(null);
    expect(model.facets.status).toEqual(["active"]);
    expect(model.addressBound).toBe(true);
    expect(model.sort).toBe("name");
    expect(model.sortDirection).toBe("asc");
  });
});
