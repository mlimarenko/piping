import { describe, expect, it } from "vitest";
import {
  inventoryInspectorFields,
  inventoryInspectorHasStatus,
} from "./inventoryInspectorFields";

describe("inventoryInspectorFields", () => {
  it("derives Platform metadata: manufacturer (related) + napalmDriver + description, no comments", () => {
    const fields = inventoryInspectorFields("Platforms");
    const manufacturer = fields.find((f) => f.key === "manufacturerName");
    expect(manufacturer).toMatchObject({ kind: "related", relatedTab: "Manufacturers", idKey: "manufacturerId" });
    expect(fields.map((f) => f.key)).toEqual(["manufacturerName", "napalmDriver", "description"]);
    // Platform has no comments field.
    expect(fields.some((f) => f.key === "comments")).toBe(false);
  });

  it("derives Device Types metadata: manufacturer (related) + description + comments, no model row", () => {
    const fields = inventoryInspectorFields("Device Types");
    expect(fields.map((f) => f.key)).toEqual(["manufacturerName", "description", "comments"]);
    expect(fields.find((f) => f.key === "manufacturerName")).toMatchObject({
      kind: "related",
      relatedTab: "Manufacturers",
      idKey: "manufacturerId",
    });
    // Long-text fields are textarea kind (ExpandableTextRow), never scalar links.
    expect(fields.find((f) => f.key === "description")?.kind).toBe("textarea");
    expect(fields.find((f) => f.key === "comments")?.kind).toBe("textarea");
    // The model is shown via the header, not a duplicated inspector row.
    expect(fields.some((f) => f.key === "model")).toBe(false);
  });

  it("excludes icon and objectRef fields from inspector rows", () => {
    // Device Roles carry an icon field; it must not surface as an inspector row.
    const fields = inventoryInspectorFields("Device Roles");
    expect(fields.some((f) => f.key === "iconName")).toBe(false);
    expect(fields.find((f) => f.key === "color")?.kind).toBe("scalar");
  });

  it("reports status capability per entity (gates the inspector status row)", () => {
    expect(inventoryInspectorHasStatus("Sites")).toBe(true);
    expect(inventoryInspectorHasStatus("Clusters")).toBe(true);
    expect(inventoryInspectorHasStatus("Regions")).toBe(false);
    expect(inventoryInspectorHasStatus("Tenants")).toBe(false);
  });
});
