import { describe, it, expect } from "vitest";
import type { TokenScopeCatalogItem } from "@/features/user-settings/api";
import {
  computeScopePacks,
  distinctScopeValues,
  groupScopesByResource,
  isSubsetSelected,
  manageScopeValues,
  patternsForScope,
  readScopeValues,
  reconcileSelectedScopes,
  setScopeTargetRows,
  toggleBundle,
} from "./tokenScopes";

interface Row {
  scope: string;
  resourcePattern: string;
}
const makeRow = (scope: string, resourcePattern: string): Row => ({ scope, resourcePattern });

const item = (action: string, resourceType: string): TokenScopeCatalogItem => ({
  action,
  resourceType,
  label: `${action} ${resourceType}`,
  value: `${action}:${resourceType}`,
});

/** A trimmed but representative slice of the 43-scope backend catalog. */
const catalog: TokenScopeCatalogItem[] = [
  item("read", "connection_target"),
  item("connect", "connection_target"),
  item("observe", "connection_target"),
  item("execute", "connection_target"),
  item("update", "connection_target"),
  item("read", "connection_session"),
  item("create", "connection_session"),
  item("update", "connection_session"),
  item("delete", "connection_session"),
  item("read", "inventory"),
  item("create", "inventory"),
  item("update", "inventory"),
  item("delete", "inventory"),
  item("read", "config_context"),
  item("create", "config_context"),
  item("read", "playbook"),
  item("create", "playbook"),
  item("update", "playbook"),
  item("delete", "playbook"),
  item("execute", "playbook"),
  item("read", "playbook_version"),
  item("create", "playbook_version"),
  item("read", "playbook_assignment"),
  item("create", "playbook_assignment"),
  item("update", "playbook_assignment"),
  item("delete", "playbook_assignment"),
  item("read", "playbook_target_set"),
  item("create", "playbook_target_set"),
  item("update", "playbook_target_set"),
  item("delete", "playbook_target_set"),
  item("read", "playbook_run_group"),
  item("create", "playbook_run_group"),
  item("read", "playbook_run"),
  item("update", "playbook_run"),
  item("read", "playbook_schedule"),
  item("create", "playbook_schedule"),
  item("update", "playbook_schedule"),
  item("delete", "playbook_schedule"),
  item("read", "playbook_artifact"),
  item("read", "user"),
  item("create", "user"),
  item("read", "role"),
  item("manage_roles", "role"),
  item("read", "audit_event"),
  item("view_audit", "audit_event"),
];

describe("groupScopesByResource", () => {
  it("groups by resource type in a stable, known-first order", () => {
    const groups = groupScopesByResource(catalog);
    const order = groups.map((g) => g.resourceType);
    expect(order.indexOf("connection_target")).toBeLessThan(order.indexOf("inventory"));
    expect(order.indexOf("inventory")).toBeLessThan(order.indexOf("playbook"));
    expect(order.indexOf("playbook_artifact")).toBeLessThan(order.indexOf("user"));
    const connection_target = groups.find((g) => g.resourceType === "connection_target");
    expect(connection_target?.scopes.map((s) => s.action)).toEqual([
      "read",
      "connect",
      "observe",
      "execute",
      "update",
    ]);
  });

  it("falls back to alphabetical for unknown resource types", () => {
    const unknown = [item("read", "zeta"), item("read", "alpha")];
    const order = groupScopesByResource(unknown).map((g) => g.resourceType);
    expect(order).toEqual(["alpha", "zeta"]);
  });
});

describe("readScopeValues / manageScopeValues", () => {
  it("read picks read/observe/view_audit only", () => {
    const connection_target = catalog.filter((s) => s.resourceType === "connection_target");
    expect(readScopeValues(connection_target)).toEqual(["read:connection_target", "observe:connection_target"]);
    const audit = catalog.filter((s) => s.resourceType === "audit_event");
    expect(readScopeValues(audit)).toEqual([
      "read:audit_event",
      "view_audit:audit_event",
    ]);
  });

  it("manage picks every action for the resource", () => {
    const session = catalog.filter((s) => s.resourceType === "connection_session");
    expect(manageScopeValues(session)).toEqual([
      "read:connection_session",
      "create:connection_session",
      "update:connection_session",
      "delete:connection_session",
    ]);
  });
});

