import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router";
import type { InventoryEntityKey } from "@/shared/api/types";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityOptionLabel from "@/shared/ui-shared/EntityOptionLabel";
import { inventoryResourceOptionsQuery } from "@/features/inventory/resourceOptions";
import { inventoryResourceDetailHref } from "@/features/inventory/resourceLinks";

/** Resolve only through the authorized catalog; unknown IDs remain visible. */
export default function InventoryRelatedValues({ entity, ids }: { entity: InventoryEntityKey; ids: string[] }) {
  const { can } = usePermissions();
  const query = useQuery({ ...inventoryResourceOptionsQuery(entity), enabled: ids.length > 0 && can("READ", "INVENTORY") });
  const byId = new Map((query.data?.items ?? []).map(item => [item.id, item]));
  if (!ids.length) return <span>—</span>;
  return <span className="flex flex-wrap gap-2">{ids.map(id => {
    const item = byId.get(id);
    const href = inventoryResourceDetailHref(entity, id);
    const label = <EntityOptionLabel label={item?.name || id} color={entity === "Tags" ? item?.color : undefined} />;
    return href ? <Link key={id} to={href} className="min-w-0 text-primary underline">{label}</Link> : <span key={id}>{label}</span>;
  })}</span>;
}
