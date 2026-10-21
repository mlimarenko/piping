import { describe, expect, it } from "vitest";
import { formatCoord, parseCoord, parseLat, parseLng } from "@/shared/forms/coordUtils";

describe("parseCoord", () => {
  it("parses a valid in-range number", () => {
    expect(parseCoord("12.5", -90, 90)).toBe(12.5);
    expect(parseCoord(" -7 ", -90, 90)).toBe(-7);
  });

  it("returns null for blank / whitespace (not 0)", () => {
    expect(parseCoord("", -90, 90)).toBeNull();
    expect(parseCoord("   ", -90, 90)).toBeNull();
    expect(parseCoord(null, -90, 90)).toBeNull();
    expect(parseCoord(undefined, -90, 90)).toBeNull();
  });

  it("returns null for non-numeric input", () => {
    expect(parseCoord("abc", -90, 90)).toBeNull();
    expect(parseCoord("1.2.3", -90, 90)).toBeNull();
    expect(parseCoord("NaN", -90, 90)).toBeNull();
  });

  it("rejects out-of-range values", () => {
    expect(parseCoord("90.1", -90, 90)).toBeNull();
    expect(parseCoord("-90.1", -90, 90)).toBeNull();
    expect(parseCoord("90", -90, 90)).toBe(90);
    expect(parseCoord("-90", -90, 90)).toBe(-90);
  });

  it("rejects non-finite values", () => {
    expect(parseCoord("Infinity", -90, 90)).toBeNull();
  });
});

describe("parseLat / parseLng", () => {
  it("applies latitude bounds", () => {
    expect(parseLat("89.9999")).toBe(89.9999);
    expect(parseLat("120")).toBeNull();
  });

  it("applies longitude bounds", () => {
    expect(parseLng("179.5")).toBe(179.5);
    expect(parseLng("181")).toBeNull();
    expect(parseLng("-181")).toBeNull();
  });

  it("treats empty string as null, never the equator/meridian", () => {
    expect(parseLat("")).toBeNull();
    expect(parseLng("")).toBeNull();
  });
});

describe("formatCoord", () => {
  it("formats to 6 decimal places", () => {
    expect(formatCoord(55.751244)).toBe("55.751244");
    expect(formatCoord(37.6)).toBe("37.600000");
    expect(formatCoord(-7)).toBe("-7.000000");
  });
});
