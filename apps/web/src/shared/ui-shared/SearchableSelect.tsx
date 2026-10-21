import { useMemo, useState, type ComponentPropsWithRef } from "react";
import type { LucideIcon } from "lucide-react";
import { Check, ChevronsUpDown, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/shared/lib/utils";
import { Button } from "@/shared/ui-primitives/button";
import { CommandGroup, CommandItem } from "@/shared/ui-primitives/command";
import {
  CommandPickerContent,
  CommandPickerInline,
} from "@/shared/ui-shared/CommandPicker";

export interface SearchableSelectOption<TId extends string = string> {
  id: TId;
  label: string;
  icon?: LucideIcon;
  searchText?: string;
  disabled?: boolean;
}

export type SearchableSelectControlProps = Omit<
  ComponentPropsWithRef<"button">,
  "value" | "onChange" | "children" | "type" | "role" | "aria-expanded"
>;

export interface SearchableSelectProps<TId extends string = string> extends SearchableSelectControlProps {
  items: SearchableSelectOption<TId>[];
  value: TId | "";
  onValueChange: (id: TId) => void;
  placeholder?: string;
  selectedLabel?: string | null;
  searchPlaceholder?: string;
  emptyText?: string;
  loadingText?: string;
  isLoading?: boolean;
  shouldFilter?: boolean;
  searchValue?: string;
  onSearchValueChange?: (value: string) => void;
  onOpenChange?: (open: boolean) => void;
  triggerClassName?: string;
  contentClassName?: string;
  listClassName?: string;
  /** "overlay" absolutely positions the panel (page/toolbar hosts where push-down
   * would shift a table); "block" (default) renders inline below the trigger. */
  panelPlacement?: "block" | "overlay";
}

function selectedFallback(value: string): string {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)
    ? `${value.slice(0, 8)}...`
    : value;
}

const SearchableSelect = <TId extends string = string>({
  id,
  items,
  value,
  onValueChange,
  placeholder,
  selectedLabel,
  searchPlaceholder,
  emptyText,
  loadingText,
  disabled,
  isLoading,
  shouldFilter = true,
  searchValue,
  onSearchValueChange,
  onOpenChange,
  triggerClassName,
  contentClassName,
  listClassName,
  panelPlacement = "block",
  className,
  onClick,
  ...rest
}: SearchableSelectProps<TId>) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [localSearch, setLocalSearch] = useState("");

  const selected = useMemo(
    () => items.find((item) => item.id === value),
    [items, value],
  );
  const effectiveSearch = searchValue ?? localSearch;
  const displayLabel = selected?.label ?? selectedLabel ?? (value ? selectedFallback(value) : null);

  const updateOpen = (next: boolean) => {
    setOpen(next);
    onOpenChange?.(next);
    if (!next && searchValue === undefined) setLocalSearch("");
  };

  const updateSearch = (next: string) => {
    if (onSearchValueChange) onSearchValueChange(next);
    else setLocalSearch(next);
  };

  const trigger = (
    <Button
      {...rest}
      type="button"
      id={id}
      variant="outline"
      role="combobox"
      aria-expanded={open}
      disabled={disabled}
      className={cn(
        "h-9 w-full min-w-0 justify-between gap-2 px-3 text-sm font-normal",
        !displayLabel && "text-muted-foreground",
        triggerClassName,
        className,
      )}
      onClick={(event) => {
        onClick?.(event);
        if (!event.defaultPrevented) updateOpen(!open);
      }}
    >
      <span className="inline-flex min-w-0 flex-1 items-center gap-1.5 text-left">
        {selected?.icon ? (
          <selected.icon className="h-3 w-3 shrink-0 text-muted-foreground" />
        ) : null}
        <span className="truncate">
          {displayLabel ?? placeholder ?? t("select")}
        </span>
      </span>
      {isLoading ? (
        <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin opacity-70" />
      ) : (
        <ChevronsUpDown className="h-3.5 w-3.5 shrink-0 opacity-50" />
      )}
    </Button>
  );

  return (
    <CommandPickerInline
      open={open}
      onOpenChange={updateOpen}
      trigger={trigger}
      panelClassName={contentClassName}
      placement={panelPlacement}
    >
      <CommandPickerContent
        shouldFilter={shouldFilter}
        searchValue={effectiveSearch}
        onSearchValueChange={updateSearch}
        searchPlaceholder={searchPlaceholder ?? t("search")}
        emptyText={emptyText ?? t("noResults")}
        isLoading={isLoading}
        loadingText={loadingText ?? t("loading")}
        listClassName={listClassName}
        inputAutoFocus
      >
        <CommandGroup>
          {items.map((item) => {
            const isSelected = item.id === value;
            return (
              <CommandItem
                key={item.id}
                value={[item.label, item.id, item.searchText].filter(Boolean).join(" ")}
                disabled={item.disabled}
                onSelect={() => {
                  onValueChange(item.id);
                  updateOpen(false);
                }}
                className="text-sm"
              >
                <Check
                  className={cn(
                    "mr-2 h-3.5 w-3.5",
                    isSelected ? "opacity-100" : "opacity-0",
                  )}
                />
                <span className="inline-flex min-w-0 items-center gap-1.5">
                  {item.icon ? (
                    <item.icon className="h-3 w-3 shrink-0 text-muted-foreground" />
                  ) : null}
                  <span className="truncate">{item.label}</span>
                </span>
              </CommandItem>
            );
          })}
        </CommandGroup>
      </CommandPickerContent>
    </CommandPickerInline>
  );
};

export default SearchableSelect;
