import { useState, useRef, useCallback, useEffect } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  Plus,
  Layers,
  ArrowLeftRight,
  ArrowRight,
  AlertTriangle,
  Circle,
  Menu,
} from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Sheet, SheetContent, SheetTrigger, SheetTitle, SheetDescription } from "@/shared/ui-primitives/sheet";
import { cn } from "@/shared/lib/utils";
import type { EntityMapping, FieldMapping } from "@/shared/api/types";

interface SelectedField {
  entityId: string;
  fieldId: string;
}

interface FieldTreeRailProps {
  entityMappings: EntityMapping[];
  selectedField: SelectedField | null;
  selectedEntityId?: string | null;
  onSelectField: (sel: SelectedField) => void;
  onSelectEntity?: (entityId: string) => void;
  onAddEntity: () => void;
  onAddField?: (entityId: string) => void;
  readonly?: boolean;
}

type FieldStatus = "bidirectional" | "forward_only" | "invalid" | "unconfigured";

function getFieldStatus(fm: FieldMapping): FieldStatus {
  if (!fm.enabled) return "unconfigured";
  if (!fm.forward_transform) return "unconfigured";
  if (fm.reverse_transform !== null) return "bidirectional";
  return "forward_only";
}

const STATUS_LABEL_KEY: Record<FieldStatus, string> = {
  bidirectional: "sync.mapping.fieldStatus.bidirectional",
  forward_only: "sync.mapping.fieldStatus.forwardOnly",
  invalid: "sync.mapping.fieldStatus.hasValidationErrors",
  unconfigured: "sync.mapping.fieldStatus.notConfigured",
};

const useStatusLabel = (status: FieldStatus) => {
  const { t } = useTranslation();
  return t(STATUS_LABEL_KEY[status]);
};

/**
 * Status dot — purely decorative color reinforcement. The adjacent
 * {@link FieldStatusIcon} carries the distinct shape and the translated
 * accessible name, so status is never conveyed by color alone and the dot is
 * marked aria-hidden to avoid a duplicate announcement.
 */
const StatusDot = ({ status }: { status: FieldStatus }) => {
  const colorClass =
    status === "bidirectional"
      ? "bg-status-positive"
      : status === "forward_only"
        ? "bg-status-warning"
        : status === "invalid"
          ? "bg-status-negative"
          : "bg-muted-foreground/30";
  return (
    <span
      aria-hidden="true"
      className={cn("inline-block w-2 h-2 rounded-full shrink-0", colorClass)}
    />
  );
};

/** Distinct per-status shape, so status is never color-only. */
const FieldStatusIcon = ({ status }: { status: FieldStatus }) => {
  const label = useStatusLabel(status);
  if (status === "bidirectional") {
    return (
      <ArrowLeftRight className="h-3 w-3 text-status-positive shrink-0" aria-label={label} />
    );
  }
  if (status === "forward_only") {
    return <ArrowRight className="h-3 w-3 text-status-warning shrink-0" aria-label={label} />;
  }
  if (status === "invalid") {
    return (
      <AlertTriangle className="h-3 w-3 text-status-negative shrink-0" aria-label={label} />
    );
  }
  return (
    <Circle className="h-3 w-3 text-muted-foreground shrink-0" aria-label={label} />
  );
};

// ── Flat navigable item list for keyboard nav ─────────────────────────────────

interface TreeItem {
  type: "entity" | "field";
  entityId: string;
  fieldId?: string;
  entityIndex: number;
  fieldIndex?: number;
  fieldTotal?: number;
  entityTotal: number;
}

function buildFlatItems(
  entityMappings: EntityMapping[],
  expandedEntities: Set<string>,
): TreeItem[] {
  const items: TreeItem[] = [];
  const entityTotal = entityMappings.length;
  entityMappings.forEach((em, ei) => {
    items.push({
      type: "entity",
      entityId: em.id,
      entityIndex: ei,
      entityTotal,
    });
    if (expandedEntities.has(em.id)) {
      const fieldTotal = em.field_mappings.length;
      em.field_mappings.forEach((fm, fi) => {
        items.push({
          type: "field",
          entityId: em.id,
          fieldId: fm.id,
          entityIndex: ei,
          fieldIndex: fi,
          fieldTotal,
          entityTotal,
        });
      });
    }
  });
  return items;
}

// ── Tree content (used in both desktop rail and mobile sheet) ─────────────────

