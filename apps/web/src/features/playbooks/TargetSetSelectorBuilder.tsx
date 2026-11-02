import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronDown, Filter } from "lucide-react";

import { Switch } from "@/shared/ui-primitives/switch";
import { Label } from "@/shared/ui-primitives/label";
import { cn } from "@/shared/lib/utils";
import type { ConnectionTarget } from "@/shared/api/types";
import {
  SELECTOR_FACETS,
  type SelectorFacet,
  type SelectorModel,
} from "@/features/playbooks/selectorModel";
import { inventoryResourceStatusLabel } from "@/features/inventory/resourceStatus";

interface TargetSetSelectorBuilderProps {
  model: SelectorModel;
  onChange: (model: SelectorModel) => void;
  /** Loaded connection targets, used to derive selectable facet values. */
  targets: ConnectionTarget[];
}

/** Maps a facet to the connection-target field whose distinct values populate it. */
function facetValues(facet: SelectorFacet, targets: ConnectionTarget[]): string[] {
  const set = new Set<string>();
  for (const target of targets) {
    let value: string | undefined;
    switch (facet) {
      case "region":
        value = target.region;
        break;
      case "site":
        value = target.site;
        break;
      case "tenant":
        value = target.tenant;
        break;
      case "role":
        value = target.role;
        break;
      case "platform":
        value = target.platform;
        break;
      case "cluster":
        value = target.cluster;
        break;
      case "status":
        value = target.status;
        break;
    }
    if (value) set.add(value);
  }
  return [...set].sort((a, b) => a.localeCompare(b));
}

/**
 * Structured builder for a dynamic target-set selector.
 *
 * Renders one collapsible facet section per dimension (region / site / tenant /
 * role / platform / cluster / status), each a chip checklist whose options are
 * derived from the loaded connection inventory. Everything renders INLINE inside
 * the dialog body (no floating popover) so the checklist scrolls with the wheel
 * inside the dialog's own scroll region — mirrors `WebhookEventPicker`.
 */
const TargetSetSelectorBuilder = ({ model, onChange, targets }: TargetSetSelectorBuilderProps) => {
  const { t } = useTranslation();

  const facetOptions = useMemo(() => {
    const map = {} as Record<SelectorFacet, string[]>;
    for (const facet of SELECTOR_FACETS) map[facet] = facetValues(facet, targets);
    return map;
  }, [targets]);

  const toggleFacetValue = (facet: SelectorFacet, value: string) => {
    const current = model.facets[facet];
    const next = current.includes(value)
      ? current.filter((v) => v !== value)
      : [...current, value];
    onChange({ ...model, facets: { ...model.facets, [facet]: next } });
  };

  const clearFacet = (facet: SelectorFacet) => {
    onChange({ ...model, facets: { ...model.facets, [facet]: [] } });
  };

  return (
    <div className="space-y-2.5">
      <div className="flex items-center gap-1.5">
        <Filter className="h-3 w-3 text-muted-foreground" />
        <p className="text-xs font-medium text-muted-foreground">
          {t("playbooks.page.selectorBuilder.title")}
        </p>
      </div>

      <div className="max-h-[300px] space-y-2 overflow-y-auto overscroll-contain rounded-md border border-border bg-background p-2">
        {SELECTOR_FACETS.map((facet) => (
          <FacetSection
            key={facet}
            facet={facet}
            label={t(`playbooks.page.selectorBuilder.facets.${facet}`)}
            options={facetOptions[facet]}
            selected={model.facets[facet]}
            onToggle={(value) => toggleFacetValue(facet, value)}
            onClear={() => clearFacet(facet)}
            formatStatus={facet === "status" ? (v) => inventoryResourceStatusLabel(t, v) : undefined}
          />
        ))}
      </div>

      <div className="flex items-center justify-between gap-2 rounded-md border border-border bg-background px-3 py-2">
        <div className="min-w-0">
          <Label className="text-sm font-medium">
            {t("playbooks.page.selectorBuilder.addressBound")}
          </Label>
          <p className="text-xs leading-snug text-muted-foreground">
            {t("playbooks.page.selectorBuilder.addressBoundHint")}
          </p>
        </div>
        <Switch
          checked={model.addressBound}
          onCheckedChange={(checked) => onChange({ ...model, addressBound: checked })}
        />
      </div>
    </div>
  );
};

