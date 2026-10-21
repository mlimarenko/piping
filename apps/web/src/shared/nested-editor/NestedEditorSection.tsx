import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  Plus,
  Trash2,
  RotateCcw,
  AlertCircle,
  Loader2,
  Star,
} from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Badge } from "@/shared/ui-primitives/badge";
import FormSection from "@/shared/ui-shared/FormSection";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import type { EntityPickerOption } from "@/shared/forms/EntityPicker";
import { fieldsByTab } from "@/features/inventory/components/inventory/inventoryTypes";
import { cn } from "@/shared/lib/utils";
import type { InventoryEntityKey } from "@/shared/api/types";
import type { LocalId, NestedEditorTree, NestedNode } from "./useNestedEditorTree";
import type { RelationSpec } from "./descriptorFromSpec";

/**
 * Presentational, shell-agnostic renderer for one nested-editor relation level.
 *
 * It draws a {@link FormSection} (title + count badge + "+") whose body is a
 * compact tree-table; expanding a row reveals that row's edit form plus an
 * indented child `<NestedEditorSection>` (recursion). It owns NO state and does
 * NO network: the headless {@link NestedEditorTree} supplies every value and
 * mutation, and the host (inventory `ScrollableDialog` or connections `sheet`
 * drawer) supplies the related-entity option loaders.
 *
 * It deliberately does NOT render the shared DataTable primitive: that primitive
 * has no expanded-row API and pulls in the table-contract registry. A hand-rolled
 * compact table keeps nested entities on the shared form surfaces.
 */

export interface NestedEditorSectionProps {
  /** The relation spec for this level (carries title, icon, child spec). */
  spec: RelationSpec;
  /** The headless tree this section reads/mutates. */
  tree: NestedEditorTree;
  /** Parent node id, or undefined for a top-level relation under the root. */
  parentLocalId?: LocalId;
  /** Nesting depth, drives the left indent of child sections. */
  depth?: number;
  /** When true, all edit/add/delete affordances are hidden (view-only). */
  readOnly?: boolean;
  /** Related-entity option loader for config-form pickers. */
  loadOptions?: (entity: InventoryEntityKey) => EntityPickerOption[];
  /** Loading state for a related entity's option list. */
  isOptionsLoading?: (entity: InventoryEntityKey) => boolean;
  /** Launch inline creation of a related entity (the "+" next to a picker). The
   *  row's node id is forwarded so the host can write the new id back into the
   *  right draft. */
  onRequestCreate?: (entity: InventoryEntityKey, returnToFieldKey: string, nodeLocalId: LocalId) => void;
  /** When provided AND this level is the IP-address relation, each persisted IP
   *  row shows a "primary" toggle that selects the parent device's primary IP. */
  primarySelection?: PrimarySelection;
}

/** Wires the device's primary-IP FK (primary_ip4_id / primary_ip6_id) to a
 *  per-IP-row toggle. The host owns the current ids + the setter. */
export interface PrimarySelection {
  current4?: string;
  current6?: string;
  onSelect: (ipRealId: string, isV6: boolean) => void;
}

const NOOP_OPTIONS = (): EntityPickerOption[] => [];

/** The IP-address relation's config entity; the primary-IP toggle only applies
 *  to rows at this level. */
const IP_ADDRESS_ENTITY = "IP Addresses";

/** A short, human row label drawn from the draft (name, else address/prefix,
 *  else the first non-empty value). Keeps the collapsed row readable without a
 *  full column set. */
function rowLabel(node: NestedNode, fallback: string): string {
  const d = node.draft;
  const primary =
    d.name?.trim() ||
    d.prefix?.trim() ||
    d.address?.trim() ||
    d.interfaceName?.trim() ||
    Object.values(d).find((v) => v && v.trim())?.trim();
  return primary || fallback;
}

const STATUS_BADGE: Record<string, { labelKey: string; className: string }> = {
  new: { labelKey: "nestedEditor.status.new", className: "bg-primary/15 text-primary border-primary/30" },
  dirty: { labelKey: "nestedEditor.status.dirty", className: "bg-amber-500/15 text-amber-600 border-amber-500/30" },
  deleted: { labelKey: "nestedEditor.status.deleted", className: "bg-destructive/15 text-destructive border-destructive/30 line-through" },
  failed: { labelKey: "nestedEditor.status.failed", className: "bg-destructive/15 text-destructive border-destructive/30" },
  blocked: { labelKey: "nestedEditor.status.blocked", className: "bg-muted text-muted-foreground border-border/40" },
};

