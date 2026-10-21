import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronsUpDown } from "lucide-react";

import { cn } from "@/shared/lib/utils";
import { Button } from "@/shared/ui-primitives/button";
import { CommandGroup, CommandItem } from "@/shared/ui-primitives/command";
import { CommandPickerContent, CommandPickerInline } from "@/shared/ui-shared/CommandPicker";

import FormField from "@/shared/ui-shared/FormField";

interface FieldPathAutocompleteProps {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  suggestions?: string[];
  disabled?: boolean;
  required?: boolean;
  error?: string;
  placeholder?: string;
}

const FieldPathAutocomplete = ({
  id,
  label,
  value,
  onChange,
  suggestions = [],
  disabled,
  required,
  error,
  placeholder = "name",
}: FieldPathAutocompleteProps) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState(value);

  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return suggestions
      .filter((path) => path !== value)
      .filter((path) => !needle || path.toLowerCase().includes(needle))
      .slice(0, 24);
  }, [search, suggestions, value]);

  const trigger = (
    <Button
      type="button"
      id={id}
      variant="outline"
      role="combobox"
      aria-expanded={open && !disabled}
      disabled={disabled}
      className="mt-1 h-9 w-full justify-between gap-2 border-input/30 px-2 text-left font-mono text-sm font-normal"
      onClick={() => {
        setSearch(value);
        setOpen((current) => !current);
      }}
    >
      <span className={cn("min-w-0 flex-1 truncate", !value && "text-muted-foreground")}>
        {value || placeholder}
      </span>
      <ChevronsUpDown className="h-3 w-3 shrink-0 opacity-50" />
    </Button>
  );

  return (
    <FormField htmlFor={id} label={label} required={required} error={error}>
      <CommandPickerInline open={open && !disabled} onOpenChange={setOpen} trigger={trigger}>
        <CommandPickerContent
          shouldFilter={false}
          commandClassName="rounded-none bg-transparent"
          inputAutoFocus
          inputClassName="h-9 font-mono text-sm"
          listClassName="max-h-36"
          searchValue={search}
          onSearchValueChange={(next) => {
            setSearch(next);
            onChange(next);
          }}
          searchPlaceholder={t("sync.mapping.args.fieldSuggestions", "Field suggestions")}
          emptyText={t("noResults")}
        >
          <CommandGroup>
            {filtered.map((path) => (
              <CommandItem
                key={path}
                value={path}
                className="min-h-9 gap-2 font-mono text-sm"
                onSelect={() => {
                  onChange(path);
                  setSearch(path);
                  setOpen(false);
                }}
              >
                <Check className={cn("h-3 w-3 shrink-0", value === path ? "opacity-100" : "opacity-0")} />
                <span className="min-w-0 truncate">{path}</span>
              </CommandItem>
            ))}
          </CommandGroup>
        </CommandPickerContent>
      </CommandPickerInline>
    </FormField>
  );
};

export default FieldPathAutocomplete;
