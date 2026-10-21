import type { ExpressionSpecification, LayerSpecification, StyleSpecification } from "maplibre-gl";

export type MapLabelLanguage = "en" | "ru";

const NAME_KEYS = new Set(["name", "name:en", "name:ru"]);

export function mapLabelLanguageForI18n(language: string | undefined): MapLabelLanguage {
  return language?.toLowerCase().startsWith("ru") ? "ru" : "en";
}

export function localizedNameExpression(language: MapLabelLanguage): ExpressionSpecification {
  return language === "ru"
    ? ["coalesce", ["get", "name:ru"], ["get", "name"], ["get", "name:en"]]
    : ["coalesce", ["get", "name:en"], ["get", "name"], ["get", "name:ru"]];
}

function referencesNameProperty(value: unknown): boolean {
  if (!Array.isArray(value)) return false;
  if (value[0] === "get" && typeof value[1] === "string" && NAME_KEYS.has(value[1])) {
    return true;
  }
  return value.some(referencesNameProperty);
}

export function localizeMapStyleLabels(
  style: StyleSpecification,
  language: MapLabelLanguage,
): StyleSpecification {
  return {
    ...style,
    layers: style.layers.map((layer): LayerSpecification => {
      if (layer.type !== "symbol") return layer;
      const layout = layer.layout;
      const textField = layout?.["text-field"];
      if (!referencesNameProperty(textField)) return layer;
      return {
        ...layer,
        layout: {
          ...layout,
          "text-field": localizedNameExpression(language),
        },
      };
    }),
  };
}
