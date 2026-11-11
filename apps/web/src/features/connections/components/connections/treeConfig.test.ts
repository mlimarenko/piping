import { describe, expect, it } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { createElement, useEffect, useState, type ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import {
  DEFAULT_TREE_CONFIG,
  computeAutoOrder,
  normalizeTreeConfig,
  useTreeConfig,
} from "./treeConfig";

describe("connection tree config", () => {
  it("does not rehydrate consumers on unrelated renders and still applies saved layout changes", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
    const key = ["settings", "effective", "connections.tree.config"];
    client.setQueryData(key, { key: "connections.tree.config", value: DEFAULT_TREE_CONFIG, source: "user" });
    const observed: unknown[] = [];
    const { result, unmount } = renderHook(() => {
      const { config } = useTreeConfig();
      const [tick, setTick] = useState(0);
      useEffect(() => { observed.push(config); }, [config]);
      return { config, tick, bump: () => setTick(value => value + 1) };
    }, {
      wrapper: ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client }, children),
    });
    try {
      expect(observed).toHaveLength(1);
      act(() => result.current.bump());
      act(() => result.current.bump());
      expect(observed).toHaveLength(1);
      act(() => client.setQueryData(key, {
        key: "connections.tree.config", source: "user",
        value: { ...DEFAULT_TREE_CONFIG, mode: "custom", order: ["site"] },
      }));
      await waitFor(() => expect(result.current.config.order).toEqual(["site"]));
      expect(observed).toHaveLength(2);
    } finally {
      unmount();
      client.clear();
    }
  });
  it("defaults to region, site, role and actual device type while retaining custom order", () => {
    const host = { region: "Europe", site: "Berlin", role: "Router", deviceType: "MX", tenant: "Tenant", platform: "Linux", cluster: "Cluster", targetKind: "virtual_machine" };
    expect(computeAutoOrder([host])).toEqual(["region", "site", "role", "deviceType"]);
    expect(normalizeTreeConfig(undefined).order).toEqual(["region", "site", "role", "deviceType"]);
    expect(normalizeTreeConfig({ mode: "custom", order: ["platform", "tenant", "deviceType"] }).order).toEqual(["platform", "tenant", "deviceType"]);
  });
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
