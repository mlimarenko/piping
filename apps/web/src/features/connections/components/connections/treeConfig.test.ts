import { describe, expect, it } from "vitest";

import {
  DEFAULT_TREE_CONFIG,
  normalizeTreeConfig,
} from "./treeConfig";

describe("connection tree config", () => {
  it("falls back to defaults for malformed preference values", () => {
    expect(normalizeTreeConfig(null)).toEqual(DEFAULT_TREE_CONFIG);
    expect(normalizeTreeConfig({ mode: "custom", order: "site" })).toEqual({
      ...DEFAULT_TREE_CONFIG,
      mode: "custom",
    });
  });

  it("keeps configs without includeUnknown on the default behavior", () => {
    expect(
      normalizeTreeConfig({
        mode: "custom",
        order: ["role", "unknown-key", "site"],
      }),
    ).toEqual({
      mode: "custom",
      order: ["role", "site"],
      includeUnknown: true,
      quickAccess: DEFAULT_TREE_CONFIG.quickAccess,
    });
  });

  it("preserves explicit includeUnknown=false", () => {
    expect(
      normalizeTreeConfig({
        mode: "auto",
        order: ["region"],
        includeUnknown: false,
      }),
    ).toEqual({
      mode: "auto",
      order: ["region"],
      includeUnknown: false,
      quickAccess: DEFAULT_TREE_CONFIG.quickAccess,
    });
  });

  it("normalizes corrupted quick access settings", () => {
    expect(
      normalizeTreeConfig({
        mode: "custom",
        order: ["site"],
        quickAccess: {
          enabled: false,
          protocols: ["ssh", "rdp", "ssh", "bad"],
          maxButtons: 99,
          showLabels: true,
        },
      }),
    ).toEqual({
      mode: "custom",
      order: ["site"],
      includeUnknown: true,
      quickAccess: {
        enabled: false,
        protocols: ["ssh", "rdp"],
      },
    });
  });
});
