import { describe, it, expect } from "vitest";
import {
  ENTITY_SOURCES,
  entityIdFromPattern,
  getEntitySource,
  isAllPattern,
  patternForEntity,
} from "./entitySources";

describe("entitySources pattern helpers", () => {
  it("builds <prefix>:<id> patterns", () => {
    expect(patternForEntity("connection_target", "abc")).toBe("connection_target:abc");
    expect(patternForEntity("config_context", "x")).toBe("config_context:x");
  });

  it("extracts the id from a matching pattern, null otherwise", () => {
    expect(entityIdFromPattern("connection_target", "connection_target:abc")).toBe("abc");
    expect(entityIdFromPattern("connection_target", "user:abc")).toBeNull();
    expect(entityIdFromPattern("connection_target", "*")).toBeNull();
  });

  it("treats empty and *-only sets as all-resources", () => {
    expect(isAllPattern([])).toBe(true);
    expect(isAllPattern(["*"])).toBe(true);
    expect(isAllPattern(["connection_target:a"])).toBe(false);
    expect(isAllPattern(["*", "connection_target:a"])).toBe(false);
  });
});

describe("getEntitySource", () => {
  it("resolves underscore catalog types", () => {
    expect(getEntitySource("connection_target")).toBe(ENTITY_SOURCES.connection_target);
    expect(getEntitySource("config_context")).toBe(ENTITY_SOURCES.config_context);
  });

  it("accepts the hyphenated variant defensively", () => {
    expect(getEntitySource("config-context")).toBe(ENTITY_SOURCES.config_context);
  });

  it("returns undefined for types without an entity list", () => {
    expect(getEntitySource("audit_event")).toBeUndefined();
    expect(getEntitySource("keycloak")).toBeUndefined();
  });

  it("uses canonical resource_id prefixes matching the backend", () => {
    expect(ENTITY_SOURCES.connection_target?.prefix).toBe("connection_target");
    expect(ENTITY_SOURCES.config_context?.prefix).toBe("config_context");
    expect(ENTITY_SOURCES.user?.prefix).toBe("user");
    expect(ENTITY_SOURCES.group?.prefix).toBe("group");
    expect(ENTITY_SOURCES.role?.prefix).toBe("role");
    expect(ENTITY_SOURCES.policy?.prefix).toBe("policy");
  });
});
