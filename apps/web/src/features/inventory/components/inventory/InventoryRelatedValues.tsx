import { useMemo } from "react";
import { Link } from "react-router";
import type { InventoryEntityKey } from "@/shared/api/types";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityOptionLabel from "@/shared/ui-shared/EntityOptionLabel";
import EntityRef from "@/shared/ui-shared/EntityRef";
import { inventoryResourceDetailHref } from "@/features/inventory/resourceLinks";
import { inventoryResourceDisplayName } from "@/features/inventory/resourceOptions";
import { inventoryRelationKey, useInventoryRelations } from "@/features/inventory/relationLabels";

/** Resolve only through the authorized catalog; unknown IDs remain visible as an explicit unknown reference. */
export default function InventoryRelatedValues({ entity, ids }: { entity: InventoryEntityKey; ids: string[] }) {
  const { can } = usePermissions();
  const refs = useMemo(() => ids.map(id => ({ entity, id })), [entity, ids]);
  const resources = useInventoryRelations(refs, ids.length > 0 && can("READ", "INVENTORY"));
  if (!ids.length) return <span>—</span>;
  return <span className="flex flex-wrap gap-2">{ids.map(id => {
    const href = inventoryResourceDetailHref(entity, id);
    const resource = resources.get(inventoryRelationKey(entity, id));
    const label = resource
      ? <EntityOptionLabel label={inventoryResourceDisplayName(resource)} color={entity === "Tags" ? resource.color : undefined} />
      : <EntityRef id={id} />;
    return href ? <Link key={id} to={href} className="min-w-0 text-primary underline">{label}</Link> : <span key={id}>{label}</span>;
  })}</span>;
}
