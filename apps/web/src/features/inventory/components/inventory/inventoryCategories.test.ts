import { describe, expect, it } from "vitest";
import en from "@/shared/i18n/locales/en";
import ru from "@/shared/i18n/locales/ru";
import { tabConfig } from "./inventoryTypes";
import {
  CABLING_WORKSTATION_RAIL_KEY,
  inventoryCategories,
  isCablingWorkstationRailKey,
  isPseudoInventoryRailKey,
} from "./inventoryCategories";

describe("inventoryCategories cabling workstation rail", () => {
  it("places the cabling workstation in circuits before Cables", () => {
    const circuits = inventoryCategories.find((category) => category.key === "circuits");
    expect(circuits).toBeDefined();
    expect(circuits?.entities.indexOf(CABLING_WORKSTATION_RAIL_KEY)).toBeGreaterThanOrEqual(0);
    expect(circuits?.entities.indexOf(CABLING_WORKSTATION_RAIL_KEY)).toBeLessThan(
      circuits?.entities.indexOf("Cables") ?? Number.POSITIVE_INFINITY,
    );
    expect(isCablingWorkstationRailKey(CABLING_WORKSTATION_RAIL_KEY)).toBe(true);
    expect(isPseudoInventoryRailKey(CABLING_WORKSTATION_RAIL_KEY)).toBe(true);
  });
});

describe("inventory entity explanations", () => {
  it.each(Object.entries({ en, ru }))("covers every catalog and workspace entry in %s", (_language, locale) => {
    const entities = new Set([
      ...tabConfig.map(tab => tab.key),
      ...inventoryCategories.flatMap(category => category.entities),
    ]);
    const hints: Record<string, string> = locale.inventory.resources.hints.entities;
    expect(Object.keys(hints).sort()).toEqual([...entities].sort());
    for (const entity of entities) {
      expect(hints[entity], entity).toEqual(expect.any(String));
      expect(hints[entity]?.trim(), entity).not.toBe("");
      expect(hints[entity], entity).not.toBe(entity);
    }
  });
});
