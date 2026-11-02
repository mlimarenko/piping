import { describe, expect, it } from "vitest";

import { modelToSelector, selectorToModel } from "./selectorModel";

describe("catalog Tag target selectors", () => {
  it("preserves catalog UUID predicates while editing another facet", () => {
    const tagIds = ["11111111-1111-4111-8111-111111111111"];
    const model = selectorToModel({ tag_ids: tagIds, role: ["Router"] });
    model.facets.role = ["Firewall"];

    expect(modelToSelector(model)).toMatchObject({
      tag_ids: tagIds,
      role: ["Firewall"],
    });
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
});