describe("computeScopePacks", () => {
  it("derives role packs intersected with the catalog", () => {
    const packs = computeScopePacks(catalog);
    const byId = (id: string): string[] => packs.find((p) => p.id === id)?.scopes ?? [];

    expect(byId("readOnly")).toContain("read:connection_target");
    expect(byId("readOnly")).toContain("observe:connection_target");
    expect(byId("readOnly")).toContain("view_audit:audit_event");
    expect(byId("readOnly")).not.toContain("connect:connection_target");

    expect(byId("connectionsOperator")).toEqual(
      expect.arrayContaining([
        "read:connection_target",
        "connect:connection_target",
        "observe:connection_target",
        "execute:connection_target",
        "read:connection_session",
        "delete:connection_session",
      ]),
    );
    // connection_target:update is not part of connections operator
    expect(byId("connectionsOperator")).not.toContain("update:connection_target");

    expect(byId("inventoryManager")).toEqual(
      expect.arrayContaining(["read:inventory", "delete:inventory", "create:config_context"]),
    );

    expect(byId("playbookOperator")).toEqual(
      expect.arrayContaining([
        "read:playbook",
        "execute:playbook",
        "create:playbook_run_group",
        "read:playbook_run",
        "read:playbook_artifact",
      ]),
    );
    expect(byId("playbookOperator")).not.toContain("delete:playbook");
    expect(byId("playbookOperator")).not.toContain("update:playbook_schedule");

    expect(byId("playbookManager")).toEqual(
      expect.arrayContaining([
        "create:playbook",
        "delete:playbook_assignment",
        "update:playbook_target_set",
        "update:playbook_run",
        "delete:playbook_schedule",
        "read:playbook_artifact",
      ]),
    );

    expect(byId("accessAdmin")).toEqual(
      expect.arrayContaining(["read:user", "create:user", "manage_roles:role"]),
    );

    expect(byId("fullAccess")).toHaveLength(catalog.length);
  });

  it("omits packs that have no available scopes", () => {
    const tiny = [item("read", "inventory")];
    const packs = computeScopePacks(tiny);
    const ids = packs.map((p) => p.id);
    expect(ids).toContain("readOnly");
    expect(ids).toContain("inventoryManager");
    expect(ids).toContain("fullAccess");
    // No connection_target/session scopes available → no connections operator pack
    expect(ids).not.toContain("connectionsOperator");
    expect(ids).not.toContain("playbookOperator");
    expect(ids).not.toContain("playbookManager");
    expect(ids).not.toContain("accessAdmin");
  });

  it("returns no packs for an empty catalog", () => {
    expect(computeScopePacks([])).toEqual([]);
  });
});

describe("isSubsetSelected", () => {
  it("is true only when every value is selected", () => {
    const selected = new Set(["read:connection_target", "connect:connection_target"]);
    expect(isSubsetSelected(["read:connection_target"], selected)).toBe(true);
    expect(isSubsetSelected(["read:connection_target", "connect:connection_target"], selected)).toBe(true);
    expect(isSubsetSelected(["read:connection_target", "execute:connection_target"], selected)).toBe(false);
  });

  it("is false for an empty bundle", () => {
    expect(isSubsetSelected([], new Set(["read:connection_target"]))).toBe(false);
  });
});

