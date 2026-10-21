import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { SELECTOR_FACETS, selectorToModel } from "@/features/playbooks/selectorModel";
import { inventoryResourceStatusLabel } from "@/features/inventory/resourceStatus";

interface SelectorChipsProps {
  /** Raw target-set selector object (opaque JSON from the backend). */
  selector: unknown;
  /** Cap the values rendered per facet before collapsing into "+N". */
  maxPerFacet?: number;
  className?: string;
}

/** Translate a single facet value (status is an inventory resource status). */
function facetValueLabel(facet: string, value: string, t: TFunction): string {
  if (facet === "status") return inventoryResourceStatusLabel(t, value);
  return value;
}

/**
 * Renders a target-set selector as human-readable facet chips
 * (e.g. "Region: eu-west, fra · Role: web (+2)") instead of raw JSON. Inverts
 * the structured {@link selectorToModel} model and reuses the selector-builder
 * facet labels; status values render through inventory resource labels.
 *
 * Falls back to an "any host" hint when no facet is constrained.
 */
function SelectorChips({ selector, maxPerFacet = 2, className }: SelectorChipsProps) {
  const { t } = useTranslation();
  const model = selectorToModel(selector);

  const chips = SELECTOR_FACETS.flatMap((facet) => {
    const values = model.facets[facet];
    if (!values || values.length === 0) return [];
    const shown = values
      .slice(0, maxPerFacet)
      .map((value) => facetValueLabel(facet, value, t));
    const overflow = values.length - shown.length;
    const label = t(`playbooks.page.selectorBuilder.facets.${facet}`);
    const valueText = overflow > 0 ? `${shown.join(", ")} (+${overflow})` : shown.join(", ");
    return [{ facet, text: `${label}: ${valueText}` }];
  });

  if (chips.length === 0) {
    return (
      <span className={className}>
        <span className="text-xs text-muted-foreground">
          {t("playbooks.page.selectorChips.anyHost")}
        </span>
      </span>
    );
  }

  return (
    <span className={className}>
      <span className="flex flex-wrap gap-1">
        {chips.map((chip) => (
          <span
            key={chip.facet}
            className="inline-flex max-w-full items-center rounded-md border border-border bg-muted px-2 py-1 text-xs font-medium text-muted-foreground"
          >
            <span className="truncate">{chip.text}</span>
          </span>
        ))}
      </span>
    </span>
  );
}

export default SelectorChips;
