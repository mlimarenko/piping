import type { EntityIconName } from "@/shared/entity-icon-utils";

export interface MapMarkerCompositionItem {
  value: string;
  label: string;
  color: string;
  count: number;
  iconName: EntityIconName;
}
