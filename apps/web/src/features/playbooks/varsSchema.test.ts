import { describe, expect, it } from "vitest";
import { effectiveParameters, formatParameterValues, jobVarsSchema, missingRequiredParameters, parameterErrors, parseParameterValues, parseVarsSchema, projectFileVarsSchema } from "./varsSchema";

const schema = { type: "object", properties: {
  name: { type: "string", default: "default" },
  count: { type: "integer", default: 5, minimum: 0 },
  enabled: { type: "boolean", default: true },
  hosts: { type: "array", items: { type: "string" } },
  settings: { type: "object" },
  note: { type: ["string", "null"] },
}, required: ["name", "count", "enabled", "note"] };

describe("canonical launch parameters", () => {
  it("uses one explicit source and refuses a missing or symlinked schema file", () => {
    expect(jobVarsSchema({ kind: "inline", schema })).toEqual(schema);
    expect(jobVarsSchema({ kind: "project_file", path: "schema.json" })).toBeUndefined();
    expect(parameterErrors(jobVarsSchema({ kind: "none" }), { hidden: "value" })).toEqual({ hidden: "undeclared" });
    const entries = [{ path: "schema.json", content: { kind: "file" as const, executable: false, bytes: Array.from(new TextEncoder().encode(JSON.stringify(schema))) } }];
    expect(projectFileVarsSchema(entries, "schema.json")).toEqual(schema);
    expect(() => projectFileVarsSchema(entries, "missing.json")).toThrow();
    expect(() => projectFileVarsSchema([{ path: "schema.json", content: { kind: "symlink", target: "other.json" } }], "schema.json")).toThrow();
  });

  it("treats a secret declaration as a reference, not an extra-vars value", () => {
    const secretSchema = { type: "object", properties: { TOKEN: { type: "string", writeOnly: true, format: "secret-reference" } }, required: ["TOKEN"] };
    expect(parameterErrors(secretSchema, {})).toEqual({});
    expect(missingRequiredParameters(secretSchema, {})).toEqual([]);
    expect(effectiveParameters(secretSchema, { TOKEN: "reference" }, {})).toEqual({});
  });
  it("preserves empty, false, zero and null through YAML and JSON", () => {
    const values = { name: "", count: 0, enabled: false, hosts: ["on", "001"], settings: { nested: [false, null] }, note: null };
    for (const format of ["yaml", "json"] as const) {
      expect(parseParameterValues(formatParameterValues(values, format), format)).toEqual(values);
    }
    expect(parameterErrors(schema, values)).toEqual({});
  });

  it("resolves defaults, then job values, then launch or schedule overrides without truthiness", () => {
    const job = { name: "job", count: 8, note: "saved" };
    const overrides = { name: "", count: 0, enabled: false, note: null };
    const effective = effectiveParameters(schema, job, overrides);
    expect(effective).toEqual(overrides);
    expect(job).toEqual({ name: "job", count: 8, note: "saved" });
    expect(missingRequiredParameters(schema, effective)).toEqual([]);
    expect(missingRequiredParameters(schema, effectiveParameters(schema, {}, {})).map((spec) => spec.name)).toEqual(["note"]);
  });

  it("keeps schema constraints in the form projection", () => {
    const count = parseVarsSchema(schema).find((spec) => spec.name === "count")!;
    expect(count.schema).toEqual(schema.properties.count);
    expect(parameterErrors(schema, { count: -1, enabled: "false", hosts: {}, note: null, extra: 1 })).toEqual({ name: "required", count: "range", enabled: "type", hosts: "type", extra: "undeclared" });
    expect(parameterErrors(schema, { count: 1.5 }, false)).toEqual({ count: "type" });
  });

  it("rejects documents that cannot be canonical JSON objects", () => {
    for (const text of ["x: 1\nx: 2", "x: &x [1]\ny: *x", "x: .nan", "- one", "null"]) {
      expect(() => parseParameterValues(text, "yaml")).toThrow();
    }
    expect(() => parseParameterValues("[]")).toThrow();
  });
});
