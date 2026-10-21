import { useState } from "react";
import { Check, ChevronsUpDown, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/shared/lib/utils";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { CommandGroup, CommandItem } from "@/shared/ui-primitives/command";
import {
  CommandPickerContent,
  CommandPickerInline,
} from "@/shared/ui-shared/CommandPicker";
import type { EntityOption } from "@/shared/ui-shared/EntitySelect";

export interface MultiSelectProps<TId extends string = string> {
  items: EntityOption<TId>[];
  /** Currently selected ids (NOT labels). */
  value: TId[];
  /** Receives the next full set of selected ids. */
  onValueChange: (ids: TId[]) => void;
  placeholder?: string;
  searchPlaceholder?: string;
  emptyText?: string;
  disabled?: boolean;
  triggerClassName?: string;
  "aria-label"?: string;
  description?: string;
}

/**
 * A multi-value entity picker that binds option **ids** as the value — never the
 * display label. The same id-binding-by-construction guarantee as
 * {@link EntitySelect}: callers pass `{ id, label }` pairs and the component is
 * the single place that decides which ids are submitted. Selected ids render as
 * removable chips; the popover is a searchable, toggleable command list.
 */
function MultiSelect<TId extends string = string>({
  items,
  value,
  onValueChange,
  placeholder,
  searchPlaceholder,
  emptyText,
  disabled,
  triggerClassName,
  description,
  ...rest
}: MultiSelectProps<TId>) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const selected = new Set<TId>(value);

  const toggle = (id: TId) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    onValueChange([...next]);
  };

  const remove = (id: TId) => {
    onValueChange(value.filter((v) => v !== id));
  };

  const selectedItems = items.filter((item) => selected.has(item.id));

  // Selected chips render BELOW the trigger, inside CommandPickerInline's
  // `trigger` slot (not as a sibling after it) so they stay inside
  // `data-picker-root`. The outside-pointerdown close handler only ignores
  // clicks inside that root — chips living outside it would close the panel
  // on every chip removal.
  const trigger = (
    <>
      <Button
        type="button"
        variant="outline"
        role="combobox"
        aria-expanded={open}
        aria-label={rest["aria-label"]}
        disabled={disabled}
        className={cn(
          "h-9 w-full min-w-0 justify-between text-sm font-normal",
          value.length === 0 && "text-muted-foreground",
          triggerClassName,
        )}
        onClick={() => setOpen((current) => !current)}
      >
        <span className="truncate">
          {value.length === 0
            ? (placeholder ?? t("select"))
            : t("multiSelect.selectedCount", { count: value.length })}
        </span>
        <ChevronsUpDown className="ml-2 h-3.5 w-3.5 shrink-0 opacity-50" />
      </Button>
      {selectedItems.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1">
          {selectedItems.map((item) => (
            <Badge key={item.id} variant="secondary" className="gap-1 pr-1 text-xs font-normal">
              <span className="truncate max-w-[160px]">{item.label}</span>
              {!disabled && (
                <button
                  type="button"
                  onClick={() => remove(item.id)}
                   aria-label={`${t("remove")} ${item.label}`}
                   className="flex size-6 items-center justify-center rounded-sm outline-none hover:text-destructive focus-visible:ring-1 focus-visible:ring-ring"
                >
                  <X className="h-3 w-3" />
                </button>
              )}
            </Badge>
          ))}
        </div>
      )}
    </>
  );

  return (
    <CommandPickerInline open={open} onOpenChange={setOpen} trigger={trigger}>
      <CommandPickerContent
        description={description}
        searchPlaceholder={searchPlaceholder ?? t("search")}
        emptyText={emptyText ?? t("noResults")}
        inputAutoFocus
      >
        <CommandGroup>
          {items.map((item) => {
            const isSelected = selected.has(item.id);
            return (
              <CommandItem
                key={item.id}
                value={[item.label, item.id, item.searchText].filter(Boolean).join(" ")}
                onSelect={() => toggle(item.id)}
                className="text-sm"
              >
                <Check
                  className={cn(
                    "mr-2 h-3.5 w-3.5",
                    isSelected ? "opacity-100" : "opacity-0",
                  )}
                />
                <span className="inline-flex min-w-0 items-center gap-1.5">
                  {item.icon && (
                    <item.icon className="h-3 w-3 shrink-0 text-muted-foreground" />
                  )}
                  <span className="truncate">{item.label}</span>
                </span>
              </CommandItem>
            );
          })}
        </CommandGroup>
      </CommandPickerContent>
    </CommandPickerInline>
  );
}

export default MultiSelect;
