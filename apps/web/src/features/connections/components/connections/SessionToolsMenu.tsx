import { useTranslation } from "react-i18next";
import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import {
  ClipboardPaste,
  ExternalLink,
  FolderOpen,
  MoreHorizontal,
  RotateCcw,
  Server,
  SquareTerminal,
  Info,
  X,
  XCircle,
  PanelBottom,
  PanelRight,
  SquareStack,
} from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui-primitives/dropdown-menu";
import { cn } from "@/shared/lib/utils";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";

interface SessionToolsMenuProps {
  session: LiveSession;
  onDetachSession: (id: string) => void;
  onReconnectSession: (session: LiveSession) => void;
  onCloseSession: (id: string) => void;
  onCloseAllSessions: () => void;
  isFileBrowserOpen?: boolean;
  onToggleFileBrowser?: () => void;
  /**
   * RDP only: true once guacd has exposed the session's redirected drive.
   * SSH file browsing is available as soon as the session is connected.
   */
  isRdpDriveReady?: boolean;
  /**
   * VNC only: true when an existing connected SSH session for the same host
   * can own the session-scoped SSH file browser.
   */
  isVncSshFileBrowserReady?: boolean;
  isClipboardReady?: boolean;
  onSendClipboard?: () => void;
  showDetachAction?: boolean;
  showCloseAllAction?: boolean;
  onChangeDock?: () => void;
  /** Side the dock action moves the panel to; the icon must match it, not the current side. */
  dockTarget?: "bottom" | "right";
  dockLabel?: string;
  onDetachAllSessions?: () => void;
  onInspectSession?: () => void;
  paneActions?: ReactNode;
}

function protocolLabel(protocol: LiveSession["protocol"]): string {
  return protocol.toUpperCase();
}

function isReconnectBusy(status: LiveSession["status"]): boolean {
  return status === "connecting" || status === "reconnecting";
}

function isSessionEnded(status: LiveSession["status"]): boolean {
  return status === "disconnected" || status === "failed" || status === "idle";
}

interface MenuActionProps {
  icon: LucideIcon;
  label: string;
  description?: string;
  destructive?: boolean;
  onSelect?: () => void;
}

const MenuAction = ({
  icon: Icon,
  label,
  description,
  destructive,
  onSelect,
}: MenuActionProps) => (
  <DropdownMenuItem
    onSelect={onSelect}
    className={cn(
      "min-h-9 items-start gap-2 py-1.5 max-md:min-h-11 pointer-coarse:min-h-11",
      destructive && "text-destructive focus:text-destructive",
    )}
  >
    <Icon className="mt-0.5 h-4 w-4 shrink-0" />
    <span className="min-w-0 flex-1">
      <span className="block whitespace-normal text-sm font-medium leading-4">{label}</span>
      {description ? (
        <span className="menu-item-muted block whitespace-normal text-xs leading-4 text-muted-foreground">
          {description}
        </span>
      ) : null}
    </span>
  </DropdownMenuItem>
);