export default function NestedEditorSection({
  spec,
  tree,
  parentLocalId,
  depth = 0,
  readOnly = false,
  loadOptions = NOOP_OPTIONS,
  isOptionsLoading,
  onRequestCreate = () => undefined,
  primarySelection,
}: NestedEditorSectionProps) {
  const { t } = useTranslation();
  const rowIds = tree.rowsByRelation(spec.key, parentLocalId);
  const liveRows = useMemo(
    () => rowIds.filter((id) => tree.nodes[id]?.status !== "deleted"),
    [rowIds, tree],
  );
  const truncated = rowIds.some((id) => tree.nodes[id]?.truncated);
  const levelTotal = rowIds.reduce(
    (max, id) => Math.max(max, tree.nodes[id]?.levelTotal ?? 0),
    0,
  );
  // A truncated level can't tell a removed row from a never-loaded one, so
  // deletes are blocked there even when otherwise editable.
  const deletesDisabled = readOnly || truncated;

  return (
    <FormSection
      title={t(spec.title)}
      action={
        <div className="flex items-center gap-2">
          <Badge variant="outline" className="h-5 px-1.5 text-xs tabular-nums">
            {liveRows.length}
          </Badge>
          {!readOnly && (
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="h-6 w-6"
              onClick={() => tree.addRow(spec.key, parentLocalId)}
              aria-label={t("nestedEditor.addRow", { entity: t(spec.title) })}
            >
              <Plus className="h-3.5 w-3.5" />
            </Button>
          )}
        </div>
      }
      className={cn(depth > 0 && "mt-3")}
    >
      {truncated && (
        <p className="mb-2 flex items-center gap-1.5 text-xs text-muted-foreground">
          <AlertCircle className="h-3 w-3 shrink-0" />
          {t("nestedEditor.truncated", { shown: liveRows.length, total: levelTotal })}
        </p>
      )}

      {liveRows.length === 0 ? (
        <p className="rounded-md border border-dashed border-border/40 py-3 text-center text-xs text-muted-foreground">
          {t("nestedEditor.empty", { entity: t(spec.title) })}
        </p>
      ) : (
        <ul className="space-y-1">
          {liveRows.map((id) => {
            const node = tree.nodes[id];
            if (!node) return null;
            return (
              <NestedEditorRow
                key={id}
                node={node}
                spec={spec}
                tree={tree}
                depth={depth}
                readOnly={readOnly}
                deletesDisabled={deletesDisabled}
                loadOptions={loadOptions}
                isOptionsLoading={isOptionsLoading}
                onRequestCreate={onRequestCreate}
                primarySelection={primarySelection}
              />
            );
          })}
        </ul>
      )}
    </FormSection>
  );
}

interface NestedEditorRowProps {
  node: NestedNode;
  spec: RelationSpec;
  tree: NestedEditorTree;
  depth: number;
  readOnly: boolean;
  deletesDisabled: boolean;
  loadOptions: (entity: InventoryEntityKey) => EntityPickerOption[];
  isOptionsLoading?: (entity: InventoryEntityKey) => boolean;
  onRequestCreate: (entity: InventoryEntityKey, returnToFieldKey: string, nodeLocalId: LocalId) => void;
  primarySelection?: PrimarySelection;
}

