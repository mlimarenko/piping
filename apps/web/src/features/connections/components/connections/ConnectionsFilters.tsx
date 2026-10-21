import { useId, useMemo, useState } from "react";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import SearchInput from "@/shared/ui-shared/SearchInput";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/shared/ui-primitives/dialog";
import { RadioGroup, RadioGroupItem } from "@/shared/ui-primitives/radio-group";
import {
  X,
  ChevronDown,
  ChevronUp,
  Check,
  Bookmark,
  Globe2,
  Lock,
  Loader2,
  Save,
  Trash2,
} from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { useTranslation } from "react-i18next";
import type { ConnectionTarget } from "@/shared/api/types";
import {
  cloneFilterState,
  deserializeFilterState,
  emptyFilterState,
  FILTER_KEYS,
  filterStateEquals,
  getFilterValues,
  type FilterKey,
  type FilterPresetVisibility,
  type FilterState,
  type SavedFilterPreset,
} from "./filters-utils";

interface Props {
  /** Closes the dialog without publishing the draft (Cancel / Escape / backdrop). */
  onClose: () => void;
  /** The full host list — options are derived from it so only real values appear. */
  hosts: ConnectionTarget[];
  /** Applied selection seeded into the draft on open and never edited directly. */
  selected: FilterState;
  /** Publishes the draft. The parent owns the applied selection and re-filters. */
  onChange: (next: FilterState) => void;
  privatePresets: SavedFilterPreset[];
  publicPresets: SavedFilterPreset[];
  presetsLoading?: boolean;
  savingPreset?: boolean;
  deletingPresetId?: string | null;
  onSavePreset: (name: string, visibility: FilterPresetVisibility, filters: FilterState) => void | Promise<void>;
  onApplyPreset: (preset: SavedFilterPreset) => void;
  onDeletePreset: (preset: SavedFilterPreset) => void | Promise<void>;
}

const SEARCH_THRESHOLD = 6;

/** Builds the option set for each filter group from the loaded hosts. Any
 *  dimension with zero distinct non-empty values collapses to [] and the
 *  group renders empty — canonical behaviour: if the data never carries
 *  "platform", stop pretending the filter exists.  */
function collectOptions(hosts: ConnectionTarget[]): Record<FilterKey, string[]> {
  const sets = emptyFilterState();
  for (const h of hosts) {
    for (const key of FILTER_KEYS) {
      for (const value of getFilterValues(h, key)) {
        sets[key].add(value);
      }
    }
  }
  const out = {} as Record<FilterKey, string[]>;
  for (const key of FILTER_KEYS) {
    out[key] = Array.from(sets[key]).sort((a, b) => a.localeCompare(b));
  }
  return out;
}

