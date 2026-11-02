import { describe, expect, it } from "vitest";

import type { PlaybookTargetSet } from "@/features/playbooks/api";

import { asJsonObject, jsonObjectSize, targetSetLabel } from "./playbookPageFormat";

function targetSet(partial: Partial<PlaybookTargetSet>): PlaybookTargetSet {
  return {
    id: partial.id ?? "ts-1",
    name: partial.name ?? "Production",
    mode: partial.mode ?? "dynamic",
    entity_kind: partial.entity_kind ?? "host",
    selector: partial.selector ?? {},
    created_at: partial.created_at ?? "2026-01-01T00:00:00Z",
    updated_at: partial.updated_at ?? "2026-01-01T00:00:00Z",
  };
}

describe("asJsonObject", () => {
  it("returns an empty object for null and undefined", () => {
    expect(asJsonObject(null)).toEqual({});
    expect(asJsonObject(undefined)).toEqual({});
  });

  it("returns an empty object for arrays and non-objects", () => {
    expect(asJsonObject([1, 2, 3])).toEqual({});
    expect(asJsonObject("text")).toEqual({});
    expect(asJsonObject(42)).toEqual({});
    expect(asJsonObject(true)).toEqual({});
  });

  it("returns the object for a plain object", () => {
    const value = { a: 1, b: "two" };
    expect(asJsonObject(value)).toBe(value);
  });
});

describe("jsonObjectSize", () => {
  it("counts the keys of a plain object", () => {
    expect(jsonObjectSize({ a: 1, b: 2, c: 3 })).toBe(3);
    expect(jsonObjectSize({})).toBe(0);
  });

  it("counts zero for non-objects", () => {
    expect(jsonObjectSize(null)).toBe(0);
    expect(jsonObjectSize([1, 2, 3])).toBe(0);
    expect(jsonObjectSize("text")).toBe(0);
  });
});

describe("targetSetLabel", () => {
  it('returns "—" without a target set', () => {
    expect(targetSetLabel()).toBe("—");
    expect(targetSetLabel(undefined)).toBe("—");
  });

  it("returns name and mode with a target set", () => {
    expect(targetSetLabel(targetSet({ name: "Production", mode: "static" }))).toBe(
      "Production · static",
    );
  });
});