function NestedEditorRow({
  node,
  spec,
  tree,
  depth,
  readOnly,
  deletesDisabled,
  loadOptions,
  isOptionsLoading,
  onRequestCreate,
  primarySelection,
}: NestedEditorRowProps) {
  const { t } = useTranslation();
  const expanded = tree.isExpanded(node.localId);
  const hasChild = !!spec.child;
  const badge = STATUS_BADGE[node.status];
  const errors = expanded ? tree.validationErrors(node.localId) : {};
  // The parent FK is implied by nesting, so its field is hidden from the row
  // form (a child IP never re-picks its interface inline).
  const hiddenFkKey = spec.parentFkField as string;
  const configEntity: InventoryEntityKey | null =
    spec.rowForm.kind === "config" ? spec.rowForm.entity : null;
  // Primary-IP toggle: only on persisted IP-address rows, and only when the host
  // wired the selection. The address (v6 has ":") routes to the v4 vs v6 FK.
  const ipText = node.draft.prefix || node.draft.address || "";
  const isV6Ip = ipText.includes(":");
  const showPrimary =
    configEntity === IP_ADDRESS_ENTITY && !!primarySelection && !!node.realId;
  const isPrimary =
    !!node.realId &&
    (node.realId === primarySelection?.current4 || node.realId === primarySelection?.current6);
  // Render every config field except the parent-FK field (and `name`/`status`,
  // which EntityFormBody always shows unless filtered out — they stay on).
  const visibleKeys = useMemo(() => {
    if (!configEntity) return undefined;
    const keys = new Set<string>(["name", "status"]);
    for (const f of fieldsByTab[configEntity] ?? []) {
      const stateKey = (f.idKey ?? f.key) as string;
      if (stateKey !== hiddenFkKey) keys.add(stateKey);
    }
    return keys;
  }, [configEntity, hiddenFkKey]);

  return (
    <li className="rounded-md border border-border bg-card">
      <div className="flex items-center gap-1.5 px-2 py-1.5">
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="size-8 shrink-0"
          onClick={() => tree.toggleExpand(node.localId)}
          aria-label={expanded ? t("nestedEditor.collapse") : t("nestedEditor.expand")}
          aria-expanded={expanded}
        >
          {expanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
        </Button>
        <span className="min-w-0 flex-1 truncate text-xs text-foreground">
          {rowLabel(node, t(spec.title))}
        </span>
        {node.status === "saving" && <Loader2 className="h-3 w-3 shrink-0 animate-spin text-muted-foreground" />}
        {badge && (
          <Badge variant="outline" className={cn("min-h-5 px-1.5 text-xs", badge.className)}>
            {t(badge.labelKey)}
          </Badge>
        )}
        {showPrimary && (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className={cn(
              "size-8 shrink-0",
              isPrimary ? "text-primary" : "text-muted-foreground hover:text-primary",
            )}
            disabled={readOnly}
            title={isPrimary ? t("nestedEditor.primaryIp") : t("nestedEditor.setPrimaryIp")}
            aria-label={isPrimary ? t("nestedEditor.primaryIp") : t("nestedEditor.setPrimaryIp")}
            aria-pressed={isPrimary}
            onClick={() => node.realId && primarySelection?.onSelect(node.realId, isV6Ip)}
          >
            <Star className={cn("h-3.5 w-3.5", isPrimary && "fill-current")} />
          </Button>
        )}
        {!readOnly && (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="size-8 shrink-0 text-muted-foreground hover:text-destructive"
            disabled={deletesDisabled && !!node.realId}
            title={deletesDisabled && node.realId ? t("nestedEditor.deleteBlockedTruncated") : undefined}
            onClick={() => tree.markDeleted(node.localId)}
            aria-label={t("delete")}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>

      {node.status === "failed" && node.error && (
        <div className="mx-2 mb-1.5 flex items-start gap-1.5 rounded-sm bg-destructive/10 px-2 py-1 text-xs text-destructive">
          <AlertCircle className="mt-0.5 h-3 w-3 shrink-0" />
          <span className="min-w-0 break-words">{node.error}</span>
          {!readOnly && (
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="ml-auto size-8 shrink-0"
              onClick={() => tree.retryNode(node.localId)}
              aria-label={t("nestedEditor.retry")}
            >
              <RotateCcw className="h-3 w-3" />
            </Button>
          )}
        </div>
      )}

      {expanded && (
        <div
          className="border-t border-border/20 px-2.5 py-2"
          style={{ marginLeft: depth > 0 ? 0 : undefined }}
        >
          {configEntity ? (
            <EntityFormBody
              entity={configEntity}
              draft={node.draft}
              errors={errors}
              onField={(key, value) => tree.patchDraft(node.localId, key, value)}
              loadOptions={loadOptions}
              isOptionsLoading={isOptionsLoading}
              onRequestCreate={(entity, fieldKey) => onRequestCreate(entity, fieldKey, node.localId)}
              visibleKeys={visibleKeys}
              disabledKeys={readOnly ? visibleKeys : undefined}
            />
          ) : (
            <p className="text-xs text-muted-foreground">{t("nestedEditor.noForm")}</p>
          )}

          {hasChild && spec.child && (
            <div className="mt-3 border-l border-border/30 pl-3">
              <NestedEditorSection
                spec={spec.child}
                tree={tree}
                parentLocalId={node.localId}
                depth={depth + 1}
                readOnly={readOnly}
                loadOptions={loadOptions}
                isOptionsLoading={isOptionsLoading}
                onRequestCreate={onRequestCreate}
                primarySelection={primarySelection}
              />
            </div>
          )}
        </div>
      )}
    </li>
  );
}
