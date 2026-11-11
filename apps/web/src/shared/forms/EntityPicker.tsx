import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronsUpDown, Loader2, Plus } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { Button } from "@/shared/ui-primitives/button";
import { CommandGroup, CommandItem } from "@/shared/ui-primitives/command";
import type { InventoryEntityKey } from "@/shared/api/types";
import { CommandPickerContent, CommandPickerInline } from "@/shared/ui-shared/CommandPicker";

export interface EntityPickerOption {
  id: string;
  name: string;
  color?: string;
  /** Scope of a readable native Tag. Missing metadata is not unrestricted. */
  objectTypes?: string[];
  subtitle?: string;
  searchText?: string;
  /** Owning device of device-scoped resources (Modules, Module Bays). */
  deviceId?: string;
  siteId?: string;
  moduleId?: string | null;
  deviceTypeId?: string | null;
  moduleTypeId?: string | null;
  interfaceType?: string;
  /** DeviceType subdevice role; gates child device-bay creation. */
  subdeviceRole?: string;
  /** Rear/rear port position capacity used to bound port-template mappings. */
  positions?: string | null;
}

interface EntityPickerProps {
  id?: string;
  "aria-invalid"?: boolean;
  "aria-describedby"?: string;
  /** The related entity this picker selects from / creates into. */
  entity?: InventoryEntityKey;
  /** Selected FK-UUID (option id), not the name. */
  value: string;
  /** Called with the selected FK-UUID. */
  onChange: (value: string) => void;
  options: EntityPickerOption[];
  loading?: boolean;
  disabled?: boolean;
  placeholder?: string;
  label: string;
  /**
   * The "+" button delegates creation to the host (it does NOT open its own
   * dialog). The host pushes a new create frame for `entity` onto its stack.
   */
  onRequestCreate?: (entity: InventoryEntityKey) => void;
}

const normalizedOptionName = (option: EntityPickerOption): string => option.name.trim().toLocaleLowerCase();

const keywordsOf = (option: EntityPickerOption): string[] =>
  [option.name, option.subtitle, option.searchText, option.id]
    .filter((part): part is string => Boolean(part && part.trim()));

/** One pass per option list: the duplicate index and the search keywords that
 *  the opened menu renders. Both decorate results only, never the trigger. */
const buildItemModels = (options: EntityPickerOption[]) => {
  const names = options.map(normalizedOptionName);
  const seen = new Set<string>();
  const duplicated = new Set<string>();
  names.forEach((name) => {
    if (seen.has(name)) duplicated.add(name);
    else seen.add(name);
  });
  return options.map((option, index) => ({
    option,
    duplicateName: duplicated.has(names[index]!),
    keywords: keywordsOf(option),
  }));
};

const EntityPicker = ({
  id,
  "aria-invalid": invalid,
  "aria-describedby": describedBy,
  entity,
  value,
  onChange,
  options,
  loading = false,
  disabled = false,
  placeholder,
  label,
  onRequestCreate,
}: EntityPickerProps) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const selected = options.find((o) => o.id === value);
  const placeholderText = placeholder ?? t("inventory.resources.form.relationPlaceholder", { label });
  const searchPlaceholder = t("inventory.resources.form.searchRelation", { label });
  // Disambiguation and search keywords decorate the opened menu only; a closed
  // trigger needs the selected name alone, so neither is built before opening.
  const itemModels = useMemo(
    () => (open ? buildItemModels(options) : null),
    [open, options],
  );

  const trigger = (
    <Button
      id={id}
      aria-invalid={invalid}
      aria-describedby={describedBy}
      type="button"
      variant="field"
      role="combobox"
      aria-label={label}
      aria-expanded={open}
      disabled={disabled || loading}
      className="h-9 w-full min-w-0 justify-between px-3 text-left text-sm font-normal"
      onClick={() => setOpen((current) => !current)}
      title={selected ? keywordsOf(selected).join(" · ") : placeholderText}
    >
      <span className={cn("min-w-0 truncate", !selected && "text-muted-foreground")}>
        {selected?.name ?? placeholderText}
      </span>
      {loading ? (
        <Loader2 className="ml-2 h-3.5 w-3.5 shrink-0 animate-spin opacity-70" />
      ) : (
        <ChevronsUpDown className="ml-2 h-3.5 w-3.5 shrink-0 opacity-50" />
      )}
    </Button>
  );

  return (
    <div className={cn("grid gap-2", onRequestCreate && !disabled && "grid-cols-[minmax(0,1fr)_2.25rem]")} data-entity-picker-root="">
      <div className="min-w-0">
        <CommandPickerInline open={open} onOpenChange={setOpen} trigger={trigger}>
          <CommandPickerContent
            description={entity ? t(`inventory.resources.hints.entities.${entity}`) : label}
            commandClassName="rounded-none bg-transparent"
            inputAutoFocus
            listClassName="max-h-72"
            searchPlaceholder={searchPlaceholder}
            emptyText={t("noResults")}
            isLoading={loading}
            loadingText={t("loading")}
          >
            <CommandGroup>
              {itemModels?.map(({ option, duplicateName, keywords }) => (
                <EntityPickerItem
                  key={option.id}
                  option={option}
                  selected={option.id === value}
                  duplicateName={duplicateName}
                  keywords={keywords}
                  onSelect={() => {
                    onChange(option.id);
                    setOpen(false);
                  }}
                />
              ))}
            </CommandGroup>
          </CommandPickerContent>
        </CommandPickerInline>
      </div>
      {onRequestCreate && entity && !disabled && <Button
        type="button"
        variant="outline"
        size="icon"
        className="shrink-0"
        onClick={() => {
          setOpen(false);
          onRequestCreate(entity);
        }}
        disabled={disabled}
        title={t("inventory.resources.form.createNew")}
        aria-label={`${t("inventory.resources.form.createNew")} ${label}`}
      >
        <Plus className="h-3.5 w-3.5" />
      </Button>}
    </div>
  );
};

interface EntityPickerItemProps {
  option: EntityPickerOption;
  selected: boolean;
  duplicateName: boolean;
  keywords: string[];
  onSelect: () => void;
}

const EntityPickerItem = ({
  option,
  selected,
  duplicateName,
  keywords,
  onSelect,
}: EntityPickerItemProps) => {
  const duplicateSuffix = `#${option.id.slice(-6)}`;
  const subtitle = duplicateName
    ? compactDisplayParts(option.subtitle, duplicateSuffix)
    : option.subtitle;

  return (
    <CommandItem
      value={option.id}
      keywords={keywords}
      className="min-h-9 gap-2 text-sm"
      onSelect={onSelect}
    >
      <Check
        className={cn(
          "h-3.5 w-3.5 shrink-0",
          selected ? "opacity-100" : "opacity-0",
        )}
      />
      <span className="min-w-0 flex-1">
        <span className="block truncate">{option.name}</span>
        {subtitle && (
          <span className="block truncate text-xs text-muted-foreground">
            {subtitle}
          </span>
        )}
      </span>
    </CommandItem>
  );
};

const compactDisplayParts = (...parts: Array<string | undefined>): string | undefined => {
  const compacted = parts
    .map((part) => part?.trim())
    .filter((part): part is string => Boolean(part));
  return compacted.length > 0 ? compacted.join(" · ") : undefined;
};

export default EntityPicker;
