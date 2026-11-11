import { useId } from "react";
import { useTranslation } from "react-i18next";
import { MonitorCog, RotateCcw } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui-primitives/popover";
import { RadioGroup, RadioGroupItem } from "@/shared/ui-primitives/radio-group";
import { cn } from "@/shared/lib/utils";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";
import { useSessionViewSettings } from "@/features/connections/hooks/useSessionViewSettings";
import {
  saveSessionViewSettings,
  type SessionDisplayMode,
  type SessionQuality,
} from "@/features/connections/sessionViewPreferences";

interface SessionViewOptionProps {
  value: string;
  label: string;
  description: string;
  onSelect: () => void;
}

function SessionViewOption({ value, label, description, onSelect }: SessionViewOptionProps) {
  const labelId = useId();
  return (
    <div
      className="flex cursor-pointer items-start gap-2 rounded-md px-2 py-2 transition-colors hover:bg-accent"
      onClick={(event) => {
        // The radio button owns its own change event; only row clicks need
        // the explicit selection.
        if ((event.target as HTMLElement).closest("button")) return;
        onSelect();
      }}
    >
      <RadioGroupItem
        value={value}
        aria-labelledby={labelId}
        className="mt-0.5 shrink-0"
      />
      <span id={labelId} className="min-w-0 flex-1">
        <span className="block text-sm font-medium leading-4">{label}</span>
        <span className="menu-item-muted block text-xs leading-4 text-muted-foreground">
          {description}
        </span>
      </span>
    </div>
  );
}

interface SessionViewMenuProps {
  session: LiveSession;
  onReconnectSession: (session: LiveSession) => void;
  className?: string;
}

/**
 * Per-host screen and image-quality controls for RDP/VNC sessions. The values
 * live in localStorage and the viewer subscribes to the same store, so a
 * change here applies without remounting the session.
 */
const SessionViewMenu = ({ session, onReconnectSession, className }: SessionViewMenuProps) => {
  const { t } = useTranslation();
  const view = useSessionViewSettings(session.hostId, session.protocol);
  const setView = (patch: { displayMode?: SessionDisplayMode; quality?: SessionQuality }) =>
    saveSessionViewSettings(session.hostId, session.protocol, patch);

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className={cn("shrink-0 max-md:size-11 pointer-coarse:size-11", className)}
          aria-label={t("connections.viewMenu.open")}
          title={t("connections.viewMenu.open")}
        >
          <MonitorCog className="size-4" aria-hidden="true" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        sideOffset={6}
        className="w-[min(22rem,calc(100vw-1rem))] p-3"
        aria-label={t("connections.viewMenu.title")}
      >
        <div className="space-y-3">
          <div className="min-w-0">
            <div className="text-sm font-semibold">{t("connections.viewMenu.title")}</div>
            <div className="truncate text-xs text-muted-foreground">
              {session.hostName} · {session.protocol.toUpperCase()}
            </div>
          </div>

          <section>
            <h3 className="px-2 pb-1 text-xs font-medium text-muted-foreground">
              {t("connections.viewMenu.display")}
            </h3>
            <RadioGroup
              value={view.displayMode}
              onValueChange={(value) => setView({ displayMode: value as SessionDisplayMode })}
              className="gap-1"
            >
              <SessionViewOption
                value="fit"
                label={t("connections.viewMenu.displayFit")}
                description={t("connections.viewMenu.displayFitDescription")}
                onSelect={() => setView({ displayMode: "fit" })}
              />
              <SessionViewOption
                value="actual"
                label={t("connections.viewMenu.displayActual")}
                description={t("connections.viewMenu.displayActualDescription")}
                onSelect={() => setView({ displayMode: "actual" })}
              />
            </RadioGroup>
          </section>

          <section>
            <h3 className="px-2 pb-1 text-xs font-medium text-muted-foreground">
              {t("connections.viewMenu.quality")}
            </h3>
            <RadioGroup
              value={view.quality}
              onValueChange={(value) => setView({ quality: value as SessionQuality })}
              className="gap-1"
            >
              <SessionViewOption
                value="connection"
                label={t("connections.viewMenu.qualityConnection")}
                description={t("connections.viewMenu.qualityConnectionDescription")}
                onSelect={() => setView({ quality: "connection" })}
              />
              <SessionViewOption
                value="sharp"
                label={t("connections.viewMenu.qualitySharp")}
                description={t("connections.viewMenu.qualitySharpDescription")}
                onSelect={() => setView({ quality: "sharp" })}
              />
              <SessionViewOption
                value="balanced"
                label={t("connections.viewMenu.qualityBalanced")}
                description={t("connections.viewMenu.qualityBalancedDescription")}
                onSelect={() => setView({ quality: "balanced" })}
              />
              <SessionViewOption
                value="data_saver"
                label={t("connections.viewMenu.qualityDataSaver")}
                description={t("connections.viewMenu.qualityDataSaverDescription")}
                onSelect={() => setView({ quality: "data_saver" })}
              />
            </RadioGroup>
          </section>

          {view.quality !== "connection" && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="w-full justify-start gap-2"
              onClick={() => onReconnectSession(session)}
            >
              <RotateCcw className="size-4" aria-hidden="true" />
              <span className="text-left">
                <span className="block">{t("connections.viewMenu.apply")}</span>
                <span className="menu-item-muted block text-xs font-normal text-muted-foreground">
                  {t("connections.viewMenu.applyDescription")}
                </span>
              </span>
            </Button>
          )}
          <p className="px-2 text-xs leading-4 text-muted-foreground">
            {t("connections.viewMenu.savedForHost")}
          </p>
        </div>
      </PopoverContent>
    </Popover>
  );
};

export default SessionViewMenu;
