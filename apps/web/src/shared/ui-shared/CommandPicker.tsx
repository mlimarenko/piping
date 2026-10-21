import { useEffect, useId, useRef, type ReactNode } from "react";
import { Loader2 } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandList,
} from "@/shared/ui-primitives/command";

interface CommandPickerInlineProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Caller-owned trigger; the caller wires its onClick to toggle `open`. */
  trigger: ReactNode;
  children: ReactNode;
  /** Width/max-w overrides for the panel. */
  panelClassName?: string;
  /** "overlay" absolutely positions the panel over following content (page/toolbar
   * hosts where push-down would shift a table). "block" (default) renders the panel
   * inline in normal flow, below the trigger. */
  placement?: "block" | "overlay";
}

/**
 * The canonical searchable-dropdown panel: rendered INLINE, never portaled. A
 * portaled panel (the old `CommandPickerPopover`) lives outside a modal Radix
 * Dialog's `react-remove-scroll` lock tree, so the lock's non-passive
 * document-level wheel handler swallows scroll events over the panel (visible
 * scrollbar, dead wheel). Rendering inline keeps the panel inside the dialog's
 * scroll-lock subtree, exactly like `src/shared/forms/EntityPicker.tsx`.
 */
export const CommandPickerInline = ({
  open,
  onOpenChange,
  trigger,
  children,
  panelClassName,
  placement = "block",
}: CommandPickerInlineProps) => {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const wasOpenRef = useRef(false);

  useEffect(() => {
    if (!open) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      onOpenChange(false);
    };

    const handlePointerDown = (event: PointerEvent) => {
      const root = rootRef.current;
      if (!root || !event.target) return;
      if (!root.contains(event.target as Node)) {
        onOpenChange(false);
      }
    };

    window.addEventListener("keydown", handleKeyDown, { capture: true });
    document.addEventListener("pointerdown", handlePointerDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown, { capture: true });
      document.removeEventListener("pointerdown", handlePointerDown);
    };
  }, [open, onOpenChange]);

  // Return focus to the trigger when the panel closes (Radix Popover did this
  // for free; an inline panel must do it manually). Only fires on a genuine
  // open->close transition, not on initial mount.
  useEffect(() => {
    if (open) {
      wasOpenRef.current = true;
      return;
    }
    if (!wasOpenRef.current) return;
    wasOpenRef.current = false;
    rootRef.current?.querySelector<HTMLElement>('[role="combobox"]')?.focus();
  }, [open]);

  return (
    <div
      ref={rootRef}
      data-picker-root=""
      className={placement === "overlay" ? "relative" : undefined}
    >
      {trigger}
      {open ? (
        <div
          data-picker-panel=""
          className={cn(
            placement === "overlay"
              ? "absolute left-0 right-0 top-full z-50 mt-2 overflow-hidden rounded-md border border-border bg-popover shadow-md"
              : "mt-2 overflow-hidden rounded-md border border-border bg-popover shadow-md",
            panelClassName,
          )}
        >
          {children}
        </div>
      ) : null}
    </div>
  );
};

interface CommandPickerContentProps {
  children: ReactNode;
  description?: string;
  shouldFilter?: boolean;
  searchValue?: string;
  onSearchValueChange?: (value: string) => void;
  searchPlaceholder: string;
  emptyText: ReactNode;
  isLoading?: boolean;
  loadingText?: ReactNode;
  commandClassName?: string;
  inputClassName?: string;
  listClassName?: string;
  inputAutoFocus?: boolean;
}

export const CommandPickerLoading = ({ children }: { children: ReactNode }) => (
  <div className="flex items-center justify-center gap-2 px-3 py-4 text-sm text-muted-foreground">
    <Loader2 className="h-3.5 w-3.5 animate-spin" />
    {children}
  </div>
);

export const CommandPickerEmptyRow = ({ children }: { children: ReactNode }) => (
  <div className="px-3 py-4 text-center text-xs text-muted-foreground">
    {children}
  </div>
);

export const CommandPickerContent = ({
  children,
  description,
  shouldFilter = true,
  searchValue,
  onSearchValueChange,
  searchPlaceholder,
  emptyText,
  isLoading = false,
  loadingText,
  commandClassName,
  inputClassName,
  listClassName,
  inputAutoFocus,
}: CommandPickerContentProps) => {
  const descriptionId = useId();
  return (
    <Command shouldFilter={shouldFilter} className={commandClassName}>
      {description && (
        <p id={descriptionId} className="border-b border-border px-3 py-2 text-xs text-muted-foreground">
          {description}
        </p>
      )}
      <CommandInput
        aria-describedby={description ? descriptionId : undefined}
        autoFocus={inputAutoFocus}
        value={searchValue}
        onValueChange={onSearchValueChange}
        placeholder={searchPlaceholder}
        className={cn("h-9 text-sm", inputClassName)}
      />
      <CommandList className={cn("max-h-64", listClassName)}>
        {isLoading ? (
          <CommandPickerLoading>{loadingText}</CommandPickerLoading>
        ) : (
          <>
            <CommandEmpty>{emptyText}</CommandEmpty>
            {children}
          </>
        )}
      </CommandList>
    </Command>
  );
};
