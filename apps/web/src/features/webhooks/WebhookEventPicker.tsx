import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronDown, Layers } from "lucide-react";

import SearchInput from "@/shared/ui-shared/SearchInput";
import { Button } from "@/shared/ui-primitives/button";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { cn } from "@/shared/lib/utils";
import type { WebhookEventCatalogItem } from "@/features/webhooks/api";
import {
  computeEventPresets,
  filterCatalog,
  groupCatalog,
  isBundleActive,
  toggleEventBundle,
  type EventPreset,
} from "@/features/webhooks/webhookEvents";

interface WebhookEventPickerProps {
  catalog: WebhookEventCatalogItem[];
  loading: boolean;
  value: string[];
  onChange: (next: string[]) => void;
}

/**
 * Catalog-driven event picker. Quick presets (all / per-action / per-group /
 * per-object) are computed from the runtime catalog, then a searchable grouped
 * checklist lets the user fine-tune individual events.
 *
 * The grouped checklist renders inline with collapsible groups, sharing the
 * editor's natural page scroll instead of introducing a second scroll owner.
 */
const WebhookEventPicker = ({ catalog, loading, value, onChange }: WebhookEventPickerProps) => {
  const { t } = useTranslation();
  const [term, setTerm] = useState("");

  const selected = useMemo(() => new Set(value), [value]);
  const presets = useMemo(() => computeEventPresets(catalog), [catalog]);
  const filtered = useMemo(() => filterCatalog(catalog, term), [catalog, term]);
  const groups = useMemo(() => groupCatalog(filtered), [filtered]);

  const presetLabel = (preset: EventPreset): string => {
    switch (preset.kind) {
      case "all":
        return t("webhooks.presets.all");
      case "action":
        return t("webhooks.presets.action", { action: preset.token });
      case "group":
        return t("webhooks.presets.group", { group: preset.token });
      case "object":
        return t("webhooks.presets.object", { object: preset.token });
    }
  };

  const toggleOne = (eventValue: string) => {
    onChange(toggleEventBundle([eventValue], value));
  };

  const toggleBundle = (values: string[]) => {
    onChange(toggleEventBundle(values, value));
  };

  const clearAll = () => onChange([]);

  return (
    <div className="space-y-2.5">
      {/* Selection summary + clear-all */}
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs text-muted-foreground">
          {t("webhooks.events.selectedSummary", { count: value.length })}
        </span>
        {value.length > 0 && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="px-2 text-muted-foreground hover:text-foreground"
            onClick={clearAll}
          >
            {t("webhooks.events.clearAll")}
          </Button>
        )}
      </div>

      {/* Quick presets */}
      {presets.length > 0 && (
        <div className="space-y-1.5">
          <div className="flex items-center gap-1.5">
            <Layers className="h-3 w-3 text-muted-foreground" />
            <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
              {t("webhooks.presets.title")}
            </p>
          </div>
          <div className="flex flex-wrap gap-1.5">
            {presets.map((preset) => {
              const active = isBundleActive(preset.values, selected);
              return (
                <Button variant="ghost" size="sm"
                  key={preset.id}
                  type="button"
                  aria-pressed={active}
                  onClick={() => toggleBundle(preset.values)}
                  className={cn(
                    "group inline-flex items-center gap-2 rounded-md border px-3 font-medium transition-colors",
                    active
                      ? "border-primary bg-accent text-primary"
                      : "border-input/30 bg-card text-muted-foreground hover:border-input/60 hover:text-foreground",
                  )}
                >
                  <span
                    className={cn(
                      "flex h-3.5 w-3.5 items-center justify-center rounded-full border transition-colors",
                      active ? "border-primary bg-primary text-primary-foreground" : "border-input/30",
                    )}
                  >
                    {active && <Check className="h-2.5 w-2.5" />}
                  </span>
                  {presetLabel(preset)}
                  <span className="tabular-nums opacity-60">{preset.values.length}</span>
                </Button>
              );
            })}
          </div>
        </div>
      )}

      {/* Search */}
      <SearchInput
        value={term}
        onValueChange={setTerm}
        placeholder={t("webhooks.events.searchPlaceholder")}
        className="h-9"
        onClear={() => setTerm("")}
        clearLabel={t("webhooks.events.clearSearch")}
      />

      {/* Grouped checklist uses the editor's page scroll. */}
      <div className="flex min-w-0 flex-col gap-2">
        {loading ? (
          <p className="px-1 py-6 text-center text-xs text-muted-foreground">{t("loading")}</p>
        ) : groups.length === 0 ? (
          <p className="px-1 py-6 text-center text-xs text-muted-foreground">
            {t("webhooks.events.noMatches")}
          </p>
        ) : (
          groups.map((group) => (
            <EventGroupSection
              key={group.group}
              groupName={group.group}
              groupValues={group.values}
              objects={group.objects}
              selected={selected}
              onToggleOne={toggleOne}
              onToggleBundle={toggleBundle}
            />
          ))
        )}
      </div>
    </div>
  );
};

