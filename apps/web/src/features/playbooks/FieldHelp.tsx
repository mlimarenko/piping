import { HelpCircle } from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/shared/ui-primitives/tooltip";

interface FieldHelpProps {
  /** One-line explanation shown on hover/focus. */
  text: string;
  /** Accessible label for the trigger button. */
  label?: string;
}

/**
 * A small inline help affordance: a muted `?` icon that reveals a one-line
 * explanation on hover/focus. Must be rendered inside a single shared
 * `TooltipProvider` (the assignment dialog wraps its whole form in one).
 *
 * Mirrors the tooltip pattern used by `sync-mapping/OperatorPicker`.
 */
export function FieldHelp({ text, label }: FieldHelpProps) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={label ?? text}
          className="inline-flex shrink-0 items-center text-muted-foreground/70 transition-colors hover:text-foreground focus-visible:text-foreground focus-visible:outline-none"
        >
          <HelpCircle className="h-3 w-3" />
        </button>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-xs text-xs">
        {text}
      </TooltipContent>
    </Tooltip>
  );
}
