import { forwardRef } from "react";
import type { ComponentPropsWithoutRef } from "react";
import { Ellipsis } from "lucide-react";

import { cn } from "@/shared/lib/utils";
import { Button } from "@/shared/ui-primitives/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/shared/ui-primitives/dropdown-menu";

export interface RowActionsProps extends ComponentPropsWithoutRef<typeof DropdownMenu> {
  label: string;
  disabled?: boolean;
  className?: string;
  contentProps?: Omit<ComponentPropsWithoutRef<typeof DropdownMenuContent>, "children">;
}

const RowActions = forwardRef<HTMLButtonElement, RowActionsProps>(
  ({ label, disabled, className, contentProps, children, ...props }, ref) => (
    <DropdownMenu {...props}>
      <DropdownMenuTrigger asChild disabled={disabled}>
        <Button
          ref={ref}
          data-slot="row-actions-trigger"
          type="button"
          variant="ghost"
          size="icon-sm"
          className={cn("size-11 shrink-0 sm:size-8", className)}
          aria-label={label}
          title={label}
          disabled={disabled}
          onClick={(event) => event.stopPropagation()}
        >
          <Ellipsis aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        {...contentProps}
        className={cn("min-w-48 max-w-[calc(100vw-2rem)]", contentProps?.className)}
        onClick={(event) => {
          event.stopPropagation();
          contentProps?.onClick?.(event);
        }}
      >
        {children}
      </DropdownMenuContent>
    </DropdownMenu>
  ),
);
RowActions.displayName = "RowActions";

export default RowActions;
