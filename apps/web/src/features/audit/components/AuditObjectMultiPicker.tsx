import { useEffect, useState, type ComponentType } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Check, ChevronsUpDown, X } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { CommandGroup, CommandItem } from "@/shared/ui-primitives/command";
import {
  CommandPickerContent,
  CommandPickerInline,
  CommandPickerLoading,
} from "@/shared/ui-shared/CommandPicker";
import { searchAuditTargets } from "@/features/audit/api";
import type { AuditLookupOption } from "@/shared/api/types";

/** Stable identity for a hydrated lookup option (`target_type:id`). */
function optionKey(option: AuditLookupOption): string {
  return `${option.target_type}:${option.id}`;
}

export interface AuditObjectMultiPickerProps {
  /** Currently selected lookup options. */
  value: AuditLookupOption[];
  /** Receives the next full selection set. */
  onChange: (next: AuditLookupOption[]) => void;
  /** Scope the server lookup to a single target type (omit to search all). */
  targetType?: string;
  placeholder: string;
  searchPlaceholder: string;
  emptyText: string;
  loadingText: string;
  clearText: string;
  groupLabel: string;
  /** Label a selected option (e.g. localize its kind for the chip subtitle). */
  kindLabel: (kind?: string) => string;
  icon?: ComponentType<{ className?: string }>;
  className?: string;
  disabled?: boolean;
}

/**
 * Multi-value variant of the audit object picker: an async, server-hydrated
 * lookup (users/targets) that accumulates a selection set instead of a single
 * value. Selected options render as removable chips below the trigger and stay
 * selected across searches (the query only drives the option list, never the
 * selection). Mirrors {@link MultiSelect}'s chip/trigger shape for consistency.
 */
export function AuditObjectMultiPicker({
  value,
  onChange,
  targetType,
  placeholder,
  searchPlaceholder,
  emptyText,
  loadingText,
  clearText,
  groupLabel,
  kindLabel,
  icon: Icon,
  className,
  disabled = false,
}: Readonly<AuditObjectMultiPickerProps>) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");

  useEffect(() => {
    const timeout = setTimeout(() => setDebouncedSearch(search.trim()), 200);
    return () => clearTimeout(timeout);
  }, [search]);

  const effectiveTargetType = targetType && targetType !== "all" ? targetType : undefined;
  const lookupQuery = useQuery({
    queryKey: ["audit-target-lookup", effectiveTargetType ?? "all", debouncedSearch],
    queryFn: () =>
      searchAuditTargets({
        targetType: effectiveTargetType,
        search: debouncedSearch || undefined,
        limit: 30,
      }),
    enabled: open && !disabled,
    placeholderData: (previous) => previous,
  });
  const options = lookupQuery.data ?? [];
  const selectedKeys = new Set(value.map(optionKey));

  const toggle = (option: AuditLookupOption) => {
    const key = optionKey(option);
    if (selectedKeys.has(key)) {
      onChange(value.filter((item) => optionKey(item) !== key));
    } else {
      onChange([...value, option]);
    }
  };

  const trigger = (
    <>
      <Button
        type="button"
        variant="outline"
        role="combobox"
        aria-expanded={open}
        disabled={disabled}
        className={cn(
          "h-9 w-full justify-between gap-2 px-3 text-left text-sm font-normal",
          value.length > 0 && "border-primary/30 bg-primary/10 text-foreground hover:bg-primary/15",
          value.length === 0 && "text-muted-foreground",
          className,
        )}
        onClick={() => setOpen((current) => !current)}
      >
        {Icon && (
          <Icon
            className={cn(
              "size-4 shrink-0",
              value.length > 0 ? "text-primary" : "text-muted-foreground",
            )}
          />
        )}
        <span className="min-w-0 flex-1 truncate">
          {value.length === 0
            ? placeholder
            : t("multiSelect.selectedCount", { count: value.length })}
        </span>
        <ChevronsUpDown className="size-4 shrink-0 text-muted-foreground" />
      </Button>
      {value.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1">
          {value.map((option) => (
            <Badge
              key={optionKey(option)}
              variant="secondary"
              className="gap-1 pr-1 text-xs font-normal"
            >
              <span className="max-w-[160px] truncate">{option.label}</span>
              {!disabled && (
                <button
                  type="button"
                  onClick={() => toggle(option)}
                  aria-label={t("remove")}
                  className="flex size-6 items-center justify-center rounded-sm outline-none hover:text-destructive focus-visible:ring-1 focus-visible:ring-ring"
                >
                  <X className="size-3.5" />
                </button>
              )}
            </Badge>
          ))}
        </div>
      )}
    </>
  );

  return (
    <CommandPickerInline
      open={open}
      onOpenChange={setOpen}
      trigger={trigger}
      panelClassName="w-[min(24rem,calc(100vw-2rem))]"
      placement="overlay"
    >
      <CommandPickerContent
        shouldFilter={false}
        searchValue={search}
        onSearchValueChange={setSearch}
        searchPlaceholder={searchPlaceholder}
        emptyText={emptyText}
        listClassName="max-h-72"
        inputAutoFocus
      >
        {value.length > 0 && (
          <CommandGroup>
            <CommandItem
              value="__clear__"
              onSelect={() => onChange([])}
              className="gap-2 py-1.5 text-xs text-muted-foreground"
            >
              <X className="size-4" />
              <span>{clearText}</span>
            </CommandItem>
          </CommandGroup>
        )}
        {lookupQuery.isLoading && options.length === 0 ? (
          <CommandPickerLoading>{loadingText}</CommandPickerLoading>
        ) : (
          <CommandGroup heading={groupLabel}>
            {options.map((option) => (
              <CommandItem
                key={optionKey(option)}
                value={`${option.label} ${option.secondary ?? ""}`}
                onSelect={() => toggle(option)}
                className="group gap-2 py-1.5"
              >
                <Check
                  className={cn(
                    "size-4 shrink-0",
                    selectedKeys.has(optionKey(option)) ? "opacity-100" : "opacity-0",
                  )}
                />
                <span className="min-w-0 flex-1 leading-tight">
                   <span className="block truncate text-sm font-medium">{option.label}</span>
                  <span className="mt-0.5 block truncate text-xs text-muted-foreground group-data-[selected=true]:text-accent-foreground/80">
                    {[kindLabel(option.kind), option.secondary].filter(Boolean).join(" · ")}
                  </span>
                </span>
              </CommandItem>
            ))}
          </CommandGroup>
        )}
      </CommandPickerContent>
    </CommandPickerInline>
  );
}
