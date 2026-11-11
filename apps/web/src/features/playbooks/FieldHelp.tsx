import { HelpCircle } from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/shared/ui-primitives/tooltip";

interface FieldHelpProps {
  /** One-line explanation shown on hover/focus. */
  text: string;
  /** Accessible label for the trigger. */
  label?: string;
  /** Public documentation page for this field; turns the trigger into a link. */
  href?: string;
}

const TRIGGER_CLASS =
  "inline-flex shrink-0 items-center text-muted-foreground/70 transition-colors hover:text-foreground focus-visible:text-foreground focus-visible:outline-none";

/**
 * A small inline help affordance: a muted `?` icon that reveals a one-line
 * explanation on hover/focus. Must be rendered inside a single shared
 * `TooltipProvider` (the assignment dialog wraps its whole form in one).
 *
 * With `href` the same trigger opens the page that documents the field, so an
 * inline hint never becomes a dead end.
 *
 * Mirrors the tooltip pattern used by `sync-mapping/OperatorPicker`.
 */
export function FieldHelp({ text, label, href }: FieldHelpProps) {
  const trigger = href
    ? <a href={href} target="_blank" rel="noopener noreferrer" aria-label={label ?? text} className={TRIGGER_CLASS}>
        <HelpCircle className="h-3 w-3" />
      </a>
    : <button type="button" aria-label={label ?? text} className={TRIGGER_CLASS}>
        <HelpCircle className="h-3 w-3" />
      </button>;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{trigger}</TooltipTrigger>
      <TooltipContent side="top" className="max-w-xs text-xs">
        {text}
      </TooltipContent>
    </Tooltip>
  );
}
