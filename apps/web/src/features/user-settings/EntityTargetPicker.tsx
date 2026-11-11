import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Check, ChevronDown, X } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import {
  CommandGroup,
  CommandItem,
  CommandSeparator,
} from "@/shared/ui-primitives/command";
import { cn } from "@/shared/lib/utils";
import {
  CommandPickerContent,
  CommandPickerEmptyRow,
  CommandPickerLoading,
} from "@/shared/ui-shared/CommandPicker";
import {
  entityIdFromPattern,
  getEntitySource,
  isAllPattern,
  patternForEntity,
  type EntityOption,
} from "@/features/user-settings/entitySources";

interface EntityTargetPickerProps {
  /** Catalog resource type for the scope, e.g. `connection_target`, `user`, `config_context`. */
  resourceType: string;
  /** Current resource patterns for this scope: `["*"]` or `["connection_target:id", …]`. */
  value: string[];
  /** Emit the new pattern set. Empty selection collapses to `["*"]`. */
  onChange: (patterns: string[]) => void;
}

const SEARCH_LIMIT = 50;
const DEBOUNCE_MS = 250;

/**
 * Multi-select picker that narrows a token scope to specific materialized
 * entities (by name, not raw UUID). Resource types with thousands of rows are
 * searched server-side; the rest are filtered locally. Selecting entities
 * emits `<prefix>:<id>` patterns; "All resources" emits `["*"]`.
 *
 * The search list renders INLINE (expand/collapse) rather than in a floating
 * popover: a portaled popover inside a Radix Dialog sits outside the dialog's
 * scroll-lock subtree, so its list can't be wheel-scrolled and the floating box
 * drifts over the form. Inline keeps the list inside the dialog's scroll region
 * where the wheel works and nothing overflows.
 *
 * Resource types without an entity source (e.g. audit events) always apply to
 * all resources and render a static note instead of a picker.
 */