const SessionToolsMenu = ({
  session,
  onDetachSession,
  onReconnectSession,
  onCloseSession,
  onCloseAllSessions,
  isFileBrowserOpen = false,
  onToggleFileBrowser,
  isRdpDriveReady = false,
  isVncSshFileBrowserReady = false,
  isClipboardReady = false,
  onSendClipboard,
  showDetachAction = true,
  showCloseAllAction = true,
  onChangeDock,
  dockTarget = "bottom",
  dockLabel,
  onDetachAllSessions,
  onInspectSession,
  paneActions,
}: SessionToolsMenuProps) => {
  const { t } = useTranslation();
  const protocol = protocolLabel(session.protocol);
  const connected = session.status === "connected";
  // Unavailable tools stay out of the menu instead of rendering disabled
  // stubs: the ended state owns reconnect, the viewer owns its own recovery.
  const showReconnect = !isSessionEnded(session.status) && !isReconnectBusy(session.status);
  const fileBrowserAvailable =
    !!onToggleFileBrowser &&
    connected &&
    (session.protocol === "ssh" ||
      (session.protocol === "rdp" && isRdpDriveReady) ||
      (session.protocol === "vnc" && isVncSshFileBrowserReady));
  const clipboardAvailable =
    (session.protocol === "rdp" || session.protocol === "vnc") &&
    connected &&
    isClipboardReady &&
    !!onSendClipboard;
  const hasPanelSection = !!(onChangeDock || onDetachAllSessions || paneActions);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className="shrink-0 max-md:size-11 pointer-coarse:size-11"
          aria-label={t("connections.toolsMenu.open")}
          title={t("connections.toolsMenu.open")}
        >
          <MoreHorizontal className="size-4" aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        sideOffset={6}
        className="max-h-[min(72vh,28rem)] w-72 max-w-[calc(100vw-1rem)] overflow-y-auto"
      >
        <DropdownMenuLabel className="px-2 py-1.5">
          <span className="flex min-w-0 items-center gap-2">
            {session.protocol === "ssh" ? (
              <SquareTerminal className="h-4 w-4 shrink-0 text-muted-foreground" />
            ) : (
              <Server className="h-4 w-4 shrink-0 text-muted-foreground" />
            )}
            <span className="min-w-0">
              <span className="block truncate text-sm font-semibold">{session.hostName}</span>
              <span className="block truncate text-xs font-normal text-muted-foreground">
                {protocol}
              </span>
            </span>
          </span>
        </DropdownMenuLabel>
        {onInspectSession && (
          <MenuAction icon={Info} label={t("connections.sessionDetails")} onSelect={onInspectSession} />
        )}
        <DropdownMenuSeparator />
        <DropdownMenuLabel className="px-2 py-1 text-xs text-muted-foreground">
          {t("connections.toolsMenu.sessionActions")}
        </DropdownMenuLabel>
        {showDetachAction ? (
          <MenuAction
            icon={ExternalLink}
            label={t("connections.openNewTab")}
            onSelect={() => onDetachSession(session.id)}
          />
        ) : null}
        {showReconnect ? (
          <MenuAction
            icon={RotateCcw}
            label={t("connections.reconnectSession")}
            onSelect={() => onReconnectSession(session)}
          />
        ) : null}
        <MenuAction
          icon={X}
          label={t("connections.closeSession")}
          destructive
          onSelect={() => onCloseSession(session.id)}
        />
        {showCloseAllAction ? (
          <MenuAction
            icon={XCircle}
            label={t("connections.closeAllSessions")}
            destructive
            onSelect={onCloseAllSessions}
          />
        ) : null}
        {fileBrowserAvailable && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuLabel className="px-2 py-1 text-xs text-muted-foreground">
              {t("connections.toolsMenu.files")}
            </DropdownMenuLabel>
            <MenuAction
              icon={FolderOpen}
              label={
                isFileBrowserOpen
                  ? t("connections.toolsMenu.hideFileBrowser")
                  : session.protocol === "vnc"
                    ? t("connections.toolsMenu.fileBrowserSsh")
                    : t("connections.toolsMenu.fileBrowser")
              }
              description={t(
                session.protocol === "rdp"
                  ? "connections.toolsMenu.rdpFileBrowserMenuDescription"
                  : session.protocol === "vnc"
                    ? "connections.toolsMenu.vncSshFileBrowserMenuDescription"
                    : "connections.toolsMenu.fileBrowserMenuDescription",
              )}
              onSelect={onToggleFileBrowser}
            />
          </>
        )}
        {clipboardAvailable && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuLabel className="px-2 py-1 text-xs text-muted-foreground">
              {t("connections.toolsMenu.clipboard")}
            </DropdownMenuLabel>
            <MenuAction
              icon={ClipboardPaste}
              label={t("connections.toolsMenu.sendClipboard")}
              description={t("connections.toolsMenu.sendClipboardDescription")}
              onSelect={onSendClipboard}
            />
          </>
        )}
        {hasPanelSection && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuLabel className="px-2 py-1 text-xs text-muted-foreground">
              {t("connections.toolsMenu.panel")}
            </DropdownMenuLabel>
            {onChangeDock && (
              <MenuAction icon={dockTarget === "bottom" ? PanelBottom : PanelRight} label={dockLabel ?? t(dockTarget === "bottom" ? "connections.dockBottom" : "connections.dockRight")} onSelect={onChangeDock} />
            )}
            {onDetachAllSessions && (
              <MenuAction icon={SquareStack} label={t("connections.detachAllSessions")} onSelect={onDetachAllSessions} />
            )}
            {paneActions}
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
};

export default SessionToolsMenu;
