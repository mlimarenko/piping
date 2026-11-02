import { describe, expect, it } from "vitest";
import { insert, rows, update } from "../store";

describe("Owner AdminModel demo contract", () => {
  it.each(["inventory/owners", "inventory/owner_groups"])("keeps %s seed/create/update free of unsupported fields", (key) => {
    const seed = rows(key)[0];
    expect(seed).toBeDefined();
    const created = insert(key, { ...seed, id: `${key}-fixture`, name: "Fixture" });
    const edited = update(key, created.id, { description: "Updated" });
    for (const record of [seed, created, edited]) {
      for (const field of ["tags", "custom_fields", "created_at", "updated_at", "status", "slug"]) {
        expect(record).not.toHaveProperty(field);
      }
    }
    if (key === "inventory/owner_groups") {
      expect(edited).not.toHaveProperty("users");
      expect(edited).not.toHaveProperty("user_groups");
      expect(edited).not.toHaveProperty("group_id");
    }
  });
});
