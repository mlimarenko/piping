import { describe, expect, it } from "vitest";

import {
  emptySelectorModel,
  modelToSelector,
  selectorToModel,
} from "./selectorModel";

describe("selectorModel", () => {
  it("round-trips a dynamic selector without dropping advanced keys", () => {
    const raw = {
      target_kind: ["connection_target"],
      source_kind: ["device", "virtual_machine"],
      address_bound: true,
      sort: "name",
      sort_direction: "asc",
      region: ["aeza-de"],
      role: ["hypervisor", "switch"],
      status: ["active"],
      // advanced key the builder does not surface
      tags: ["prod"],
    };
    const model = selectorToModel(raw);
    expect(model.facets.region).toEqual(["aeza-de"]);
    expect(model.facets.role).toEqual(["hypervisor", "switch"]);
    expect(model.addressBound).toBe(true);
    expect(model.passthrough).toEqual({ tags: ["prod"] });

    const out = modelToSelector(model);
    expect(out.region).toEqual(["aeza-de"]);
    expect(out.role).toEqual(["hypervisor", "switch"]);
    expect(out.tags).toEqual(["prod"]);
    expect(out.target_kind).toEqual(["connection_target"]);
  });

  it("omits empty facets from the serialized selector", () => {
    const model = emptySelectorModel();
    model.facets.status = [];
    const out = modelToSelector(model);
    expect(out.region).toBeUndefined();
    expect(out.role).toBeUndefined();
    expect(out.status).toBeUndefined();
    expect(out.address_bound).toBe(true);
  });

  it("falls back to defaults for a non-object selector", () => {
    const model = selectorToModel(null);
    expect(model.facets.status).toEqual(["active"]);
    expect(model.addressBound).toBe(true);
  });
});