const EntityTargetPicker = ({ resourceType, value, onChange }: EntityTargetPickerProps) => {
  const { t } = useTranslation();
  const source = getEntitySource(resourceType);
  const [open, setOpen] = useState(false);
  const [term, setTerm] = useState("");
  const [debounced, setDebounced] = useState("");
  const debounceTimer = useRef<number | undefined>(undefined);
  // id → name cache, populated as the user selects from search results so the
  // selected chips keep their names even after the search term changes. Kept in
  // state (not a ref) so chips re-render and we never read a ref during render.
  const [nameCache, setNameCache] = useState<Map<string, string>>(new Map());

  const prefix = source?.prefix ?? "";
  const all = isAllPattern(value);
  const selectedIds = useMemo(() => {
    if (!source || all) return [] as string[];
    return value
      .map((p) => entityIdFromPattern(prefix, p))
      .filter((id): id is string => id !== null);
  }, [value, prefix, source, all]);

  const optionsQuery = useQuery({
    queryKey: ["entity-target", resourceType, source?.serverSearch ? debounced : ""],
    queryFn: () => source!.search(source!.serverSearch ? debounced : "", SEARCH_LIMIT),
    enabled: open && !!source,
    staleTime: 30_000,
  });

  const options = useMemo<EntityOption[]>(() => {
    const data = optionsQuery.data ?? [];
    if (source?.serverSearch) return data;
    // Client-side filter for sources that load a bounded set.
    const needle = term.trim().toLowerCase();
    if (!needle) return data;
    return data.filter((o) =>
      [o.name, o.id].some((part) => part.toLowerCase().includes(needle)),
    );
  }, [optionsQuery.data, source, term]);

  const handleTermChange = (next: string) => {
    setTerm(next);
    if (debounceTimer.current) window.clearTimeout(debounceTimer.current);
    debounceTimer.current = window.setTimeout(() => setDebounced(next), DEBOUNCE_MS);
  };

  const toggleOpen = () => {
    const next = !open;
    setOpen(next);
    if (!next) {
      setTerm("");
      setDebounced("");
    }
  };

  const emit = (ids: string[]) => {
    onChange(ids.length ? ids.map((id) => patternForEntity(prefix, id)) : ["*"]);
  };

  const toggleEntity = (opt: EntityOption) => {
    setNameCache((prev) => new Map(prev).set(opt.id, opt.name));
    const set = new Set(selectedIds);
    if (set.has(opt.id)) set.delete(opt.id);
    else set.add(opt.id);
    emit([...set]);
  };

  const selectAll = () => onChange(["*"]);
  const removeId = (id: string) => emit(selectedIds.filter((s) => s !== id));
  const nameFor = (id: string) => nameCache.get(id) ?? id;

  // Resource types without a backing entity list always apply to all resources.
  if (!source) {
    return (
      <p className="text-xs text-muted-foreground/80">
        {t("userSettings.targetAllResources")}
      </p>
    );
  }

  return (
    <div className="space-y-1.5">
      <Button
        type="button"
        variant="outline"
        aria-expanded={open}
        onClick={toggleOpen}
        className="h-9 w-full justify-between gap-1 px-2 text-sm font-normal"
      >
        <span className={cn("min-w-0 flex-1 truncate text-left", all && "text-muted-foreground")}>
          {all
            ? t("userSettings.targetAllResources")
            : t("userSettings.targetSelectedCount", { count: selectedIds.length })}
        </span>
        <ChevronDown
          className={cn("h-3.5 w-3.5 shrink-0 opacity-50 transition-transform", open && "rotate-180")}
        />
      </Button>

      {open && (
        <div className="overflow-hidden rounded-md border border-border/40 bg-card">
          <CommandPickerContent
            shouldFilter={false}
            commandClassName="bg-transparent"
            inputAutoFocus
            inputClassName="text-sm"
            listClassName="max-h-[220px]"
            searchValue={term}
            onSearchValueChange={handleTermChange}
            searchPlaceholder={t("userSettings.targetSearchPlaceholder")}
            emptyText={t("userSettings.targetNoObjects")}
          >
            <CommandGroup>
              <CommandItem value="__all__" onSelect={selectAll} className="text-sm">
                <Check className={cn("mr-2 h-3.5 w-3.5", all ? "opacity-100" : "opacity-0")} />
                {t("userSettings.targetAllResources")}
              </CommandItem>
            </CommandGroup>
            <CommandSeparator />
            <CommandGroup heading={t("userSettings.targetSpecificObject")}>
              {optionsQuery.isPending ? (
                <CommandPickerLoading>{t("loading")}</CommandPickerLoading>
              ) : options.length === 0 ? (
                <CommandPickerEmptyRow>{t("userSettings.targetNoObjects")}</CommandPickerEmptyRow>
              ) : (
                options.map((opt) => {
                  const selected = selectedIds.includes(opt.id);
                  return (
                    <CommandItem
                      key={opt.id}
                      value={`${opt.name} ${opt.id}`}
                      onSelect={() => toggleEntity(opt)}
                      className="text-sm"
                    >
                      <span
                        className={cn(
                          "mr-2 flex h-3.5 w-3.5 items-center justify-center rounded-[4px] border",
                          selected
                            ? "border-primary bg-primary text-primary-foreground"
                            : "border-input/30",
                        )}
                      >
                        {selected && <Check className="h-2.5 w-2.5" />}
                      </span>
                      <span className="min-w-0 flex-1 truncate">{opt.name}</span>
                    </CommandItem>
                  );
                })
              )}
            </CommandGroup>
          </CommandPickerContent>
        </div>
      )}

      {!all && selectedIds.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {selectedIds.map((id) => (
            <span
              key={id}
              className="inline-flex max-w-[12rem] items-center gap-1 rounded-md border border-border/40 bg-card px-1.5 py-0.5 text-xs text-foreground"
            >
              <span className="min-w-0 truncate">{nameFor(id)}</span>
              <Button variant="ghost" size="sm"
                type="button"
                onClick={() => removeId(id)}
                className="size-6 shrink-0 p-0 text-muted-foreground hover:text-foreground"
                aria-label={t("userSettings.removeScope")}
              >
                <X className="h-3 w-3" />
              </Button>
            </span>
          ))}
        </div>
      )}
    </div>
  );
};

export default EntityTargetPicker;
