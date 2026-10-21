import type { ElementType, ReactNode } from "react";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/shared/ui-primitives/tooltip";
import { cn } from "@/shared/lib/utils";

interface InspectorFieldLabelProps {
  icon?: ElementType;
  label: ReactNode;
  help?: ReactNode;
  className?: string;
  iconClassName?: string;
}

const InspectorFieldLabel = ({
  icon: Icon,
  label,
  help,
  className,
  iconClassName,
}: InspectorFieldLabelProps) => {
  const content = (
    <span
      className={cn(
        "inline-flex min-w-0 max-w-full items-start gap-1.5",
        className,
      )}
      tabIndex={help ? 0 : undefined}
    >
      {Icon ? <Icon className={cn("h-3 w-3 shrink-0", iconClassName)} aria-hidden="true" /> : null}
      <span className="min-w-0 whitespace-normal [overflow-wrap:anywhere]">{label}</span>
    </span>
  );

  if (!help) return content;

  return (
    <TooltipProvider delayDuration={200}>
      <Tooltip>
        <TooltipTrigger asChild>{content}</TooltipTrigger>
        <TooltipContent side="top" className="max-w-72 text-xs leading-relaxed">
          {help}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
};

export default InspectorFieldLabel;
