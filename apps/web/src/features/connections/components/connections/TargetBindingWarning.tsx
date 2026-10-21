import { useTranslation } from "react-i18next";
import { AlertTriangle } from "lucide-react";
import { Tooltip, TooltipTrigger, TooltipContent } from "@/shared/ui-primitives/tooltip";
import { cn } from "@/shared/lib/utils";
import type { ConnectionTarget } from "@/shared/api/types";

/**
 * Amber warning shown next to a target whose primary address is not bound to an
 * inventory interface (`addressBound === false`). The backend anti-SSRF guard
 * rejects connect for such targets (`endpoint_not_bound_to_target`); this badge
 * sets the expectation up front. It never blocks the connect action — the
 * backend stays the source of truth.
 */
export function TargetBindingWarning({
  host,
  className,
  iconClassName,
}: {
  host: Pick<ConnectionTarget, "addressBound">;
  className?: string;
  iconClassName?: string;
}) {
  const { t } = useTranslation();
  if (host.addressBound !== false) return null;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          className={cn("inline-flex shrink-0 items-center text-status-warning", className)}
          role="img"
          aria-label={t("connections.noBoundAddress")}
        >
          <AlertTriangle className={cn("h-3 w-3", iconClassName)} />
        </span>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-[16rem] text-xs">
        {t("connections.noBoundAddress")}
      </TooltipContent>
    </Tooltip>
  );
}
