import { Bot } from "lucide-react";
import { useTranslation } from "react-i18next";

import { useAssistant } from "./assistantContext";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { cn } from "@/shared/lib/utils";
import { Button } from "@/shared/ui-primitives/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/shared/ui-primitives/tooltip";

export function AssistantLauncher({ collapsed }: { collapsed: boolean }) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const assistant = useAssistant();
  const open = assistant.window.open;

  // Hide entirely when bootstrap is still loading, failed, or the assistant is not
  // configured / the user lacks Execute permission. assistantReady is false in all
  // those cases, so a single guard covers them all. Also require READ — the same
  // gate the sidebar nav uses — so a user without any assistant right never sees
  // the launcher even transiently.
  if (!can("READ", "ASSISTANT") || !assistant.assistantReady) return null;

  const button = (
    <Button
      type="button"
      variant={open ? "secondary" : "ghost"}
      size={collapsed ? "icon" : "sm"}
      className={cn(
        "relative w-full justify-start gap-2 text-sm",
        collapsed && "h-9 justify-center px-0",
        open && "bg-accent text-primary hover:bg-accent",
      )}
      aria-pressed={open}
      aria-label={t("assistant.launcher")}
      onClick={() => assistant.setOpen(!open)}
    >
      <Bot className="h-4 w-4 shrink-0" />
      {!collapsed && <span className="truncate">{t("assistant.launcher")}</span>}
      {/* dot is always green here — we only render when assistantReady=true */}
      <span
        className={cn(
          "absolute rounded-full border border-background bg-status-positive",
          collapsed ? "right-1.5 top-1.5 h-2 w-2" : "right-2 top-2 h-2 w-2",
        )}
      />
    </Button>
  );

  if (!collapsed) return button;

  return (
    <TooltipProvider delayDuration={200}>
      <Tooltip>
        <TooltipTrigger asChild>{button}</TooltipTrigger>
        <TooltipContent side="right" className="text-sm">
          {t("assistant.launcher")}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