interface TreeContentProps {
  entityMappings: EntityMapping[];
  selectedField: SelectedField | null;
  selectedEntityId?: string | null;
  onSelectField: (sel: SelectedField) => void;
  onSelectEntity?: (entityId: string) => void;
  onAddEntity: () => void;
  onAddField?: (entityId: string) => void;
  readonly: boolean;
}

const TreeContent = ({
  entityMappings,
  selectedField,
  selectedEntityId,
  onSelectField,
  onSelectEntity,
  onAddEntity,
  onAddField,
  readonly,
}: TreeContentProps) => {
  const { t } = useTranslation();
  const [expandedEntities, setExpandedEntities] = useState<Set<string>>(
    () => new Set(entityMappings.map((em) => em.id)),
  );
  const treeRef = useRef<HTMLDivElement>(null);
  const [focusedKey, setFocusedKey] = useState<string | null>(null);

  const toggleEntity = useCallback((id: string) => {
    setExpandedEntities((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }, []);

  const handleSelectEntity = useCallback(
    (entityId: string) => {
      onSelectEntity?.(entityId);
      setExpandedEntities((prev) => {
        if (prev.has(entityId)) return prev;
        const next = new Set(prev);
        next.add(entityId);
        return next;
      });
    },
    [onSelectEntity],
  );

  const expandAll = useCallback(() => {
    setExpandedEntities(new Set(entityMappings.map((em) => em.id)));
  }, [entityMappings]);

  const flatItems = buildFlatItems(entityMappings, expandedEntities);

  const focusItemByKey = useCallback(
    (key: string) => {
      setFocusedKey(key);
      // Focus the DOM element with data-tree-key matching
      const el = treeRef.current?.querySelector<HTMLElement>(`[data-tree-key="${key}"]`);
      el?.focus();
    },
    [],
  );

  const getItemKey = (item: TreeItem) =>
    item.type === "entity" ? `entity:${item.entityId}` : `field:${item.entityId}:${item.fieldId}`;

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent, item: TreeItem) => {
      const key = getItemKey(item);
      const currentIdx = flatItems.findIndex((it) => getItemKey(it) === key);

      switch (e.key) {
        case "ArrowDown": {
          e.preventDefault();
          const next = flatItems[currentIdx + 1];
          if (next) focusItemByKey(getItemKey(next));
          break;
        }
        case "ArrowUp": {
          e.preventDefault();
          const prev = flatItems[currentIdx - 1];
          if (prev) focusItemByKey(getItemKey(prev));
          break;
        }
        case "ArrowRight": {
          e.preventDefault();
          if (item.type === "entity") {
            if (!expandedEntities.has(item.entityId)) {
              toggleEntity(item.entityId);
            } else {
              // Move to first child if any
              const firstChild = flatItems[currentIdx + 1];
              if (firstChild?.type === "field" && firstChild.entityId === item.entityId) {
                focusItemByKey(getItemKey(firstChild));
              }
            }
          }
          break;
        }
        case "ArrowLeft": {
          e.preventDefault();
          if (item.type === "field") {
            // Move to parent entity
            const parentKey = `entity:${item.entityId}`;
            focusItemByKey(parentKey);
          } else if (item.type === "entity" && expandedEntities.has(item.entityId)) {
            toggleEntity(item.entityId);
          }
          break;
        }
        case "Home": {
          e.preventDefault();
          const first = flatItems[0];
          if (first) focusItemByKey(getItemKey(first));
          break;
        }
        case "End": {
          e.preventDefault();
          const last = flatItems[flatItems.length - 1];
          if (last) focusItemByKey(getItemKey(last));
          break;
        }
        case "*": {
          e.preventDefault();
          expandAll();
          break;
        }
        case "Enter":
        case " ": {
          e.preventDefault();
          if (item.type === "entity") {
            handleSelectEntity(item.entityId);
          } else if (item.fieldId) {
            onSelectField({ entityId: item.entityId, fieldId: item.fieldId });
          }
          break;
        }
        default:
          break;
      }
    },
    [
      flatItems,
      expandedEntities,
      toggleEntity,
      handleSelectEntity,
      expandAll,
      focusItemByKey,
      onSelectField,
    ],
  );

  // Keep focusedKey in sync if tree changes.
  // The focused key is user-driven (keyboard navigation) and persists across
  // renders; it must be reconciled against the live item collection and cleared
  // only when its target disappears, so it cannot be purely derived.
  useEffect(() => {
    if (focusedKey) {
      const stillExists = flatItems.some((it) => getItemKey(it) === focusedKey);
      // eslint-disable-next-line react-hooks/set-state-in-effect -- focusedKey is user-driven and clears only when its item disappears; deriving it in render would silently refocus a reappearing item
      if (!stillExists) setFocusedKey(null);
    }
  }, [flatItems, focusedKey]);

  if (entityMappings.length === 0) {
    return (
      <div className="flex-1 flex flex-col">
        <h3 className="px-3 py-2 border-b border-border text-sm font-semibold text-foreground">
          {t("sync.mapping.entitiesLabel", "Entities")}
        </h3>
        <div className="flex-1 flex w-full flex-col items-center justify-center gap-3 p-4 text-center">
          <Layers className="size-9 text-muted-foreground" />
          <p className="mx-auto max-w-xs text-xs text-muted-foreground text-balance">
            {t("sync.mapping.noEntitiesYet", "No entities mapped yet.")}
          </p>
          {!readonly && (
            <Button
              size="sm"
              variant="outline"
              className="h-9 text-sm gap-1"
              onClick={onAddEntity}
            >
              <Plus className="h-3 w-3" />
              {t("sync.mapping.addEntity", "Add entity")}
            </Button>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      {/* Rail header */}
      <div className="px-3 py-2 border-b border-border flex items-center justify-between shrink-0">
        <h3 className="text-sm font-semibold text-foreground">
          {t("sync.mapping.entitiesLabel", "Entities")}
        </h3>
        {!readonly && (
          <Button
            variant="ghost"
            size="icon"
            className="size-8"
            onClick={onAddEntity}
            aria-label={t("sync.mapping.addEntity", "Add entity")}
          >
            <Plus className="h-3 w-3" />
          </Button>
        )}
      </div>

      {/* WAI-ARIA tree */}
      <div
        ref={treeRef}
        role="tree"
        aria-label={t("sync.mapping.fieldTreeLabel", "Entity and field tree")}
        className="flex-1 overflow-y-auto py-1"
      >
        {entityMappings.map((em, emIndex) => {
          const expanded = expandedEntities.has(em.id);
          const entityKey = `entity:${em.id}`;
          const entityItem: TreeItem = {
            type: "entity",
            entityId: em.id,
            entityIndex: emIndex,
            entityTotal: entityMappings.length,
          };

          return (
            <div key={em.id} role="none">
              {/* Entity group header */}
              <div className="group flex items-center">
                <Button variant="ghost"
                  type="button"
                  className="ml-2 flex size-6 shrink-0 items-center justify-center rounded p-0 hover:bg-accent"
                  tabIndex={-1}
                  aria-label={expanded ? t("collapse", "Collapse") : t("expand", "Expand")}
                  onClick={() => toggleEntity(em.id)}
                >
                  {expanded ? (
                    <ChevronDown className="size-3 text-muted-foreground" />
                  ) : (
                    <ChevronRight className="size-3 text-muted-foreground" />
                  )}
                </Button>
              <Button variant="ghost"
                type="button"
                data-tree-key={entityKey}
                role="treeitem"
                aria-expanded={expanded}
                aria-selected={selectedEntityId === em.id}
                aria-level={1}
                aria-posinset={emIndex + 1}
                aria-setsize={entityMappings.length}
                tabIndex={focusedKey === entityKey || (!focusedKey && emIndex === 0) ? 0 : -1}
                className={cn(
                  "h-9 min-w-0 flex-1 flex items-center justify-start gap-2 px-3 text-left hover:bg-accent transition-colors focus:outline-none focus-visible:ring-1 focus-visible:ring-primary/50 focus-visible:bg-accent",
                  selectedEntityId === em.id && !selectedField && "bg-accent",
                  !em.enabled && "opacity-50",
                )}
                onClick={() => handleSelectEntity(em.id)}
                onKeyDown={(e) => handleKeyDown(e, entityItem)}
                onFocus={() => setFocusedKey(entityKey)}
              >
                <span className="text-sm font-medium text-foreground truncate">
                  {em.source_entity}
                </span>
              </Button>
              {!readonly && onAddField && (
                <Button
                  variant="ghost"
                  size="icon"
                   className="mr-1 size-9 shrink-0 text-muted-foreground hover:text-foreground"
                  onClick={(event) => {
                    event.stopPropagation();
                    handleSelectEntity(em.id);
                    onAddField(em.id);
                  }}
                  aria-label={t("sync.mapping.addField", "Add field")}
                >
                  <Plus className="h-3 w-3" />
                </Button>
              )}
              </div>

              {/* Field rows */}
              {expanded && (
                <div role="group" aria-label={em.source_entity}>
                  {em.field_mappings.length === 0 && (
                    <div className="flex items-center justify-between gap-2 pl-8 pr-3 py-1 text-xs text-muted-foreground">
                      <span className="italic">
                        {t("sync.mapping.noFieldsYet", "No fields configured")}
                      </span>
                      {!readonly && onAddField && (
                        <Button variant="ghost"
                          type="button"
                          className="text-primary hover:text-primary/80"
                          onClick={() => {
                            handleSelectEntity(em.id);
                            onAddField(em.id);
                          }}
                        >
                          {t("sync.mapping.addField", "Add field")}
                        </Button>
                      )}
                    </div>
                  )}
                  {em.field_mappings.map((fm, fmIndex) => {
                    const status = getFieldStatus(fm);
                    const isSelected =
                      selectedField?.entityId === em.id && selectedField?.fieldId === fm.id;
                    const fieldKey = `field:${em.id}:${fm.id}`;
                    const fieldItem: TreeItem = {
                      type: "field",
                      entityId: em.id,
                      fieldId: fm.id,
                      entityIndex: emIndex,
                      fieldIndex: fmIndex,
                      fieldTotal: em.field_mappings.length,
                      entityTotal: entityMappings.length,
                    };

                    return (
                      <Button variant="ghost"
                        type="button"
                        key={fm.id}
                        data-tree-key={fieldKey}
                        role="treeitem"
                        aria-selected={isSelected}
                        aria-level={2}
                        aria-posinset={fmIndex + 1}
                        aria-setsize={em.field_mappings.length}
                        tabIndex={focusedKey === fieldKey ? 0 : -1}
                        className={cn(
                          "h-9 w-full flex items-center justify-start gap-2 pl-8 pr-3 text-left hover:bg-accent transition-colors focus:outline-none focus-visible:ring-1 focus-visible:ring-primary/50 focus-visible:bg-accent",
                          isSelected && "bg-accent",
                          !fm.enabled && "opacity-50",
                        )}
                        onClick={() => onSelectField({ entityId: em.id, fieldId: fm.id })}
                        onKeyDown={(e) => handleKeyDown(e, fieldItem)}
                        onFocus={() => setFocusedKey(fieldKey)}
                      >
                        <StatusDot status={status} />
                        <FieldStatusIcon status={status} />
                        <span
                          className={cn(
                            "text-sm truncate flex-1",
                            isSelected
                              ? "text-foreground font-medium"
                              : "text-muted-foreground",
                          )}
                        >
                          {fm.source_field}
                        </span>
                      </Button>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
};

// ── Public component: desktop 280px rail + mobile Sheet ───────────────────────

const FieldTreeRail = ({
  entityMappings,
  selectedField,
  selectedEntityId,
  onSelectField,
  onSelectEntity,
  onAddEntity,
  onAddField,
  readonly = false,
}: FieldTreeRailProps) => {
  const { t } = useTranslation();
  const [sheetOpen, setSheetOpen] = useState(false);

  const handleSelectField = useCallback(
    (sel: SelectedField) => {
      onSelectField(sel);
      setSheetOpen(false);
    },
    [onSelectField],
  );

  const treeProps = {
    entityMappings,
    selectedField,
    selectedEntityId,
    onSelectField: handleSelectField,
    onSelectEntity,
    onAddEntity,
    onAddField,
    readonly,
  };

  return (
    <>
      {/* Mobile hamburger trigger — visible below md */}
      <div className="md:hidden flex items-center px-3 py-2 border-b border-border">
        <Sheet open={sheetOpen} onOpenChange={setSheetOpen}>
          <SheetTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              className="h-9 gap-1.5 text-sm"
              aria-label={t("sync.mapping.openFieldTree", "Open field tree")}
            >
              <Menu className="h-3.5 w-3.5" />
              {t("sync.mapping.fields", "Fields")}
            </Button>
          </SheetTrigger>
          <SheetContent side="left" className="w-[280px] p-0 flex flex-col">
            <SheetTitle className="sr-only">{t("sync.mapping.fieldTreeLabel", "Entity and field tree")}</SheetTitle>
            <SheetDescription className="sr-only">{t("sync.mapping.selectFieldToBegin", "Select a field to configure its mapping.")}</SheetDescription>
            <TreeContent {...treeProps} onSelectField={handleSelectField} />
          </SheetContent>
        </Sheet>
      </div>

      {/* Desktop: always-visible rail */}
      <div className="hidden md:flex flex-col flex-1 overflow-hidden">
        <TreeContent {...treeProps} />
      </div>
    </>
  );
};

export default FieldTreeRail;
