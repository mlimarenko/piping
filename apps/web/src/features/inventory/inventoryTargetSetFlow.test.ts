import { beforeEach, describe, expect, it } from "vitest";
import { inventoryFilterTargetSelector, parseInventoryTargetSetSnapshot } from "./inventoryTargetSetFlow";
import { consumeTargetSetReturnFlow, readTargetSetInitialSelector, readTargetSetReturnFlow, startTargetSetReturnFlow, updateTargetSetReturnFlowTarget } from "@/features/playbooks/targetSetReturnFlow";
import { isTargetSetReturnHref } from "@/features/playbooks/playbookEditorRoutes";

beforeEach(() => { sessionStorage.clear(); });

describe("inventory target-set save-as", () => {
  it("maps supported inventory IDs without adding status or address restrictions", () => {
    const site = "11111111-1111-4111-8111-111111111111";
    expect(inventoryFilterTargetSelector("Devices", "", { site_id: [site], status: ["active"] })).toEqual({
      target_kind: ["connection_target"], source_kind: ["device"], site_ids: [site], status: ["active"],
    });
    expect(inventoryFilterTargetSelector("Virtual Machines", "", {})).toMatchObject({ source_kind: ["virtual_machine"] });
  });

  it("refuses filters or searches that would change meaning instead of dropping them", () => {
    expect(inventoryFilterTargetSelector("Devices", "serial-or-host-name", {})).toBeNull();
    expect(inventoryFilterTargetSelector("Devices", "", { rack_id: ["rack"] })).toBeNull();
    expect(inventoryFilterTargetSelector("Sites", "", {})).toBeNull();
  });

  it("preserves the source filter, sort, page and URL through creation, then consumes the reference", () => {
    const draft = { entity: "Devices", search: "", columnFilters: { siteId: ["site"] }, page: 2, sort: { key: "name", dir: "desc" } };
    const selector = { site_ids: ["site"] };
    const href = "/inventory/resources?entity=Devices&category=devices";
    const state = startTargetSetReturnFlow(href, draft, { initialSelector: selector });
    expect(readTargetSetInitialSelector(state)).toEqual(selector);
    const returned = updateTargetSetReturnFlowTarget(state, "created-id");
    expect(returned?.returnHref).toBe(href);
    expect(readTargetSetReturnFlow(returned?.state, parseInventoryTargetSetSnapshot)).toEqual({ returnHref: href, targetSetId: "created-id", draft });
    consumeTargetSetReturnFlow(returned?.state);
    expect(readTargetSetReturnFlow(state, parseInventoryTargetSetSnapshot)).toBeNull();
    expect(readTargetSetInitialSelector(state)).toBeUndefined();
  });

  it.each(["//foreign.example/playbooks", "/inventory/resources/../admin", "/inventory/%72esources", "/playbooks/../../admin", "https://foreign.example/inventory/resources"])("rejects foreign or out-of-scope return destinations: %s", href => {
    expect(isTargetSetReturnHref(href)).toBe(false);
    expect(() => startTargetSetReturnFlow(href, {})).toThrow();
  });
});
