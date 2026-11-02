import { describe, it, expect } from "vitest";
import {
  derivePalette,
  hexToHsl,
  hslToHex,
  DEFAULT_BASE,
  PALETTE_PRESETS,
  applyPalette,
} from "./palette";

describe("derivePalette", () => {
  it("emits valid HSL-channel triplets for both themes", () => {
    const { dark, light } = derivePalette({ h: 211, s: 90, l: 54 });
    const triplet = /^\d{1,3} \d{1,3}% \d{1,3}%$/;
    expect(dark["--primary"]).toMatch(triplet);
    expect(light["--primary"]).toMatch(triplet);
    // accent-dim carries an alpha suffix
    expect(dark["--accent-dim"]).toMatch(/^\d{1,3} \d{1,3}% \d{1,3}% \/ 0\.12$/);
  });

  it("separates readable light-theme ink from the brand fill", () => {
    const { dark, light } = derivePalette({ h: 211, s: 90, l: 54 });
    const lDark = Number(dark["--primary"]!.split(" ")[2]!.replace("%", ""));
    const lLight = Number(light["--primary"]!.split(" ")[2]!.replace("%", ""));
    expect(lLight).toBeLessThan(lDark);
    expect(light["--primary-fill"]).not.toBe(light["--primary"]);
  });

  it("clamps dark-theme lightness into a legible band", () => {
    const veryDark = derivePalette({ h: 262, s: 83, l: 8 });
    const l = Number(veryDark.dark["--primary"]!.split(" ")[2]!.replace("%", ""));
    expect(l).toBeGreaterThanOrEqual(46);
  });

  it("picks dark ink on bright accents and light ink on dark accents", () => {
    const bright = derivePalette({ h: 52, s: 95, l: 62 }); // yellow → dark ink
    const deep = derivePalette({ h: 230, s: 80, l: 30 }); // deep blue → light ink
    expect(bright.dark["--primary-foreground"]).toBe("218 28% 9%");
    expect(deep.light["--primary-foreground"]).toBe("0 0% 100%");
  });

  it("uses amber as the default without recoloring surfaces or semantic statuses", () => {
    const { dark, light } = derivePalette(DEFAULT_BASE);
    expect(Number(dark["--primary"]!.split(" ")[0])).toBe(DEFAULT_BASE.h);
    expect(Number(dark["--primary"]!.split(" ")[0])).toBe(42);
    expect(Number(light["--primary-fill"]!.split(" ")[0])).toBe(42);
    for (const tokens of [dark, light]) {
      expect(tokens["--accent"]).not.toBe(tokens["--primary"]);
      expect(tokens).not.toHaveProperty("--background");
      expect(tokens).not.toHaveProperty("--status-negative");
    }
  });

  it.each(PALETTE_PRESETS)("keeps $id actions and selection labels at AA contrast", ({ base }) => {
    for (const tokens of Object.values(derivePalette(base))) {
      for (const [foreground, background] of [
        ["--primary-foreground", "--primary-fill"],
        ["--accent-foreground", "--accent"],
        ["--accent-foreground", "--accent-hover"],
        ["--accent-foreground", "--accent-active"],
        ["--sidebar-accent-foreground", "--sidebar-accent"],
      ]) {
        expect(contrastRatio(tokens[foreground!], tokens[background!]), `${foreground} on ${background}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
});

describe("paletteToCss", () => {
  it("applies the appropriate accent when the document switches between light and dark", () => {
    const palette = derivePalette(DEFAULT_BASE);
    applyPalette(palette);
    document.documentElement.classList.remove("dark");
    expect(getComputedStyle(document.documentElement).getPropertyValue("--primary").trim()).toBe(palette.light["--primary"]);
    document.documentElement.classList.add("dark");
    expect(getComputedStyle(document.documentElement).getPropertyValue("--primary").trim()).toBe(palette.dark["--primary"]);
    document.documentElement.classList.remove("dark");
    applyPalette(null);
  });

  it("replaces the active brand override and removes it when reset", () => {
    applyPalette(derivePalette(DEFAULT_BASE));
    const style = document.getElementById("wl-palette-overrides");
    expect(style).not.toBeNull();
    const blue = derivePalette({ h: 211, s: 90, l: 54 });
    applyPalette(blue);
    expect(document.getElementById("wl-palette-overrides")).toBe(style);
    expect(getComputedStyle(document.documentElement).getPropertyValue("--primary").trim()).toBe(blue.light["--primary"]);
    applyPalette(null);
    expect(document.getElementById("wl-palette-overrides")).toBeNull();
  });
});

function contrastRatio(first: string, second: string): number {
  const luminance = (token: string) => {
    const [h = 0, s = 0, l = 0] = token.match(/[\d.]+/g)!.map(Number);
    const hex = hslToHex({ h, s, l });
    const rgb = [1, 3, 5].map((offset) => parseInt(hex.slice(offset, offset + 2), 16) / 255);
    const linear = rgb.map((value) => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4);
    return linear[0]! * 0.2126 + linear[1]! * 0.7152 + linear[2]! * 0.0722;
  };
  const a = luminance(first);
  const b = luminance(second);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

describe("hex ⇄ hsl", () => {
  it("round-trips a known color within rounding tolerance", () => {
    const hsl = hexToHsl("#3b82f6");
    expect(hsl).not.toBeNull();
    const back = hexToHsl(hslToHex(hsl!));
    expect(back).not.toBeNull();
    expect(Math.abs(back!.h - hsl!.h)).toBeLessThanOrEqual(2);
    expect(Math.abs(back!.l - hsl!.l)).toBeLessThanOrEqual(2);
  });

  it("accepts shorthand and rejects malformed input", () => {
    expect(hexToHsl("#fff")).toEqual({ h: 0, s: 0, l: 100 });
    expect(hexToHsl("nope")).toBeNull();
    expect(hexToHsl("#12")).toBeNull();
  });
});

describe("presets", () => {
  it("all presets derive without throwing and include amber", () => {
    expect(PALETTE_PRESETS.some((p) => p.id === "amber")).toBe(true);
    for (const p of PALETTE_PRESETS) {
      expect(() => derivePalette(p.base)).not.toThrow();
    }
  });
});
