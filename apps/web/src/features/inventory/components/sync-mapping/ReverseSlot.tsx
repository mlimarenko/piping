import { useApiMutation } from "@/shared/api/hooks";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Undo2, ChevronDown, ChevronRight, X } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { cn } from "@/shared/lib/utils";
import type { EntityMapping, FieldMapping, Transform } from "@/shared/api/types";
import { suggestReverse } from "@/features/inventory/sync-mapping-api";
import OperatorPicker from "./OperatorPicker";
import OperatorArgsForm from "./OperatorArgsForm";

/**
 * Reverse-transform editor with read-only badge when reverse_transform is null.
 *
 * - When reverse is null and forward.invertibility_hint === "invertible":
 *   shows [↩ Suggest reverse] button.
 * - When reverse is null: read-only badge in muted styling.
 * - When reverse is authored: full operator picker + args form.
 *
 * Follow-up: aria-live="polite" announce when reverse is materialized
 * Follow-up: wire invertibility_hint from operator registry to show/hide suggest button
 */

interface ReverseSlotProps {
  fieldMapping: FieldMapping;
  entityMapping: EntityMapping;
  readonly?: boolean;
  onUpdate: (reverse: Transform | null) => void;
}

const ReverseSlot = ({
  fieldMapping,
  entityMapping,
  readonly = false,
  onUpdate,
}: ReverseSlotProps) => {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);

  const suggestMutation = useApiMutation({
    mutationFn: () =>
      suggestReverse({
        forward_transform: fieldMapping.forward_transform,
        source_entity: entityMapping.source_entity,
        source_field: fieldMapping.source_field,
      }),
    // Keep the originating authoring callback: a pending mutation's live
    // options may otherwise replace it after Save or Discard advances scope.
    onSuccess: (res, applyResult: ReverseSlotProps["onUpdate"]) => {
      if (res.reverse_transform) {
        applyResult(res.reverse_transform);
      }
    },
  });

  const hasReverse = fieldMapping.reverse_transform !== null;

  return (
    <section className="flex flex-col gap-4 border-t border-border pt-4">
      {/* Section header */}
      <div className="flex items-center gap-2">
        <Button variant="ghost"
          type="button"
           className="flex flex-wrap items-center gap-1.5 text-sm font-semibold text-foreground hover:text-foreground/80 transition-colors"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
        >
          {expanded ? (
            <ChevronDown className="h-3 w-3" />
          ) : (
            <ChevronRight className="h-3 w-3" />
          )}
          {t("sync.mapping.reverseTransform", "Reverse transform")}
          <span className="ml-1 font-mono text-[11px] font-normal text-muted-foreground">
            {entityMapping.target_entity} → {entityMapping.source_entity}
          </span>
        </Button>

        {/* Read-only badge when reverse is null */}
        {!hasReverse && (
          <span
            className="px-1.5 py-0.5 rounded text-xs font-medium bg-muted text-muted-foreground"
            title={t(
              "sync.mapping.readOnlyBadgeTooltip",
              "This field is forward-only. Reverse direction is not authored. Target→source writes for this field will be blocked.",
            )}
          >
            {t("sync.mapping.readOnlyBadge", "← read-only")}
          </span>
        )}

        {/* Suggest reverse button */}
        {!hasReverse && !readonly && (
          <Button
            variant="outline"
            size="sm"
            className="h-9 text-sm px-2 gap-1 ml-auto"
            onClick={() => suggestMutation.mutate(onUpdate)}
            disabled={suggestMutation.isPending}
          >
            <Undo2 className="h-3 w-3" />
            {t("sync.mapping.suggestReverse", "Suggest reverse")}
          </Button>
        )}

        {/* Clear reverse button */}
        {hasReverse && !readonly && (
          <Button
            variant="ghost"
            size="sm"
            className="h-9 text-sm px-2 gap-1 ml-auto text-muted-foreground"
            onClick={() => onUpdate(null)}
          >
            <X className="h-3 w-3" />
            {t("sync.mapping.clearReverse", "Clear reverse")}
          </Button>
        )}
      </div>

      {/* Reverse form */}
      {expanded && (
        <div
          className={cn(
            "space-y-2 pl-3 border-l-2 border-border",
            !hasReverse && "opacity-60",
          )}
        >
          {hasReverse && fieldMapping.reverse_transform ? (
            <>
              <div className="space-y-2">
                <div className="text-sm font-medium">
                  {t("sync.mapping.operatorLabel", "Operator")}
                </div>
                <OperatorPicker
                  value={fieldMapping.reverse_transform.operator}
                  onChange={(op) =>
                    onUpdate({ operator: op, args: {} })
                  }
                  disabled={readonly}
                  slot="reverse"
                />
              </div>
              <OperatorArgsForm
                operator={fieldMapping.reverse_transform.operator}
                args={fieldMapping.reverse_transform.args}
                onChange={(args) =>
                  onUpdate({ ...fieldMapping.reverse_transform!, args })
                }
                disabled={readonly}
                sourceEntity={entityMapping.target_entity}
              />
            </>
          ) : (
            <p className="text-xs text-muted-foreground italic py-1">
              {t(
                "sync.mapping.noReverseHint",
                "No automatic reverse — author manually if needed, or leave null for read-only direction.",
              )}
            </p>
          )}
        </div>
      )}

      {/* Suggest reverse note */}
      {suggestMutation.data?.note && (
        <p className="text-xs text-muted-foreground">
          {suggestMutation.data.note}
        </p>
      )}

      {/* aria-live region for screen reader announcement */}
      <div aria-live="polite" className="sr-only">
        {suggestMutation.isSuccess && hasReverse
          ? t(
              "sync.mapping.reverseSuggested",
              "Reverse expression suggested. Review before saving.",
            )
          : ""}
      </div>
    </section>
  );
};

export default ReverseSlot;
