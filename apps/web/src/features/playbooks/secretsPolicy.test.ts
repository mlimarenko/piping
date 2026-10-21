import { describe, expect, it } from "vitest";
import {
  isReservedEnvName,
  readSecretEnv,
  secretEnvUnchanged,
  serializeSecretEnv,
  validateEnvName,
  writeSecretEnv,
  type SecretEnvRow,
} from "./secretsPolicy";

const row = (envName: string, key: string): SecretEnvRow => ({
  id: `${envName}:${key}`,
  envName,
  key,
});

describe("validateEnvName", () => {
  it("flags empty, malformed and reserved names", () => {
    expect(validateEnvName("")).toBe("empty");
    expect(validateEnvName("  ")).toBe("empty");
    expect(validateEnvName("lowercase")).toBe("pattern");
    expect(validateEnvName("9LEADING")).toBe("pattern");
    expect(validateEnvName("HAS-DASH")).toBe("pattern");
    expect(validateEnvName("PIPING_TOKEN")).toBe("reserved");
    // Lowercase fails the pattern check first; the reserved check only matters
    // for otherwise-valid uppercase names.
    expect(validateEnvName("ansible_x")).toBe("pattern");
    expect(validateEnvName("ANSIBLE_X")).toBe("reserved");
    expect(validateEnvName("HOME")).toBe("reserved");
    expect(validateEnvName("PATH")).toBe("reserved");
  });

  it("accepts a valid uppercase identifier", () => {
    expect(validateEnvName("GRAD_TOKEN")).toBeNull();
    expect(validateEnvName("_X1")).toBeNull();
  });
});

describe("isReservedEnvName", () => {
  it("matches reserved prefixes case-insensitively", () => {
    expect(isReservedEnvName("ssh_key")).toBe(true);
    expect(isReservedEnvName("EXT_KEY")).toBe(false);
  });
});

describe("readSecretEnv", () => {
  it("returns rows for a well-formed policy", () => {
    const rows = readSecretEnv({
      runtime: { secrets: { env: { GRAD_TOKEN: "grad-token", EXT_KEY: "ext" } } },
    });
    expect(rows.map((r) => ({ envName: r.envName, key: r.key }))).toEqual([
      { envName: "GRAD_TOKEN", key: "grad-token" },
      { envName: "EXT_KEY", key: "ext" },
    ]);
    expect(rows.every((r) => typeof r.id === "string" && r.id.length > 0)).toBe(true);
  });

  it("degrades to empty on malformed shapes", () => {
    expect(readSecretEnv(null)).toEqual([]);
    expect(readSecretEnv("nope")).toEqual([]);
    expect(readSecretEnv({ runtime: 5 })).toEqual([]);
    expect(readSecretEnv({ runtime: { secrets: { env: [] } } })).toEqual([]);
    expect(readSecretEnv({ runtime: { secrets: { env: { A: 1 } } } })).toEqual([]);
  });
});

describe("serializeSecretEnv", () => {
  it("drops blank rows and lets the last duplicate win", () => {
    expect(
      serializeSecretEnv([row("A", "a"), row(" ", "x"), row("B", " "), row("A", "a2")]),
    ).toEqual({ A: "a2" });
  });
});

describe("writeSecretEnv", () => {
  it("merges env while preserving sibling token_policy and runtime keys", () => {
    const base = {
      ttl_seconds: 60,
      runtime: { flavor: "ansible", secrets: { redact: true, env: { OLD: "old" } } },
    };
    const merged = writeSecretEnv(base, [row("NEW", "new-key")]);
    expect(merged).toEqual({
      ttl_seconds: 60,
      runtime: {
        flavor: "ansible",
        secrets: { redact: true, env: { NEW: "new-key" } },
      },
    });
  });

  it("persists an empty declaration as an empty env map", () => {
    expect(writeSecretEnv({ runtime: { secrets: { env: { A: "a" } } } }, [])).toEqual({
      runtime: { secrets: { env: {} } },
    });
  });

  it("builds a fresh policy from a non-object base", () => {
    expect(writeSecretEnv(undefined, [row("A", "a")])).toEqual({
      runtime: { secrets: { env: { A: "a" } } },
    });
  });
});

describe("secretEnvUnchanged", () => {
  it("is true only when the declared map is identical", () => {
    const policy = { runtime: { secrets: { env: { A: "a", B: "b" } } } };
    expect(secretEnvUnchanged(policy, [row("A", "a"), row("B", "b")])).toBe(true);
    expect(secretEnvUnchanged(policy, [row("B", "b"), row("A", "a")])).toBe(true);
    expect(secretEnvUnchanged(policy, [row("A", "a")])).toBe(false);
    expect(secretEnvUnchanged(policy, [row("A", "a"), row("B", "c")])).toBe(false);
  });
});
