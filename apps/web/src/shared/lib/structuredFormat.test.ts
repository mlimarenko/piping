import { serializeStructured } from "./structuredFormat";

describe("serializeStructured", () => {
  const sample = { name: "edge", nested: { enabled: true }, tags: ["a", "b"] };

  it("renders pretty JSON", () => {
    const { text, fellBackToJson } = serializeStructured(sample, "json");
    expect(fellBackToJson).toBeUndefined();
    expect(text).toBe(JSON.stringify(sample, null, 2));
  });

  it("renders YAML", () => {
    const { text, fellBackToJson } = serializeStructured(sample, "yaml");
    expect(fellBackToJson).toBeUndefined();
    expect(text).toContain("name: edge");
    expect(text).toContain("enabled: true");
  });

  it("renders TOML for a top-level table", () => {
    const { text, fellBackToJson } = serializeStructured(sample, "toml");
    expect(fellBackToJson).toBeUndefined();
    expect(text).toContain('name = "edge"');
  });

  it("falls back to JSON when TOML cannot encode the value (null leaf)", () => {
    const result = serializeStructured({ a: null }, "toml");
    expect(result.fellBackToJson).toBe(true);
    expect(result.text).toBe(JSON.stringify({ a: null }, null, 2));
  });

  it("falls back to JSON when a nested null would be silently dropped by TOML", () => {
    const value = { outer: { keep: 1, drop: null } };
    const result = serializeStructured(value, "toml");
    expect(result.fellBackToJson).toBe(true);
    expect(result.text).toBe(JSON.stringify(value, null, 2));
  });

  it("renders TOML for mixed-type arrays (no null)", () => {
    const result = serializeStructured({ arr: [1, "two", true] }, "toml");
    expect(result.fellBackToJson).toBeUndefined();
    expect(result.text).toContain("arr = [");
  });

  it("falls back to JSON when the top level is not a TOML table (array)", () => {
    const result = serializeStructured([1, 2, 3], "toml");
    expect(result.fellBackToJson).toBe(true);
    expect(result.text).toBe(JSON.stringify([1, 2, 3], null, 2));
  });

  it("treats null/undefined as an empty object", () => {
    expect(serializeStructured(null, "json").text).toBe("{}");
    expect(serializeStructured(undefined, "yaml").text.trim()).toBe("{}");
  });
});
