import { describe, expect, it } from "vitest";
import { parseDataflowIcmpTypes, parseDataflowPorts } from "./dataflowPorts";

describe("parseDataflowPorts", () => {
  it("keeps the historical comma-separated integers sorted and deduplicated", () => {
    expect(parseDataflowPorts("65535,1,1")).toEqual([1, 65535]);
  });

  it("normalizes inclusive ranges into the bounded representation", () => {
    expect(parseDataflowPorts("8443, 8000-8002, 8001")).toEqual([8000, 8001, 8002, 8443]);
  });

  it("rejects an invalid or widened range instead of clamping it", () => {
    expect(parseDataflowPorts("65535-65536")).toBeNull();
    expect(parseDataflowPorts("0-10")).toBeNull();
    expect(parseDataflowPorts("9000-8000")).toBeNull();
    expect(parseDataflowPorts("80-")).toBeNull();
    expect(parseDataflowPorts("80,,")).toBeNull();
    expect(parseDataflowPorts("tcp/80")).toBeNull();
  });

  it("enforces the 256-value bound after expansion", () => {
    expect(parseDataflowPorts("1-256")).toHaveLength(256);
    expect(parseDataflowPorts("1-257")).toBeNull();
    expect(parseDataflowPorts(Array.from({ length: 257 }, (_, index) => String(index + 1)).join(","))).toBeNull();
  });
});

describe("parseDataflowIcmpTypes", () => {
  it("normalizes unique types from 0 to 255", () => {
    expect(parseDataflowIcmpTypes("255,0,8,8,3")).toEqual([0, 3, 8, 255]);
  });

  it("rejects out-of-bound and non-integer input", () => {
    expect(parseDataflowIcmpTypes("256")).toBeNull();
    expect(parseDataflowIcmpTypes("-1")).toBeNull();
    expect(parseDataflowIcmpTypes("1-2")).toBeNull();
    expect(parseDataflowIcmpTypes("8,")).toBeNull();
    expect(parseDataflowIcmpTypes("")).toBeNull();
  });

  it("allows all 256 values but not more", () => {
    expect(parseDataflowIcmpTypes(Array.from({ length: 256 }, (_, index) => String(index)).join(","))).toHaveLength(256);
    expect(parseDataflowIcmpTypes(Array.from({ length: 257 }, (_, index) => String(index)).join(","))).toBeNull();
  });
});