interface EventGroupSectionProps {
  groupName: string;
  groupValues: string[];
  objects: ReturnType<typeof groupCatalog>[number]["objects"];
  selected: Set<string>;
  onToggleOne: (value: string) => void;
  onToggleBundle: (values: string[]) => void;
}

const EventGroupSection = ({
  groupName,
  groupValues,
  objects,
  selected,
  onToggleOne,
  onToggleBundle,
}: EventGroupSectionProps) => {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useState(false);
  const groupActive = isBundleActive(groupValues, selected);
  const selectedInGroup = groupValues.filter((v) => selected.has(v)).length;

  return (
    <div className="rounded-md border border-border/30 bg-card">
      <div className="flex items-center justify-between gap-2 px-2 py-1.5">
        <Button variant="ghost" size="sm"
          type="button"
          onClick={() => setCollapsed((c) => !c)}
          aria-expanded={!collapsed}
           className="group min-w-0 flex-1 justify-start gap-2 px-1 text-left"
        >
          <ChevronDown
            className={cn(
              "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform",
              collapsed && "-rotate-90",
            )}
          />
          <span className="truncate font-mono text-sm font-semibold text-foreground">{groupName}</span>
          {selectedInGroup > 0 && (
            <span className="shrink-0 rounded-full bg-accent px-1.5 text-xs font-medium tabular-nums text-primary">
              {selectedInGroup}
            </span>
          )}
        </Button>
        <Button variant="ghost" size="sm"
          type="button"
          aria-pressed={groupActive}
          onClick={() => onToggleBundle(groupValues)}
          className={cn(
             "shrink-0 rounded-md border px-2 font-medium transition-colors",
            groupActive
              ? "border-border bg-accent text-primary"
              : "border-input/30 text-muted-foreground hover:border-input/60 hover:text-foreground",
          )}
        >
          {t("webhooks.events.selectAllInGroup")}
        </Button>
      </div>

      {!collapsed && (
        <div className="space-y-2 px-2 pb-2">
          {objects.map((row) => {
            const rowValues = row.items.map((i) => i.value);
            const rowActive = isBundleActive(rowValues, selected);
            return (
              <div key={row.object} className="rounded border border-border/30 bg-card p-1.5">
                <div className="mb-1 flex items-center justify-between gap-2">
                  <span className="truncate font-mono text-xs text-foreground">{row.object}</span>
                  <Button variant="ghost" size="sm"
                    type="button"
                    aria-pressed={rowActive}
                    onClick={() => onToggleBundle(rowValues)}
                    className={cn(
                       "shrink-0 rounded-md px-2 font-medium transition-colors",
                      rowActive
                        ? "text-primary"
                        : "text-muted-foreground hover:text-foreground",
                    )}
                  >
                    {t("webhooks.events.allActions")}
                  </Button>
                </div>
                <div className="flex flex-wrap gap-x-3 gap-y-1">
                  {row.items.map((item) => {
                    const checked = selected.has(item.value);
                    return (
                      <label
                        key={item.value}
                        className="flex min-h-9 cursor-pointer items-center gap-2 text-sm text-foreground"
                        title={item.value}
                      >
                        <Checkbox
                          className="h-3.5 w-3.5"
                          checked={checked}
                          onCheckedChange={() => onToggleOne(item.value)}
                          aria-label={item.value}
                        />
                        <span className="font-mono">{item.action}</span>
                      </label>
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};

export default WebhookEventPicker;
