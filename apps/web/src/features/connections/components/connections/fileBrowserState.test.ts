import { afterEach, describe, expect, it, vi } from "vitest";

import {
  SSH_FILE_BROWSER_PREFERENCE_KEY,
  clampSshFileBrowserSize,
  getSshFileBrowserMaxSize,
  loadSshFileBrowserState,
  normalizeSshFileBrowserState,
  saveSshFileBrowserState,
} from "./fileBrowserState";

describe("SSH file browser persisted state", () => {
  afterEach(() => {
    window.localStorage.removeItem(SSH_FILE_BROWSER_PREFERENCE_KEY);
  });

  it("normalizes placement and per-placement sizes", () => {
    const state = normalizeSshFileBrowserState({
      placement: "right",
      sizes: { bottom: 10, right: 99999 },
    });

    expect(state.placement).toBe("right");
    expect(state.sizes.bottom).toBe(96);
    expect(state.sizes.right).toBeLessThanOrEqual(Math.floor(window.innerWidth * 0.56));
  });

  it("clamps against an available split size", () => {
    expect(clampSshFileBrowserSize("right", 600, 320)).toBe(320);
    expect(clampSshFileBrowserSize("bottom", 10, 500)).toBe(96);
    expect(getSshFileBrowserMaxSize("right", 320)).toBe(320);
  });

  it("saves and loads local storage preferences", () => {
    window.localStorage.removeItem(SSH_FILE_BROWSER_PREFERENCE_KEY);

    saveSshFileBrowserState({
      placement: "bottom",
      sizes: { bottom: 220, right: 360 },
    });

    expect(loadSshFileBrowserState()).toEqual({
      placement: "bottom",
      sizes: { bottom: 220, right: 360 },
    });
  });

  it("keeps UI state updates best-effort when local storage rejects writes", () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("storage blocked");
    });

    expect(() => {
      saveSshFileBrowserState({
        placement: "right",
        sizes: { bottom: 180, right: 360 },
      });
    }).not.toThrow();

    setItem.mockRestore();
  });
});
