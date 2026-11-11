import { emptySelectorModel, selectorToModel, type SelectorFacet, type SelectorModel } from "./selectorModel";
import type { TargetSetMode } from "./playbookFormModels";

/**
 * Starting points offered by the target-set list's empty state. Each one seeds
 * the canonical editor; nothing is created until the editor is saved.
 */
export interface TargetSetTemplate {
  key: string;
  mode: TargetSetMode;
  selector?: SelectorModel;
  requiredFacet?: SelectorFacet;
}

function allHostsSelector(): SelectorModel {
  const model = emptySelectorModel();
  return { ...model, facets: { ...model.facets, status: [] }, addressBound: false };
}

export const TARGET_SET_TEMPLATES: TargetSetTemplate[] = [
  { key: "activeAddressable", mode: "dynamic", selector: emptySelectorModel() },
  { key: "allHosts", mode: "dynamic", selector: allHostsSelector() },
  { key: "staticHosts", mode: "static" },
  { key: "activeSsh", mode: "dynamic", selector: selectorToModel({ status: ["active"], runtime: ["ansible"], connection_protocol: ["ssh"], address_bound: true }) },
  { key: "linuxVm", mode: "dynamic", requiredFacet: "platform", selector: selectorToModel({ source_kind: ["virtual_machine"], runtime: ["ansible"], address_bound: true }) },
  ...(["role", "region", "tenant", "tag"] as const).map(facet => ({ key: facet, mode: "dynamic" as const, requiredFacet: facet, selector: emptySelectorModel() })),
];

export function targetSetTemplate(key: string | null | undefined): TargetSetTemplate | undefined {
  return TARGET_SET_TEMPLATES.find((template) => template.key === key);
}
