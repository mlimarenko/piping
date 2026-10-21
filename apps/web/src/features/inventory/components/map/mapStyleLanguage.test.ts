import { describe, expect, it } from "vitest";
import {
  localizeMapStyleLabels,
  localizedNameExpression,
  mapLabelLanguageForI18n,
} from "@/features/inventory/components/map/mapStyleLanguage";
import type { StyleSpecification } from "maplibre-gl";

function textFieldAt(style: StyleSpecification, index: number): unknown {
  const layer = style.layers[index];
  if (!layer || layer.type !== "symbol") return undefined;
  return layer.layout?.["text-field"];
}

describe("mapStyleLanguage", () => {
  it("maps interface language to map label language", () => {
    expect(mapLabelLanguageForI18n("ru")).toBe("ru");
    expect(mapLabelLanguageForI18n("ru-RU")).toBe("ru");
    expect(mapLabelLanguageForI18n("en")).toBe("en");
    expect(mapLabelLanguageForI18n(undefined)).toBe("en");
  });

  it("prefers the requested localized name field with fallbacks", () => {
    expect(localizedNameExpression("ru")).toEqual([
      "coalesce",
      ["get", "name:ru"],
      ["get", "name"],
      ["get", "name:en"],
    ]);
    expect(localizedNameExpression("en")).toEqual([
      "coalesce",
      ["get", "name:en"],
      ["get", "name"],
      ["get", "name:ru"],
    ]);
  });

  it("rewrites only symbol labels that reference map name fields", () => {
    const style = {
      version: 8,
      sources: {},
      layers: [
        {
          id: "country-labels",
          type: "symbol",
          source: "basemap",
          layout: { "text-field": ["coalesce", ["get", "name:ru"], ["get", "name"]] },
        },
        {
          id: "object-counts",
          type: "symbol",
          source: "objects",
          layout: { "text-field": ["to-string", ["get", "count"]] },
        },
      ],
    } satisfies StyleSpecification;

    const localized = localizeMapStyleLabels(style, "en");
    expect(textFieldAt(localized, 0)).toEqual(localizedNameExpression("en"));
    expect(textFieldAt(localized, 1)).toEqual(["to-string", ["get", "count"]]);
  });
});
