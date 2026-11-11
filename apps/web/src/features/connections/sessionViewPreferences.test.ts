import { beforeEach, describe, expect, it } from "vitest";
import {
  DATA_SAVER_MAX_RESOLUTION,
  loadSessionViewSettings,
  saveSessionViewSettings,
  sessionViewLaunchOptions,
  sessionViewMaxResolution,
} from "./sessionViewPreferences";

beforeEach(() => {
  window.localStorage.clear();
});

describe("session view preferences", () => {
  it("defaults to fit and the connection's own quality", () => {
    expect(loadSessionViewSettings("host-a", "vnc")).toEqual({
      displayMode: "fit",
      quality: "connection",
    });
  });

  it("keeps one preference per host and protocol", () => {
    saveSessionViewSettings("host-a", "vnc", { displayMode: "actual", quality: "data_saver" });
    saveSessionViewSettings("host-b", "vnc", { quality: "sharp" });
    saveSessionViewSettings("host-a", "rdp", { displayMode: "actual" });

    expect(loadSessionViewSettings("host-a", "vnc")).toEqual({
      displayMode: "actual",
      quality: "data_saver",
    });
    expect(loadSessionViewSettings("host-b", "vnc")).toEqual({
      displayMode: "fit",
      quality: "sharp",
    });
    expect(loadSessionViewSettings("host-a", "rdp")).toEqual({
      displayMode: "actual",
      quality: "connection",
    });
  });

  it("ignores malformed stored values", () => {
    window.localStorage.setItem(
      "piping:session-view:v1:vnc:host-bad",
      JSON.stringify({ displayMode: "stretch", quality: 42 }),
    );
    expect(loadSessionViewSettings("host-bad", "vnc")).toEqual({
      displayMode: "fit",
      quality: "connection",
    });
  });

  it("maps presets to guacd options only for graphical protocols", () => {
    expect(sessionViewLaunchOptions("vnc", "sharp")).toEqual({
      color_depth: 24,
      force_lossless: true,
    });
    expect(sessionViewLaunchOptions("rdp", "balanced")).toEqual({
      color_depth: 16,
      force_lossless: false,
    });
    expect(sessionViewLaunchOptions("vnc", "data_saver")).toEqual({
      color_depth: 8,
      force_lossless: false,
    });
    expect(sessionViewLaunchOptions("vnc", "connection")).toBeUndefined();
    expect(sessionViewLaunchOptions("ssh", "sharp")).toBeUndefined();
  });

  it("caps remote resolution only for the data-saver preset", () => {
    expect(sessionViewMaxResolution("data_saver")).toEqual(DATA_SAVER_MAX_RESOLUTION);
    expect(sessionViewMaxResolution("balanced")).toBeNull();
  });
});
