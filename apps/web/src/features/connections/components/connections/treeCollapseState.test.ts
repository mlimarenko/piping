import { describe, expect, it } from "vitest";

import {
  collapsedTreeGroupSetForScope,
  normalizeTreeCollapsedGroups,
  treeCollapseScope,
  withCollapsedTreeGroupScope,
} from "./treeCollapseState";

describe("connection tree collapsed state", () => {
  it("scopes collapsed groups by the rendered hierarchy", () => {
    expect(treeCollapseScope(["site", "role"], true)).toBe("v1:unknown:site>role");
    expect(treeCollapseScope(["role", "site"], true)).toBe("v1:unknown:role>site");
  });

  it("normalizes malformed persisted values", () => {
    expect(normalizeTreeCollapsedGroups(null)).toEqual({ version: 1, byScope: {} });
    expect(
      normalizeTreeCollapsedGroups({
        byScope: {
          scope: [" b ", "a", "a", "", 42],
          bad: "site=DC|",
        },
      }),
    ).toEqual({
      version: 1,
      byScope: {
        scope: ["a", "b"],
      },
    });
  });

  it("keeps only collapsed groups that exist in the current tree", () => {
    const validKeys = new Set(["site=Amsterdam|", "site=Frankfurt|"]);
    const scope = treeCollapseScope(["site"], true);
    const stored = withCollapsedTreeGroupScope(
      null,
      scope,
      ["site=Amsterdam|", "site=Missing|"],
      validKeys,
    );

    expect(stored.byScope[scope]).toEqual(["site=Amsterdam|"]);
    expect([...collapsedTreeGroupSetForScope(stored, scope, validKeys)]).toEqual([
      "site=Amsterdam|",
    ]);
  });

  it("stores an explicit empty scope instead of falling back to the pre-scope key", () => {
    const validKeys = new Set(["site=Amsterdam|"]);
    const scope = treeCollapseScope(["site"], true);
    const stored = withCollapsedTreeGroupScope(
      { byScope: { [scope]: ["site=Amsterdam|"] } },
      scope,
      [],
      validKeys,
    );

    expect(stored.byScope[scope]).toEqual([]);
    expect([
      ...collapsedTreeGroupSetForScope(
        stored,
        scope,
        validKeys,
        ["site=Amsterdam|"],
      ),
    ]).toEqual([]);
  });
});
