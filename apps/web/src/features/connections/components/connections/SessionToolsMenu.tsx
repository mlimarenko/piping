import { useTranslation } from "react-i18next";
import type { LucideIcon } from "lucide-react";
import {
  ClipboardPaste,
  ExternalLink,
  FolderOpen,
  RotateCcw,
  Server,
  SquareTerminal,
  Wrench,
  X,
  XCircle,
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
  dockLabel?: string;
  onDetachAllSessions?: () => void;
}

function protocolLabel(protocol: LiveSession["protocol"]): string {
  return protocol.toUpperCase();
}

function isReconnectBusy(status: LiveSession["status"]): boolean {
  return status === "connecting" || status === "reconnecting";
}

interface MenuActionProps {
  icon: LucideIcon;
  label: string;
  description?: string;
  disabled?: boolean;
  destructive?: boolean;
  onSelect?: () => void;
}

const MenuAction = ({
  icon: Icon,
  label,
  description,
  disabled,
  destructive,
  onSelect,
}: MenuActionProps) => (
  <DropdownMenuItem
    disabled={disabled}
    onSelect={disabled || !onSelect ? undefined : onSelect}
    className={cn(
      "min-h-9 items-start gap-2 py-1.5",
      destructive && "text-destructive focus:text-destructive",
    )}
  >
    <Icon className="mt-0.5 h-4 w-4 shrink-0" />
    <span className="min-w-0 flex-1">
      <span className="block truncate text-sm font-medium leading-4">{label}</span>
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
  dockLabel,
  onDetachAllSessions,
}: SessionToolsMenuProps) => {
  const { t } = useTranslation();
  const reconnectBusy = isReconnectBusy(session.status);
  const protocol = protocolLabel(session.protocol);
  const hasFileTools =
    session.protocol === "ssh" ||
    session.protocol === "rdp" ||
    (session.protocol === "vnc" && isVncSshFileBrowserReady);
  const hasClipboardTools = session.protocol === "rdp" || session.protocol === "vnc";
  const fileBrowserDisabledReason =
    session.status !== "connected" || !onToggleFileBrowser
      ? t(
          session.protocol === "rdp"
            ? "connections.toolsMenu.rdpDriveNotReady"
            : "connections.toolsMenu.sshNotReady",
        )
      : session.protocol === "rdp" && !isRdpDriveReady
        ? t("connections.toolsMenu.rdpDriveNotReady")
        : undefined;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t("connections.toolsMenu.open")}
          title={t("connections.toolsMenu.open")}
        >
          <Wrench className="h-4 w-4" />
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
              <span className="block truncate text-sm font-semibold">
                {t("connections.toolsMenu.title")}
              </span>
              <span className="block truncate text-xs font-normal text-muted-foreground">
                {t("connections.toolsMenu.sessionSummary", {
                  protocol,
                  host: session.hostName,
                })}
              </span>
            </span>
          </span>
        </DropdownMenuLabel>
        {hasFileTools && (
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
              description={
                fileBrowserDisabledReason ??
                t(
                  session.protocol === "rdp"
                    ? "connections.toolsMenu.rdpFileBrowserMenuDescription"
                    : session.protocol === "vnc"
                      ? "connections.toolsMenu.vncSshFileBrowserMenuDescription"
                      : "connections.toolsMenu.fileBrowserMenuDescription",
                )
              }
              disabled={!!fileBrowserDisabledReason}
              onSelect={onToggleFileBrowser}
            />
          </>
        )}
        {(onChangeDock || onDetachAllSessions) && <>
          <DropdownMenuSeparator />
          {onChangeDock && <MenuAction icon={PanelRight} label={dockLabel ?? t("connections.dockRight")} onSelect={onChangeDock} />}
          {onDetachAllSessions && <MenuAction icon={SquareStack} label={t("connections.detachAllSessions")} onSelect={onDetachAllSessions} />}
        </>}
        {hasClipboardTools && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuLabel className="px-2 py-1 text-xs text-muted-foreground">
              {t("connections.toolsMenu.clipboard")}
            </DropdownMenuLabel>
            <MenuAction
              icon={ClipboardPaste}
              label={t("connections.toolsMenu.sendClipboard")}
              description={
                session.status !== "connected" || !onSendClipboard
                  ? t("connections.toolsMenu.unavailableWhileConnecting")
                  : !isClipboardReady
                    ? t("connections.toolsMenu.clipboardNotReady")
                    : t("connections.toolsMenu.sendClipboardDescription")
              }
              disabled={session.status !== "connected" || !onSendClipboard || !isClipboardReady}
              onSelect={onSendClipboard}
            />
          </>
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
        <MenuAction
          icon={RotateCcw}
          label={t("connections.reconnectSession")}
          description={reconnectBusy ? t("connections.toolsMenu.unavailableWhileConnecting") : undefined}
          disabled={reconnectBusy}
          onSelect={() => onReconnectSession(session)}
        />
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
      </DropdownMenuContent>
    </DropdownMenu>
  );
};

export default SessionToolsMenu;
