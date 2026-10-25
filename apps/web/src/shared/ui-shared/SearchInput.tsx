import { forwardRef, type ComponentPropsWithoutRef } from "react";
import { Search, X } from "lucide-react";
import { Input } from "@/shared/ui-primitives/input";
import { cn } from "@/shared/lib/utils";

export interface SearchInputProps
  extends Omit<ComponentPropsWithoutRef<typeof Input>, "value" | "onChange" | "type"> {
  value: string;
  onValueChange: (value: string) => void;
  /** When provided AND the field is non-empty, renders a trailing clear button. */
  onClear?: () => void;
  /** Wrapper styling (e.g. `flex-1 max-w-xs`). The wrapper is always `relative`. */
  containerClassName?: string;
  /** aria-label for the trailing clear button. */
  clearLabel?: string;
}

/**
 * The single search-field primitive: a leading magnifier icon over an
 * {@link Input}, with an optional trailing clear button. Replaces the
 * hand-rolled `relative` + absolutely-positioned `<Search/>` + padded `<Input/>`
 * trio duplicated across ~two dozen list toolbars.
 *
 * Debounce, page-reset and the search state stay at the call site.
 */
const SearchInput = forwardRef<HTMLInputElement, SearchInputProps>(
  ({ value, onValueChange, onClear, containerClassName, clearLabel, className, placeholder, "aria-label": ariaLabel, ...rest }, ref) => {
    const showClear = !!onClear && value.length > 0;
    return (
      <div data-slot="search-input" className={cn("relative min-w-0 w-full max-w-full sm:w-72", containerClassName)}>
        <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          ref={ref}
          // type="text" (not "search"): matches every call site we replaced and
          // avoids the native WebKit cancel button (a second × next to onClear)
          // plus the in-dialog Escape-clears-field-instead-of-closing behaviour.
          type="text"
          placeholder={placeholder}
          aria-label={ariaLabel ?? placeholder}
          value={value}
          onChange={(event) => onValueChange(event.target.value)}
          className={cn(
            "h-9 border-input bg-card pl-9 text-sm",
            showClear && "pr-9",
            className,
          )}
          {...rest}
        />
        {showClear && (
          <button
            type="button"
            aria-label={clearLabel ?? "Clear"}
            onClick={onClear}
            className="absolute right-1.5 top-1/2 flex size-6 -translate-y-1/2 items-center justify-center rounded text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        )}
      </div>
    );
  },
);
SearchInput.displayName = "SearchInput";

export default SearchInput;
