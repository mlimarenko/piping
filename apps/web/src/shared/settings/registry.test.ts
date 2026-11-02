import { describe, expect, it } from "vitest";
import {
  normalizeConnectionsOpenInNewTabSetting,
  normalizeTablePageSize,
  shouldOpenConnectionInNewTab,
  TABLE_PAGE_SIZE_OPTIONS,
} from "./registry";

describe("settings registry", () => {
  it("keeps the table page-size normalizer aligned with selectable options", () => {
    expect(TABLE_PAGE_SIZE_OPTIONS).toEqual([50, 100, 250, 500, 1000]);
    expect(normalizeTablePageSize(1000)).toBe(1000);
    expect(normalizeTablePageSize("1000")).toBe(1000);
    expect(normalizeTablePageSize(1500)).toBe(1000);
    expect(normalizeTablePageSize(1)).toBe(10);
  });

  it("normalizes open-in-new-tab settings from bare-boolean and partial values", () => {
    expect(normalizeConnectionsOpenInNewTabSetting(true)).toEqual({
      enabled: true,
      protocols: { ssh: false, rdp: false, vnc: false },
    });
    expect(
      normalizeConnectionsOpenInNewTabSetting({
        enabled: false,
        protocols: { ssh: "yes", rdp: false },
      }),
    ).toEqual({
      enabled: false,
      protocols: { ssh: true, rdp: false, vnc: false },
    });
  });

  it("resolves open-in-new-tab globally or per protocol", () => {
    const perProtocol = normalizeConnectionsOpenInNewTabSetting({
      enabled: false,
      protocols: { ssh: true, rdp: false, vnc: true },
    });
    expect(shouldOpenConnectionInNewTab(perProtocol, "ssh")).toBe(true);
    expect(shouldOpenConnectionInNewTab(perProtocol, "rdp")).toBe(false);
    expect(shouldOpenConnectionInNewTab(perProtocol, "vnc")).toBe(true);

    const global = normalizeConnectionsOpenInNewTabSetting({
      enabled: true,
      protocols: { ssh: false, rdp: false, vnc: false },
    });
    expect(shouldOpenConnectionInNewTab(global, "ssh")).toBe(true);
    expect(shouldOpenConnectionInNewTab(global, "rdp")).toBe(true);
    expect(shouldOpenConnectionInNewTab(global, "vnc")).toBe(true);
  });
});
