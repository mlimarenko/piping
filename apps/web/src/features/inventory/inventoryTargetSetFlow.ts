import { z } from "zod";

const snapshotSchema = z.object({
  entity: z.enum(["Devices", "Virtual Machines"]),
  search: z.string(),
  columnFilters: z.record(z.string(), z.array(z.string())),
  page: z.number().int().nonnegative(),
  sort: z.object({ key: z.string(), dir: z.enum(["asc", "desc"]) }).nullable(),
});
export type InventoryTargetSetSnapshot = z.infer<typeof snapshotSchema>;

export function parseInventoryTargetSetSnapshot(value: unknown): InventoryTargetSetSnapshot | null {
  const parsed = snapshotSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
}

const selectorKeys: Record<string, string> = {
  site_id: "site_ids", region_id: "region_ids", tenant_id: "tenant_ids", role_id: "role_ids",
  platform_id: "platform_ids", cluster_id: "cluster_ids", device_type_id: "device_type_ids", status: "status",
};

/** Unsupported conditions must not silently broaden a saved selection. */
export function inventoryFilterTargetSelector(entity: string, search: string, filters: Record<string, string[]>): Record<string, unknown> | null {
  if ((entity !== "Devices" && entity !== "Virtual Machines") || search.trim()) return null;
  const selector: Record<string, unknown> = {
    target_kind: ["connection_target"], source_kind: [entity === "Devices" ? "device" : "virtual_machine"],
  };
  for (const [key, values] of Object.entries(filters)) {
    if (!values.length) continue;
    const selectorKey = selectorKeys[key];
    if (!selectorKey) return null;
    selector[selectorKey] = values;
  }
  return selector;
}
