import type { LucideIcon } from "lucide-react";
import SearchableSelect, { type SearchableSelectControlProps } from "@/shared/ui-shared/SearchableSelect";

export interface EntityOption<TId extends string = string> {
  /**
   * The id that is submitted to the backend. Always a stable id (e.g. UUID).
   *
   * Generic over `TId` so a picker fed branded entity ids (`{ id: role.id }`
   * where `role.id: Uuid`) infers `EntityOption<Uuid>` — and binding a display
   * name there (`{ id: role.name }`) is a compile error at the call site.
   * Defaults to `string` for composite (`kind:uuid`) or sentinel pickers.
   */
  id: TId;
  /** The human-readable label shown to the user. */
  label: string;
  /** Optional type icon (lucide). */
  icon?: LucideIcon;
  /** Extra searchable text such as slug, address, or secondary name. */
  searchText?: string;
  disabled?: boolean;
}

export interface EntitySelectProps<TId extends string = string> extends SearchableSelectControlProps {
  items: EntityOption<TId>[];
  /** Currently selected id (NOT a label); `""` for the no-selection state. */
  value: TId | "";
  /** Receives the selected id. */
  onValueChange: (id: TId) => void;
  placeholder?: string;
  triggerClassName?: string;
  searchPlaceholder?: string;
  emptyText?: string;
}

/**
 * An entity picker that binds the option **id** as the value — never the
 * display name. This makes the "bound a name where the backend expects an id"
 * class of bug impossible by construction: callers pass `{ id, label }` pairs
 * and the component is the single place that decides what gets submitted.
 *
 * Pair with {@link EntityRef} for read-only display of the same entities.
 */
function EntitySelect<TId extends string = string>({
  id,
  items,
  value,
  onValueChange,
  placeholder,
  triggerClassName,
  searchPlaceholder,
  emptyText,
  disabled,
  ...rest
}: EntitySelectProps<TId>) {
  return (
    <SearchableSelect
      {...rest}
      id={id}
      items={items}
      value={value}
      onValueChange={onValueChange}
      placeholder={placeholder}
      searchPlaceholder={searchPlaceholder}
      emptyText={emptyText}
      triggerClassName={triggerClassName}
      disabled={disabled}
    />
  );
}

export default EntitySelect;
