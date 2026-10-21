import { describe, expect, it } from "vitest";
import { inventoryResourceOption } from "./resourceOptions";

describe("inventoryResourceOption", () => {
  it("disambiguates device models by manufacturer and keeps slug separate from search text", () => {
    const option = inventoryResourceOption("Device Types", {
      id: "type-a", name: "Core Switch", type: "device_type", slug: "core-switch",
      manufacturerName: "Maker A", manufacturerId: "maker-a",
    });
    expect(option.subtitle).toBe("Maker A · maker-a");
    expect(option.searchText).toContain("Maker A");
    expect(option.searchText).toContain("maker-a");
    expect(option.slug).toBe("core-switch");
  });
});