describe("toggleBundle", () => {
  it("adds missing values when the bundle is not fully selected", () => {
    const next = toggleBundle(["read:connection_target", "connect:connection_target"], ["read:connection_target"]);
    expect(next).toEqual(["read:connection_target", "connect:connection_target"]);
  });

  it("removes the whole bundle when already fully selected", () => {
    const next = toggleBundle(["read:connection_target", "connect:connection_target"], [
      "read:connection_target",
      "connect:connection_target",
      "read:inventory",
    ]);
    expect(next).toEqual(["read:inventory"]);
  });

  it("preserves existing unrelated selections and order", () => {
    const next = toggleBundle(["read:user"], ["read:inventory", "read:connection_target"]);
    expect(next).toEqual(["read:inventory", "read:connection_target", "read:user"]);
  });

  it("does not duplicate already-selected values", () => {
    const next = toggleBundle(["read:connection_target", "connect:connection_target"], ["read:connection_target"]);
    expect(next.filter((v) => v === "read:connection_target")).toHaveLength(1);
  });
});

describe("distinctScopeValues / patternsForScope", () => {
  it("collapses multiple rows of the same scope to one value, in order", () => {
    const rows = [
      makeRow("connect:connection_target", "connection_target:a"),
      makeRow("connect:connection_target", "connection_target:b"),
      makeRow("read:inventory", "*"),
    ];
    expect(distinctScopeValues(rows)).toEqual(["connect:connection_target", "read:inventory"]);
  });

  it("returns every pattern attached to a scope", () => {
    const rows = [
      makeRow("connect:connection_target", "connection_target:a"),
      makeRow("connect:connection_target", "connection_target:b"),
      makeRow("read:inventory", "*"),
    ];
    expect(patternsForScope(rows, "connect:connection_target")).toEqual(["connection_target:a", "connection_target:b"]);
    expect(patternsForScope(rows, "read:inventory")).toEqual(["*"]);
  });
});

describe("reconcileSelectedScopes", () => {
  it("preserves all rows for retained scopes and seeds new scopes with *", () => {
    const rows = [
      makeRow("connect:connection_target", "connection_target:a"),
      makeRow("connect:connection_target", "connection_target:b"),
    ];
    const next = reconcileSelectedScopes(rows, ["connect:connection_target", "read:inventory"], makeRow);
    expect(next).toEqual([
      { scope: "connect:connection_target", resourcePattern: "connection_target:a" },
      { scope: "connect:connection_target", resourcePattern: "connection_target:b" },
      { scope: "read:inventory", resourcePattern: "*" },
    ]);
  });

  it("drops rows whose scope is no longer selected", () => {
    const rows = [
      makeRow("connect:connection_target", "connection_target:a"),
      makeRow("read:inventory", "*"),
    ];
    const next = reconcileSelectedScopes(rows, ["read:inventory"], makeRow);
    expect(next).toEqual([{ scope: "read:inventory", resourcePattern: "*" }]);
  });
});

describe("setScopeTargetRows", () => {
  const base = [
    makeRow("connect:connection_target", "*"),
    makeRow("read:inventory", "*"),
  ];

  it("expands one scope into one row per entity pattern, preserving position", () => {
    const next = setScopeTargetRows(base, "connect:connection_target", ["connection_target:a", "connection_target:b"], makeRow);
    expect(next).toEqual([
      { scope: "connect:connection_target", resourcePattern: "connection_target:a" },
      { scope: "connect:connection_target", resourcePattern: "connection_target:b" },
      { scope: "read:inventory", resourcePattern: "*" },
    ]);
  });

  it("collapses an empty or *-only selection back to a single * row", () => {
    const targeted = [makeRow("connect:connection_target", "connection_target:a"), makeRow("connect:connection_target", "connection_target:b")];
    expect(setScopeTargetRows(targeted, "connect:connection_target", [], makeRow)).toEqual([
      { scope: "connect:connection_target", resourcePattern: "*" },
    ]);
    expect(setScopeTargetRows(targeted, "connect:connection_target", ["*"], makeRow)).toEqual([
      { scope: "connect:connection_target", resourcePattern: "*" },
    ]);
  });

  it("ignores a stray * mixed with specific patterns", () => {
    const next = setScopeTargetRows(base, "connect:connection_target", ["*", "connection_target:a"], makeRow);
    expect(patternsForScope(next, "connect:connection_target")).toEqual(["connection_target:a"]);
  });
});