interface FacetSectionProps {
  facet: SelectorFacet;
  label: string;
  options: string[];
  selected: string[];
  onToggle: (value: string) => void;
  onClear: () => void;
  formatStatus?: (value: string) => string;
}

const FacetSection = ({
  facet,
  label,
  options,
  selected,
  onToggle,
  onClear,
  formatStatus,
}: FacetSectionProps) => {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useState(true);
  const selectedCount = selected.length;
  const knownValues = new Set(options);
  const renderedOptions = [
    ...options.map((value) => ({ value, missing: false })),
    ...selected
      .filter((value) => !knownValues.has(value))
      .map((value) => ({ value, missing: true })),
  ];

  return (
    <div className="rounded-md border border-border bg-card">
      <div className="flex items-center justify-between gap-2 px-2 py-1.5">
        <button
          type="button"
          onClick={() => setCollapsed((c) => !c)}
          aria-expanded={!collapsed}
          aria-label={selectedCount > 0 ? `${label} ${selectedCount}` : label}
          className="group flex min-h-9 min-w-0 flex-1 items-center gap-2 rounded-md text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <ChevronDown
            className={cn(
              "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform",
              collapsed && "-rotate-90",
            )}
          />
          <span className="truncate text-sm font-semibold text-foreground">{label}</span>
          {selectedCount > 0 && (
            <span className="shrink-0 rounded-md bg-muted px-2 py-1 text-xs font-medium tabular-nums text-foreground">
              {selectedCount}
            </span>
          )}
        </button>
        {selectedCount > 0 && (
          <button
            type="button"
            onClick={onClear}
            className="min-h-9 shrink-0 rounded-md px-2 text-sm font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            {t("playbooks.page.selectorBuilder.clear")}
          </button>
        )}
      </div>

      {!collapsed && (
        <div className="px-2 pb-2">
          {renderedOptions.length === 0 ? (
            <p className="px-1 py-2 text-sm text-muted-foreground">
              {t("playbooks.page.selectorBuilder.noValues")}
            </p>
          ) : (
            <div className="flex flex-wrap gap-1">
              {renderedOptions.map(({ value, missing }) => {
                const active = selected.includes(value);
                const displayValue = formatStatus ? formatStatus(value) : value;
                const missingBadge = t("playbooks.page.selectorBuilder.missingBadge");
                const selectedClass = active
                  ? "border-primary bg-accent text-foreground"
                  : "border-border bg-card text-muted-foreground hover:bg-accent hover:text-foreground";
                const stateClass = missing
                  ? "border-status-warning bg-card text-status-warning hover:bg-accent"
                  : selectedClass;
                return (
                  <button
                    key={`${facet}:${value}`}
                    type="button"
                    aria-pressed={active}
                    aria-label={missing ? `${displayValue} ${missingBadge}` : displayValue}
                    title={missing ? t("playbooks.page.selectorBuilder.missingValue") : undefined}
                    onClick={() => onToggle(value)}
                    className={cn(
                      "inline-flex min-h-9 items-center gap-2 rounded-md border px-3 py-2 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                      stateClass,
                    )}
                  >
                    {active && <Check className="h-2.5 w-2.5" />}
                    <span className="max-w-[10rem] truncate">
                      {displayValue}
                    </span>
                    {missing && (
                      <span className="text-xs text-muted-foreground">
                        {missingBadge}
                      </span>
                    )}
                  </button>
                );
              })}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

export default TargetSetSelectorBuilder;
