import { beforeEach, describe, expect, it } from "vitest";
import type { Branding } from "@/shared/api/branding-api";
import { applyBranding } from "./branding";
import { derivePalette, DEFAULT_BASE, hslToHex, PALETTE_PRESETS } from "./palette";

function branding(overrides: Partial<Branding> = {}): Branding {
  return {
    app_name: "Piping",
    logo_data_url: null,
    favicon_data_url: null,
    theme: {},
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function faviconHref(): string {
  const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  expect(link).not.toBeNull();
  return link!.href;
}

function decodeSvgDataUrl(href: string): string {
  expect(href).toMatch(/^data:image\/svg\+xml,/);
  return decodeURIComponent(href.replace(/^data:image\/svg\+xml,/, ""));
}

function opaqueTheme(theme: unknown): Branding["theme"] {
  return theme as Branding["theme"];
}

function expectGeneratedFavicon(fill = hslToHex(DEFAULT_BASE)) {
  const svg = new DOMParser().parseFromString(decodeSvgDataUrl(faviconHref()), "image/svg+xml");
  expect(svg.documentElement.getAttribute("viewBox")).toBe("0 0 32 32");
  expect(svg.querySelector("svg > rect, g")).toBeNull();
  const circles = svg.querySelectorAll("svg > circle");
  expect(circles).toHaveLength(2);
  for (const [index, circle] of Array.from(circles).entries()) {
    expect(circle.getAttribute("fill")).toBe(fill);
    const geometry = index === 0 ? ["12", "20", "12"] : ["20", "12", "9"];
    expect(["cx", "cy", "r"].map(name => circle.getAttribute(name))).toEqual(geometry);
    const maskId = circle.getAttribute("mask")?.match(/^url\(#(.+)\)$/)?.[1];
    const mask = svg.getElementById(maskId ?? "");
    expect(mask?.querySelector('circle[fill="white"]')?.getAttribute("r")).toBe(geometry[2]);
    expect(mask?.querySelector('circle[fill="black"]')?.getAttribute("r")).toBe(index === 0 ? "9" : "12");
  }
  expect(circles[0]?.getAttribute("opacity")).toBeNull();
  expect(Number(circles[1]?.getAttribute("opacity"))).toBe(0.9);
}

describe("applyBranding favicon", () => {
  beforeEach(() => {
    document.head.innerHTML = '<link rel="icon" href="/favicon.svg" type="image/svg+xml" />';
    document.body.innerHTML = "";
  });

  it("generates a stock favicon data URL instead of reusing the cacheable static path", () => {
    const stockPrimary = getComputedStyle(document.documentElement).getPropertyValue("--primary").trim();
    applyBranding(branding());

    expectGeneratedFavicon();
    expect(faviconHref()).not.toContain("/favicon.svg");
    expect(getComputedStyle(document.documentElement).getPropertyValue("--primary").trim()).toBe(stockPrimary);
  });

  it("also refreshes to the stock favicon before branding has loaded", () => {
    applyBranding(null);

    expectGeneratedFavicon();
  });

  it("keeps an uploaded custom favicon as the highest-priority source", () => {
    const customFavicon = "data:image/png;base64,ZmF2aWNvbg==";

    applyBranding(branding({ favicon_data_url: customFavicon, theme: opaqueTheme({ mode: "custom", base: PALETTE_PRESETS[3]!.base }) }));

    expect(faviconHref()).toBe(customFavicon);
  });

  it.each(PALETTE_PRESETS)("colors the generated favicon with the $id accent", ({id, base}) => {
    applyBranding(
      branding({
        theme: opaqueTheme({
          mode: "preset",
          presetId: id,
          base,
        }),
      }),
    );

    expectGeneratedFavicon(hslToHex(base));
    expect(getComputedStyle(document.documentElement).getPropertyValue("--primary").trim()).toBe(derivePalette(base).light["--primary"]);
    expect(getComputedStyle(document.documentElement).getPropertyValue("--primary-fill").trim()).toBe(derivePalette(base).light["--primary-fill"]);
  });

  it("updates the generated favicon when the custom accent changes and resets", () => {
    const stockPrimary = getComputedStyle(document.documentElement).getPropertyValue("--primary").trim();
    const base = { h: 240, s: 100, l: 50 };
    applyBranding(branding({ theme: opaqueTheme({ mode: "custom", base }) }));

    expectGeneratedFavicon("#0000ff");
    expect(getComputedStyle(document.documentElement).getPropertyValue("--primary").trim()).toBe(derivePalette(base).light["--primary"]);

    const blueHref = faviconHref();
    applyBranding(branding({ theme: opaqueTheme({ mode: "custom", base: { h: 0, s: 100, l: 50 } }) }));
    expectGeneratedFavicon("#ff0000");
    expect(faviconHref()).not.toBe(blueHref);

    document.head.innerHTML = '<link rel="icon" href="/favicon.svg" type="image/svg+xml" />';
    applyBranding(branding({ theme: opaqueTheme({ mode: "custom", base }) }));
    expect(faviconHref()).toBe(blueHref);

    applyBranding(branding({ theme: opaqueTheme({ mode: "default", base }) }));

    expectGeneratedFavicon();
    expect(getComputedStyle(document.documentElement).getPropertyValue("--primary").trim()).toBe(stockPrimary);
  });

  it("restores the generated accent icon when an uploaded favicon is cleared", () => {
    const theme = opaqueTheme({ mode: "custom", base: { h: 240, s: 100, l: 50 } });
    const favicon = "data:image/png;base64,ZmF2aWNvbg==";
    applyBranding(branding({ theme, favicon_data_url: favicon }));
    expect(faviconHref()).toBe(favicon);

    applyBranding(branding({ theme, favicon_data_url: null }));
    expectGeneratedFavicon("#0000ff");
  });
});