const ConnectionsFilters = ({
  onClose,
  hosts,
  selected,
  onChange,
  privatePresets,
  publicPresets,
  presetsLoading = false,
  savingPreset = false,
  deletingPresetId = null,
  onSavePreset,
  onApplyPreset,
  onDeletePreset,
}: Props) => {
  const { t } = useTranslation();
  const options = useMemo(() => collectOptions(hosts), [hosts]);
  // The dialog edits a draft copy; the applied selection only changes on Apply.
  // `selected` is the seed when the dialog mounts; a remount (open) reseeds it.
  const [draft, setDraft] = useState<FilterState>(() => cloneFilterState(selected));
  const [expanded, setExpanded] = useState<FilterKey[]>(["status"]);
  const [searches, setSearches] = useState<Record<FilterKey, string>>({} as Record<FilterKey, string>);
  const [presetName, setPresetName] = useState("");
  const [presetVisibility, setPresetVisibility] = useState<FilterPresetVisibility>("private");
  const [presetSearch, setPresetSearch] = useState("");
  const [saveDialogOpen, setSaveDialogOpen] = useState(false);
  const [pendingPreset, setPendingPreset] = useState<SavedFilterPreset | null>(null);
  const presetNameInputId = useId();
  const presetVisibilityId = useId();

  const groups = FILTER_KEYS.map((key) => ({
    key,
    label: t(`connections.filtersPanel.${key}`),
    options: options[key],
  }));

  const toggle = (key: FilterKey) => {
    setExpanded((prev) => (prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key]));
  };

  const toggleOption = (groupKey: FilterKey, option: string) => {
    setDraft((prev) => {
      const next = cloneFilterState(prev);
      const s = new Set(next[groupKey]);
      if (s.has(option)) s.delete(option);
      else s.add(option);
      next[groupKey] = s;
      return next;
    });
  };

  const clearGroup = (groupKey: FilterKey) => {
    setDraft((prev) => ({ ...cloneFilterState(prev), [groupKey]: new Set<string>() }));
  };

  const clearAll = () => {
    setDraft(emptyFilterState());
  };

  const totalSelected = FILTER_KEYS.reduce((s, k) => s + draft[k].size, 0);
  const dirty = !filterStateEquals(draft, selected) || pendingPreset !== null;
  const apply = () => {
    if (pendingPreset && filterStateEquals(draft, deserializeFilterState(pendingPreset.filters))) {
      onApplyPreset(pendingPreset);
      setPendingPreset(null);
    } else {
      onChange(cloneFilterState(draft));
    }
    onClose();
  };
  const canSavePreset = totalSelected > 0 && presetName.trim().length > 0 && !savingPreset;
  const normalizedPresetSearch = presetSearch.trim().toLowerCase();
  const totalPresetCount = privatePresets.length + publicPresets.length;
  const filteredPrivatePresets = normalizedPresetSearch
    ? privatePresets.filter((preset) => preset.name.toLowerCase().includes(normalizedPresetSearch))
    : privatePresets;
  const filteredPublicPresets = normalizedPresetSearch
    ? publicPresets.filter((preset) => preset.name.toLowerCase().includes(normalizedPresetSearch))
    : publicPresets;
  const visiblePresetCount = filteredPrivatePresets.length + filteredPublicPresets.length;

  const getFilteredOptions = (group: { key: FilterKey; options: string[] }) => {
    const q = (searches[group.key] ?? "").toLowerCase();
    if (!q) return group.options;
    return group.options.filter((o) => formatFilterOption(group.key, o).toLowerCase().includes(q));
  };

  const formatFilterOption = (key: FilterKey, value: string) => {
    switch (key) {
      case "targetKind":
        return t(`connections.filtersPanel.targetKinds.${value}`, value);
      case "ipFamily":
        return t(`connections.filtersPanel.ipFamilies.${value}`, value);
      case "addressState":
        return t(`connections.filtersPanel.addressStates.${value}`, value);
      case "addressBinding":
        return t(`connections.filtersPanel.addressBindings.${value}`, value);
      default:
        return value;
    }
  };

  const handleSavePreset = async () => {
    if (!canSavePreset) return;
    await onSavePreset(presetName, presetVisibility, cloneFilterState(draft));
    setPresetName("");
    setSaveDialogOpen(false);
  };

  const renderPresetList = (
    presets: SavedFilterPreset[],
    visibility: FilterPresetVisibility,
  ) => {
    const VisibilityIcon = visibility === "public" ? Globe2 : Lock;
    if (presets.length === 0) {
      return null;
    }
    return (
      <div className="space-y-1">
        <div className="flex items-center gap-1.5 px-1 text-xs font-medium uppercase text-muted-foreground/70">
          <VisibilityIcon className="h-3 w-3" />
          <span>{t(`connections.filtersPanel.${visibility}`)}</span>
        </div>
        <div className="space-y-1">
          {presets.map((preset) => (
            <div
              key={preset.id}
              className="group/preset flex items-center gap-1 rounded-md border border-input/30 bg-accent px-1.5 py-1 transition-colors hover:border-primary/30 hover:bg-primary/8"
            >
              <button
                type="button"
                className="min-w-0 flex-1 text-left"
                onClick={() => {
                  setDraft(deserializeFilterState(preset.filters));
                  setPendingPreset(preset);
                }}
                title={t("connections.filtersPanel.applyPreset")}
              >
                <span className="block truncate text-xs font-medium leading-4 text-foreground">
                  {preset.name}
                </span>
                <span className="block truncate text-xs leading-3 text-muted-foreground">
                  {t("connections.filtersPanel.selectedCount", {
                    count: Object.values(preset.filters).reduce((sum, values) => sum + (values?.length ?? 0), 0),
                  })}
                </span>
              </button>
              <button
                type="button"
                className="flex h-8 w-8 shrink-0 items-center justify-center rounded text-muted-foreground opacity-70 transition-colors hover:bg-destructive/10 hover:text-destructive group-hover/preset:opacity-100"
                onClick={() => void onDeletePreset(preset)}
                title={t("connections.filtersPanel.deletePreset")}
                disabled={deletingPresetId === preset.id}
              >
                {deletingPresetId === preset.id ? (
                  <Loader2 className="h-3 w-3 animate-spin" />
                ) : (
                  <Trash2 className="h-3 w-3" />
                )}
              </button>
            </div>
          ))}
        </div>
      </div>
    );
  };

  return (
    <SurfaceCard className="flex min-h-0 w-full flex-col overflow-hidden rounded-none border-0 bg-transparent shadow-none">
      <div className="flex items-center justify-between px-3 py-2 pr-12 border-b border-border/30">
        <div className="flex items-center gap-1.5">
          <DialogTitle className="text-base font-semibold text-foreground">{t("connections.filters")}</DialogTitle>
          {totalSelected > 0 && (
            <span className="flex items-center justify-center min-w-[16px] h-4 rounded-full bg-primary/15 text-primary text-xs font-semibold px-1">
              {totalSelected}
            </span>
          )}
        </div>
        <div className="flex gap-1">
          {totalSelected > 0 && (
            <Button
              variant="ghost"
              size="sm"
              className="h-9 text-sm text-muted-foreground"
              onClick={clearAll}
            >
              {t("connections.filtersPanel.clearAll")}
            </Button>
          )}
        </div>
      </div>

      {totalSelected > 0 && (
        <div className="px-2 py-1.5 border-b border-border/20 flex flex-wrap gap-1 max-h-20 overflow-y-auto">
          {FILTER_KEYS.flatMap((gk) =>
            Array.from(draft[gk]).map((v) => (
              <button
                type="button"
                key={`${gk}-${v}`}
                onClick={() => toggleOption(gk, v)}
                className="flex items-center gap-0.5 surface-chip px-1.5 py-0.5 rounded text-xs text-foreground hover:bg-destructive/10 hover:text-destructive transition-colors group"
              >
                <span className="truncate max-w-[100px]">{formatFilterOption(gk, v)}</span>
                <X className="h-2.5 w-2.5 text-muted-foreground group-hover:text-destructive shrink-0" />
              </button>
            )),
          )}
        </div>
      )}

      <div className="grid min-h-0 flex-1 grid-cols-1 items-start gap-2 overflow-y-auto p-4 sm:grid-cols-2">
        <div className="rounded-lg border border-border/25 bg-accent p-2 sm:col-span-2">
          <div className="flex items-center justify-between gap-2">
            <div className="flex min-w-0 items-center gap-1.5">
              <Bookmark className="h-3.5 w-3.5 text-primary" />
              <span className="truncate text-xs font-semibold text-foreground">
                {t("connections.filtersPanel.savedFilters")}
              </span>
              {totalPresetCount > 0 && (
                <span className="rounded-full bg-background px-1.5 py-0.5 text-xs font-semibold text-muted-foreground">
                  {totalPresetCount}
                </span>
              )}
            </div>
            <div className="flex shrink-0 items-center gap-1">
              {presetsLoading && <Loader2 className="h-3 w-3 animate-spin text-muted-foreground" />}
              <Dialog open={saveDialogOpen} onOpenChange={setSaveDialogOpen}>
                <DialogTrigger asChild>
                  <button
                    type="button"
                    disabled={totalSelected === 0 || savingPreset}
                    className="flex h-8 w-8 items-center justify-center rounded-md border border-border/25 text-muted-foreground transition-colors hover:bg-primary/10 hover:text-primary disabled:pointer-events-none disabled:opacity-40"
                    title={
                      totalSelected === 0
                        ? t("connections.filtersPanel.emptyPreset")
                        : t("connections.filtersPanel.save")
                    }
                  >
                    {savingPreset ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Save className="h-3.5 w-3.5" />}
                  </button>
                </DialogTrigger>
                <DialogContent
                  className="surface max-w-[calc(100vw-2rem)] border-border/40 p-4 sm:max-w-sm"
                  description={t("connections.filtersPanel.saveDescription")}
                >
                  <DialogHeader>
                    <DialogTitle className="flex items-center gap-2 text-sm">
                      <Save className="h-4 w-4 text-primary" />
                      {t("connections.filtersPanel.save")}
                    </DialogTitle>
                    <DialogDescription className="text-xs">
                      {t("connections.filtersPanel.selectedCount", { count: totalSelected })}
                    </DialogDescription>
                  </DialogHeader>

                  <div className="space-y-3">
                    <div className="space-y-1.5">
                      <FieldLabel
                        htmlFor={presetNameInputId}
                        className="text-xs font-medium text-muted-foreground"
                        required
                      >
                        {t("connections.filtersPanel.filterName")}
                      </FieldLabel>
                      <Input
                        id={presetNameInputId}
                        value={presetName}
                        onChange={(event) => setPresetName(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" && canSavePreset) {
                            event.preventDefault();
                            void handleSavePreset();
                          }
                        }}
                        placeholder={t("connections.filtersPanel.filterName")}
                        className="h-8 text-xs"
                        autoFocus
                      />
                    </div>

                    <div className="space-y-1.5">
                      <div className="text-xs font-medium text-muted-foreground">
                        {t("connections.filtersPanel.visibility")}
                      </div>
                      <RadioGroup
                        value={presetVisibility}
                        onValueChange={(value) => setPresetVisibility(value as FilterPresetVisibility)}
                        className="grid grid-cols-2 gap-2"
                      >
                        {(["private", "public"] as FilterPresetVisibility[]).map((visibility) => {
                          const Icon = visibility === "public" ? Globe2 : Lock;
                          const visibilityInputId = `${presetVisibilityId}-${visibility}`;
                          return (
                            <label
                              key={visibility}
                              htmlFor={visibilityInputId}
                              className={cn(
                                "flex min-h-11 cursor-pointer items-center gap-2 rounded-md border px-2.5 py-2 text-xs transition-colors",
                                presetVisibility === visibility
                                  ? "border-primary/45 bg-primary/10 text-foreground"
                                  : "border-input/30 text-muted-foreground hover:bg-accent hover:text-foreground",
                              )}
                            >
                              <RadioGroupItem
                                id={visibilityInputId}
                                value={visibility}
                                className="h-3.5 w-3.5 shrink-0"
                              />
                              <Icon className="h-3.5 w-3.5 shrink-0" />
                              <span className="truncate font-medium">
                                {t(`connections.filtersPanel.${visibility}`)}
                              </span>
                            </label>
                          );
                        })}
                      </RadioGroup>
                    </div>
                  </div>

                  <DialogFooter className="gap-2 sm:space-x-0">
                    <DialogClose asChild>
                      <Button variant="ghost" size="sm" className="h-11 md:h-8">
                        {t("cancel")}
                      </Button>
                    </DialogClose>
                    <Button
                      type="button"
                      size="sm"
                      className="h-11 md:h-8"
                      disabled={!canSavePreset}
                      onClick={() => void handleSavePreset()}
                    >
                      {savingPreset ? <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" /> : <Save className="mr-1.5 h-3.5 w-3.5" />}
                      {t("connections.filtersPanel.save")}
                    </Button>
                  </DialogFooter>
                </DialogContent>
              </Dialog>
            </div>
          </div>

          <div className="mt-1.5">
            <SearchInput
              value={presetSearch}
              onValueChange={setPresetSearch}
              onClear={() => setPresetSearch("")}
              clearLabel={t("connections.filtersPanel.clearSavedFiltersSearch")}
              placeholder={t("connections.filtersPanel.savedFiltersSearch")}
              className="h-9 rounded-md border-input/60 bg-background text-sm"
            />
          </div>

          <div className="mt-1.5 max-h-36 space-y-1.5 overflow-y-auto pr-0.5">
            {renderPresetList(filteredPrivatePresets, "private")}
            {renderPresetList(filteredPublicPresets, "public")}
            {!presetsLoading && totalPresetCount === 0 && (
              <p className="px-1 py-0.5 text-xs text-muted-foreground/70">
                {t("connections.filtersPanel.noSavedFilters")}
              </p>
            )}
            {!presetsLoading && totalPresetCount > 0 && visiblePresetCount === 0 && (
              <p className="px-1 py-0.5 text-center text-xs text-muted-foreground/70">
                {t("noResults")}
              </p>
            )}
          </div>
        </div>

        {groups.map((group) => {
          const isExpanded = expanded.includes(group.key);
          const groupSelected = draft[group.key]?.size ?? 0;
          const filteredOpts = getFilteredOptions(group);
          const showSearch = group.options.length >= SEARCH_THRESHOLD;
          const isEmpty = group.options.length === 0;

          return (
            <div key={group.key} className="rounded-md">
              <button
                type="button"
                onClick={() => !isEmpty && toggle(group.key)}
                disabled={isEmpty}
                className={cn(
                  "flex items-center justify-between w-full px-2 py-1.5 text-xs font-medium rounded-md transition-colors",
                  isEmpty
                    ? "text-muted-foreground/40 cursor-not-allowed"
                    : "text-foreground hover:bg-accent",
                )}
              >
                <div className="flex items-center gap-1.5">
                  <span>{group.label}</span>
                  {groupSelected > 0 && (
                    <span className="flex items-center justify-center min-w-[14px] h-3.5 rounded-full bg-primary/15 text-primary text-xs font-semibold px-1">
                      {groupSelected}
                    </span>
                  )}
                </div>
                <div className="flex items-center gap-1">
                  <span className="text-xs text-muted-foreground/60">{group.options.length}</span>
                  {!isEmpty &&
                    (isExpanded ? (
                      <ChevronUp className="h-3 w-3 text-muted-foreground" />
                    ) : (
                      <ChevronDown className="h-3 w-3 text-muted-foreground" />
                    ))}
                </div>
              </button>

              {isExpanded && !isEmpty && (
                <div className="pb-1.5">
                  {showSearch && (
                    <div className="px-2 pb-1.5 pt-0.5">
                      <SearchInput
                        value={searches[group.key] ?? ""}
                        onValueChange={(value) =>
                          setSearches((prev) => ({ ...prev, [group.key]: value }))
                        }
                        placeholder={`${group.label}...`}
                        className="h-9 text-sm bg-accent border-input/60 rounded-md"
                      />
                    </div>
                  )}

                  {filteredOpts.length > 3 && (
                    <div className="px-3 pb-1 flex items-center justify-between">
                      <button
                        type="button"
                        className="text-xs text-primary/70 hover:text-primary transition-colors"
                        onClick={() => {
                          const allSelected = filteredOpts.every((o) =>
                            draft[group.key]?.has(o),
                          );
                          if (allSelected) {
                            clearGroup(group.key);
                          } else {
                            setDraft((prev) => {
                              const next = cloneFilterState(prev);
                              const s = new Set(next[group.key]);
                              for (const o of filteredOpts) s.add(o);
                              next[group.key] = s;
                              return next;
                            });
                          }
                        }}
                      >
                        {filteredOpts.every((o) => draft[group.key]?.has(o))
                          ? t("connections.filtersPanel.clearAll")
                          : t("selectAll")}
                      </button>
                      {groupSelected > 0 && (
                        <button
                          type="button"
                          className="text-xs text-muted-foreground hover:text-foreground transition-colors"
                          onClick={() => clearGroup(group.key)}
                        >
                          {t("connections.filtersPanel.clearAll")}
                        </button>
                      )}
                    </div>
                  )}

                  <div
                    className={cn(
                      "px-2 space-y-px",
                      showSearch && filteredOpts.length > 8 && "max-h-44 overflow-y-auto",
                    )}
                  >
                    {filteredOpts.length === 0 ? (
                      <p className="text-xs text-muted-foreground/60 text-center py-2">
                        {t("noResults")}
                      </p>
                    ) : (
                      filteredOpts.map((opt) => {
                        const isChecked = draft[group.key]?.has(opt) ?? false;
                        return (
                          <button
                            type="button"
                            key={opt}
                            onClick={() => toggleOption(group.key, opt)}
                            className={cn(
                              "flex items-center gap-2 w-full px-1.5 py-1 text-xs rounded-md transition-all text-left group",
                              isChecked
                                ? "bg-primary/8 text-foreground"
                                : "text-muted-foreground hover:text-foreground hover:bg-accent",
                            )}
                          >
                            <div
                              className={cn(
                                "w-3.5 h-3.5 rounded border flex items-center justify-center shrink-0 transition-all",
                                isChecked
                                  ? "bg-primary border-primary"
                                  : "border-input/30 group-hover:border-foreground/30",
                              )}
                            >
                              {isChecked && <Check className="h-2.5 w-2.5 text-primary-foreground" />}
                            </div>
                            <span className="truncate flex-1">{formatFilterOption(group.key, opt)}</span>
                          </button>
                        );
                      })
                    )}
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* Responsive actions. On phones the count line stacks above the
          controls, Reset takes its own full-width row (its Russian label is the
          longest) and Cancel/Apply share the next, so every action is at least a
          44px touch target and none escapes the dialog. The row re-flows at
          `sm`, but the dense 32px height only returns at `md` — 768px, the app's
          `use-mobile` boundary — so 640–767px is still a phone. This governs the
          filters footer only; other fixed-height controls are unchanged. */}
      <div className="flex flex-col gap-2 border-t border-border/30 px-3 py-2 sm:flex-row sm:items-center sm:justify-between">
        <p className="min-w-0 truncate text-xs text-muted-foreground">
          {totalSelected > 0
            ? t("connections.filtersPanel.selectedCount", { count: totalSelected })
            : t("connections.filtersPanel.noneSelected")}
        </p>
        <div className="flex w-full flex-wrap items-center gap-1.5 sm:w-auto sm:flex-nowrap sm:shrink-0">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-11 w-full text-sm md:h-8 sm:w-auto sm:flex-none sm:text-xs"
            onClick={() => {
              setDraft(emptyFilterState());
              setPendingPreset(null);
            }}
          >
            {t("connections.filtersPanel.reset")}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-11 flex-1 text-sm md:h-8 sm:flex-none sm:text-xs"
            onClick={onClose}
          >
            {t("cancel")}
          </Button>
          <Button
            type="button"
            size="sm"
            className="h-11 flex-1 text-sm md:h-8 sm:flex-none sm:text-xs"
            disabled={!dirty}
            onClick={apply}
          >
            {t("connections.filtersPanel.apply")}
          </Button>
        </div>
      </div>
    </SurfaceCard>
  );
};

export default ConnectionsFilters;
