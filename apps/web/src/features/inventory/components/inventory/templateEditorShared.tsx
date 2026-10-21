import { type ReactNode } from "react";
import { HelpCircle, Braces } from "lucide-react";
import { useTranslation } from "react-i18next";

import { cn } from "@/shared/lib/utils";
import { Button } from "@/shared/ui-primitives/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/shared/ui-primitives/tooltip";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui-primitives/dropdown-menu";
import type { TokenOption } from "./templateEditorHelp";

/**
 * Small help affordance: an info icon that reveals explanatory copy on hover or
 * keyboard focus. Used to demystify every non-obvious field in the template
 * authoring surface (value types, conflict policies, placeholder syntax, …).
 */
export function FieldHelp({
  text,
  className,
  side = "top",
}: {
  text: ReactNode;
  className?: string;
  side?: "top" | "right" | "bottom" | "left";
}) {
  const { t } = useTranslation();
  if (!text) return null;
  return (
    <Tooltip delayDuration={150}>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label={t("inventory.templates.editor.help")}
          className={cn(
            "inline-flex items-center justify-center text-muted-foreground hover:text-foreground transition-colors rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring",
            className,
          )}
        >
          <HelpCircle className="h-3 w-3" />
        </Button>
      </TooltipTrigger>
      <TooltipContent side={side} className="max-w-[260px] text-xs leading-relaxed">
        {text}
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Insert-assist menu for `{{token}}` fields. Surfaces the available variable
 * keys and `{{steps.<key>.id}}` references so users never hand-type tokens
 * (and never typo them). Calls `onInsert` with the chosen raw token.
 */
export function TokenMenu({
  variableTokens,
  stepTokens,
  onInsert,
  disabled,
  ariaLabel,
}: {
  variableTokens: TokenOption[];
  stepTokens: TokenOption[];
  onInsert: (token: string) => void;
  disabled?: boolean;
  ariaLabel: string;
}) {
  const { t } = useTranslation();
  const empty = variableTokens.length === 0 && stepTokens.length === 0;

  return (
    <DropdownMenu>
      <Tooltip delayDuration={150}>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="size-9 shrink-0 text-muted-foreground hover:text-primary"
              disabled={disabled || empty}
              aria-label={ariaLabel}
            >
              <Braces className="h-3.5 w-3.5" />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="top" className="text-xs">
          {t("inventory.templates.editor.insertToken")}
        </TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-64 max-h-72 overflow-y-auto">
        {variableTokens.length > 0 && (
          <>
            <DropdownMenuLabel className="text-xs uppercase tracking-wide text-muted-foreground">
              {t("inventory.templates.editor.tokenVariables")}
            </DropdownMenuLabel>
            {variableTokens.map((option) => (
              <DropdownMenuItem
                key={option.token}
                onSelect={() => onInsert(option.token)}
                className="flex flex-col items-start gap-0.5"
              >
                <span className="font-mono text-xs">{option.token}</span>
                <span className="text-xs text-muted-foreground">
                  {option.label}
                  {option.hint ? ` · ${option.hint}` : ""}
                </span>
              </DropdownMenuItem>
            ))}
          </>
        )}
        {variableTokens.length > 0 && stepTokens.length > 0 && <DropdownMenuSeparator />}
        {stepTokens.length > 0 && (
          <>
            <DropdownMenuLabel className="text-xs uppercase tracking-wide text-muted-foreground">
              {t("inventory.templates.editor.tokenStepRefs")}
            </DropdownMenuLabel>
            {stepTokens.map((option) => (
              <DropdownMenuItem
                key={option.token}
                onSelect={() => onInsert(option.token)}
                className="flex flex-col items-start gap-0.5"
              >
                <span className="font-mono text-xs">{option.token}</span>
                <span className="text-xs text-muted-foreground">{option.label}</span>
              </DropdownMenuItem>
            ))}
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
