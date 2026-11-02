import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

import {
  buildDataTableFilterParams,
  buildFilterParams,
  mapDataTableSortKey,
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  serializeDataTableFilterValuesCsv,
  updateDataTableColumnFilters,
} from "./tableState";

const WEB_SRC_ROOT = path.resolve(import.meta.dirname, "..", "..");

function collectSourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      return collectSourceFiles(fullPath);
    }
    if (!/\.(ts|tsx)$/.test(entry.name)) return [];
    if (entry.name.includes(".test.")) return [];
    return [fullPath];
  });
}

describe("tableState", () => {
  it("keeps only sort keys allowed by the current table schema", () => {
    expect(
      sanitizeDataTableSortState({ key: "status", dir: "asc" }, ["name", "status"]),
    ).toEqual({ key: "status", dir: "asc" });
    expect(
      sanitizeDataTableSortState({ key: "removed-column", dir: "desc" }, ["name", "status"]),
    ).toBeNull();
  });

  it("maps UI sort keys to backend sort keys when needed", () => {
    expect(
      mapDataTableSortKey(
        { key: "lastActivity", dir: "desc" },
        { lastActivity: "last_activity" },
      ),
    ).toBe("last_activity");
    expect(mapDataTableSortKey({ key: "status", dir: "asc" })).toBe("status");
    expect(mapDataTableSortKey(null, { status: "status" })).toBeUndefined();
  });

  it("drops unknown filter keys and empty values", () => {
    expect(
      sanitizeDataTableColumnFilters(
        {
          status: ["active", "active", ""],
          ghost: ["x"],
          role: [],
        },
        ["status", "role"],
      ),
    ).toEqual({
      status: ["active"],
    });
  });

  it("updates filter selections via the shared helper", () => {
    expect(updateDataTableColumnFilters({}, "status", ["active"])).toEqual({
      status: ["active"],
    });
    expect(
      updateDataTableColumnFilters(
        { status: ["active"], role: ["cashier"] },
        "status",
        ["offline"],
      ),
    ).toEqual({
      status: ["offline"],
      role: ["cashier"],
    });
    expect(
      updateDataTableColumnFilters(
        { status: ["active"], role: ["cashier"] },
        "status",
        [],
      ),
    ).toEqual({
      role: ["cashier"],
    });
  });

  it("serializes filter params from the shared table contract", () => {
    expect(
      buildDataTableFilterParams(
        {
          status: ["active"],
          role: ["cashier", "operator"],
        },
        [
          { colKey: "status", param: "status" },
          {
            colKey: "role",
            param: "role_id",
            serialize: serializeDataTableFilterValuesCsv,
          },
        ],
      ),
    ).toEqual({
      status: ["active"],
      role_id: "cashier,operator",
    });
  });

  it("builds CSV-only server filter params and drops array-valued entries", () => {
    expect(
      buildFilterParams(
        { status: ["active", "draft"], role: ["cashier"] },
        [
          { colKey: "status", param: "status", serialize: serializeDataTableFilterValuesCsv },
          { colKey: "role", param: "role_id" },
        ],
      ),
    ).toEqual({
      status: "active,draft",
    });
  });

  it("forbids inline table allowlists in screen-level sanitizers", () => {
    const offenders = collectSourceFiles(WEB_SRC_ROOT).filter((filePath) => {
      const source = fs.readFileSync(filePath, "utf8");
      return (
        /sanitizeDataTableSortState\([^,]+,\s*\[/gs.test(source) ||
        /sanitizeDataTableColumnFilters\([^,]+,\s*\[/gs.test(source)
      );
    });

    expect(offenders).toEqual([]);
  });
});
