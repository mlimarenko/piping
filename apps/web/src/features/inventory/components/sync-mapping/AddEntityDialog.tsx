import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Plus, RefreshCw, Loader2 } from "lucide-react";
import { notify } from "@/shared/lib/notifications";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/shared/ui-primitives/dialog";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { cn } from "@/shared/lib/utils";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import type { EntityMapping, MappingTarget } from "@/shared/api/types";
import { listLocalInventoryResources } from "@/features/inventory/sync-mapping-api";
import type { LocalInventoryResource } from "@/features/inventory/sync-mapping-api";

interface AddEntityDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  providerId: string;
  target: MappingTarget;
  onAdded: (em: EntityMapping) => boolean;
}

/**
 * Adds an entity mapping. For a `local_inventory` target the source entities
 * come from the canonical local-inventory catalogue
 * (`/sync/targets/local-inventory/resources`), never from a hardcoded or
 * provider-side registry. A remote provider target has no entity catalogue, so
 * the affordance is disabled rather than faked.
 */
const AddEntityDialog = ({
  open,
  onOpenChange,
  providerId,
  target,
  onAdded,
}: AddEntityDialogProps) => {
  const { t } = useTranslation();
  const isLocalInventory = target.kind === "local_inventory";
  const [sourceEntity, setSourceEntity] = useState("");
  const [targetEntity, setTargetEntity] = useState("");
  const [search, setSearch] = useState("");

  const catalogQuery = useQuery({
    queryKey: ["sync", "local-inventory-resources", providerId],
    queryFn: () => listLocalInventoryResources(providerId),
    enabled: open && isLocalInventory,
  });

  const filtered = useMemo(() => {
    const resources = catalogQuery.data?.resources ?? [];
    const needle = search.trim().toLowerCase();
    if (!needle) return resources;
    return resources.filter(
      (resource) =>
        resource.sourceEntity.toLowerCase().includes(needle) ||
        resource.targetEntity.toLowerCase().includes(needle),
    );
  }, [catalogQuery.data, search]);

  const reset = () => {
    setSourceEntity("");
    setTargetEntity("");
    setSearch("");
  };

  const handleAdd = () => {
    if (!sourceEntity) return;
    const em: EntityMapping = {
      id: `em-${Date.now()}`,
      source_entity: sourceEntity,
      target_entity: targetEntity || (sourceEntity.split(".").pop() ?? sourceEntity),
      enabled: true,
      field_mappings: [],
    };
    if (!onAdded(em)) return;
    notify("success", t("sync.mapping.entityAdded", "Entity {{entity}} added.", {
      entity: em.source_entity,
    }));
    reset();
    onOpenChange(false);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(v) => {
        if (!v) reset();
        onOpenChange(v);
      }}
    >
      <DialogContent className="max-w-md surface border-border" description={t("sync.mapping.addEntityDialogDescription", "Map a source entity to a Piping resource type.")}>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-sm">
            <Plus className="h-4 w-4 text-primary" />
            {t("sync.mapping.addEntityDialogTitle", "Add entity mapping")}
          </DialogTitle>
        </DialogHeader>

        {!isLocalInventory ? (
          <p className="text-xs leading-relaxed text-muted-foreground">
            {t(
              "sync.mapping.localTargetRequired",
              "Entity mapping is available only for the local inventory target.",
            )}
          </p>
        ) : (
          <div className="space-y-3">
            <p className="text-xs leading-relaxed text-muted-foreground">
              {t(
                "sync.mapping.addEntityDialogHelp",
                "Choose the object type from the local inventory catalogue. The target model is filled automatically and can be adjusted before saving.",
              )}
            </p>
            <div>
              <FieldLabel htmlFor="sync-mapping-entity-search" className="text-xs font-medium text-muted-foreground mb-1 block" required>
                {t("sync.mapping.sourceEntityLabel", "Source entity")}
              </FieldLabel>
              <Input
                id="sync-mapping-entity-search"
                className="h-9 text-sm border-border mb-1.5"
                placeholder={t("sync.mapping.searchEntityPlaceholder", "Search entity types…")}
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
              <EntityCatalog
                isPending={catalogQuery.isPending}
                isError={catalogQuery.isError}
                onRetry={() => { void catalogQuery.refetch(); }}
                filtered={filtered}
                sourceEntity={sourceEntity}
                onSelect={(resource) => {
                  setSourceEntity(resource.sourceEntity);
                  setTargetEntity(resource.targetEntity);
                }}
              />
            </div>

            <div>
              <FieldLabel htmlFor="sync-mapping-target-entity" className="text-xs font-medium text-muted-foreground mb-1 block">
                {t("sync.mapping.targetEntityLabel", "Target entity (Piping model)")}
              </FieldLabel>
              <Input
                id="sync-mapping-target-entity"
                className="h-9 text-sm border-border font-mono"
                value={targetEntity}
                onChange={(e) => setTargetEntity(e.target.value)}
              />
            </div>
          </div>
        )}

        <DialogFooter>
          <Button
            variant="outline"
            size="sm"
            className="h-9 text-sm"
            onClick={() => {
              reset();
              onOpenChange(false);
            }}
          >
            {t("cancel")}
          </Button>
          {isLocalInventory && (
            <Button
              size="sm"
              className="h-9 text-sm gap-1"
              disabled={!sourceEntity}
              onClick={handleAdd}
            >
              <Plus className="h-3 w-3" /> {t("add")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

/**
 * Renders the local-inventory catalogue: pending, error+retry, or the filtered
 * entity list. Kept as its own component so the dialog render holds no nested
 * conditional chain (Sonar S3358).
 */
const EntityCatalog = ({
  isPending,
  isError,
  onRetry,
  filtered,
  sourceEntity,
  onSelect,
}: {
  isPending: boolean;
  isError: boolean;
  onRetry: () => void;
  filtered: LocalInventoryResource[];
  sourceEntity: string;
  onSelect: (resource: LocalInventoryResource) => void;
}) => {
  const { t } = useTranslation();
  if (isPending) {
    return (
      <p className="flex items-center justify-center gap-2 py-3 text-xs text-muted-foreground">
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
        {t("inventory.sync.mappingConfigurationsLoading")}
      </p>
    );
  }
  if (isError) {
    return (
      <div className="flex flex-col items-center gap-2 py-3">
        <p className="text-xs text-destructive">
          {t("sync.mapping.catalogLoadFailed", "Could not load the local inventory catalogue.")}
        </p>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-9 gap-1.5 text-sm"
          onClick={onRetry}
        >
          <RefreshCw className="h-3 w-3" />
          {t("retry")}
        </Button>
      </div>
    );
  }
  return (
    <div className="max-h-48 overflow-y-auto rounded-md border border-border bg-card">
      {filtered.length === 0 && (
        <p className="text-xs text-muted-foreground text-center py-3">
          {t("sync.mapping.noEntityTypesFound", "No entity types found.")}
        </p>
      )}
      {filtered.map((resource) => (
        <Button variant="ghost"
          key={resource.sourceEntity}
          type="button"
          className={cn(
            "h-auto min-h-9 w-full flex-wrap justify-start whitespace-normal text-left px-3 py-2 text-sm hover:bg-accent transition-colors",
            sourceEntity === resource.sourceEntity && "bg-accent font-medium text-foreground",
            sourceEntity !== resource.sourceEntity && "text-muted-foreground",
          )}
          onClick={() => onSelect(resource)}
        >
          <span className="font-mono">{resource.sourceEntity}</span>
          <span className="ml-1 text-muted-foreground">→ {resource.targetEntity}</span>
        </Button>
      ))}
    </div>
  );
};

export default AddEntityDialog;
