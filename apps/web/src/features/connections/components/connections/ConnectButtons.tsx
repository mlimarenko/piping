import { useTranslation } from "react-i18next";
import { Terminal, ChevronDown, KeyRound, Circle, Users } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/shared/ui-primitives/dropdown-menu";
import { cn } from "@/shared/lib/utils";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useConnectionLaunchOptions } from "@/features/connections/hooks";
import type { ConnectionTarget } from "@/shared/api/types";
import type { ConnectionLaunchAccount, ConnectionLaunchProtocol } from "@/shared/api/types";
import {
  LAUNCH_PROTOCOL_COLOR,
  LAUNCH_PROTOCOL_ICON,
  LAUNCH_PROTOCOL_LABEL,
  connectionAccountText,
  connectionAccountTitle,
  launchConnectionAccount,
  notifyAppLaunchResult,
  presentLaunchAccounts,
  sessionAccountInput,
  type OpenableProtocol,
} from "./launchPresentation";
import type { AdHocCredentialsContext } from "./AdHocCredentialsDialog";
import SessionProtocolIcon from "./SessionProtocolIcon";

// ── Component props ───────────────────────────────────────────────────────────

interface ConnectButtonsProps {
  host: ConnectionTarget;
  onConnect: (host: ConnectionTarget, protocol: OpenableProtocol, entryId?: string, username?: string) => void;
  /**
   * Opens the ad-hoc credentials dialog. Supplied by the workspace (the dialog
   * is mounted there, not here). When omitted, accounts that require input stay
   * inert and the "connect with credentials" affordance is hidden.
   */
  onOpenAdHoc?: (context: AdHocCredentialsContext) => void;
}

// ── Main component ────────────────────────────────────────────────────────────

const ConnectButtons = ({ host, onConnect, onOpenAdHoc }: ConnectButtonsProps) => {
  const { can } = usePermissions();
  const launchQuery = useConnectionLaunchOptions(host.id);

  // A recognized non-video family (Linux today) has no `cctv` launch protocol,
  // but its local accounts are still managed through the neutral device console.
  // A video device keeps its `CCTV` entry from the launch options, so the
  // neutral entry is derived only for the non-cctv case.
  const showDeviceAccounts = Boolean(host.family) && !host.isCctv;

  // Hide all launch affordances (incl. the loading/error skeletons) from a role
  // that can read connections but not open a session. CONNECT is held by every
  // seed role, so this only affects custom READ-without-CONNECT roles. The
  // server re-checks on connect regardless; this is the UX layer.
  if (!can("CONNECT", "CONNECTION_TARGET")) return null;

  if (launchQuery.isPending) {
    // Show skeleton protocol buttons while loading
    return (
      <div className="flex flex-wrap gap-1">
        {(["ssh", "rdp", "vnc"] as const).map((proto) => {
          const Icon = LAUNCH_PROTOCOL_ICON[proto] ?? Terminal;
          const color = LAUNCH_PROTOCOL_COLOR[proto] ?? "text-muted-foreground";
          return (
            <Button
              key={proto}
              variant="outline"
              size="sm"
              className={cn("h-9 text-sm gap-1.5 opacity-50", color)}
              disabled
            >
              <Icon className="h-3 w-3" />
              {proto.toUpperCase()}
            </Button>
          );
        })}
      </div>
    );
  }

  if (launchQuery.isError || !launchQuery.data) {
    // Fallback: show plain protocol buttons when options can't be fetched
    return (
      <div className="flex flex-wrap gap-1">
        {(["ssh", "rdp", "vnc"] as const).map((proto) => {
          const Icon = LAUNCH_PROTOCOL_ICON[proto] ?? Terminal;
          const color = LAUNCH_PROTOCOL_COLOR[proto] ?? "text-muted-foreground";
          return (
            <Button
              key={proto}
              variant="outline"
              size="sm"
              className={cn("h-9 text-sm gap-1.5", color)}
              onClick={() => onConnect(host, proto)}
            >
              <Icon className="h-3 w-3" />
              {proto.toUpperCase()}
            </Button>
          );
        })}
        {showDeviceAccounts && <DeviceAccountsButton host={host} onConnect={onConnect} />}
      </div>
    );
  }

  return (
    <div className="flex flex-wrap gap-1">
      {launchQuery.data.protocols.map((proto) => (
        <ProtocolButtons
          key={proto.protocol}
          proto={proto}
          host={host}
          onConnect={onConnect}
          onOpenAdHoc={onOpenAdHoc}
        />
      ))}
      {showDeviceAccounts && <DeviceAccountsButton host={host} onConnect={onConnect} />}
    </div>
  );
};

// ── Neutral device-accounts entry ────────────────────────────────────────────

interface DeviceAccountsButtonProps {
  host: ConnectionTarget;
  onConnect: (host: ConnectionTarget, protocol: OpenableProtocol, entryId?: string, username?: string) => void;
}

/**
 * A non-video (Linux) target's entry into the device console, focused on its
 * accounts. It opens the same `cctv` console tab as a camera does — the session
 * protocol is the existing client-only console switch — so the two entries
 * share one tab component and one lifecycle.
 */
const DeviceAccountsButton = ({ host, onConnect }: DeviceAccountsButtonProps) => {
  const { t } = useTranslation();
  const label = t("connections.deviceAccounts", "Accounts");
  return (
    <Button
      variant="outline"
      size="sm"
      className="h-9 text-sm gap-1.5 text-primary"
      onClick={() => onConnect(host, "cctv")}
      title={label}
    >
      <Users className="h-3 w-3" />
      {label}
    </Button>
  );
};

// ── Per-protocol button renderer ─────────────────────────────────────────────

interface ProtocolButtonsProps {
  proto: ConnectionLaunchProtocol;
  host: ConnectionTarget;
  onConnect: (host: ConnectionTarget, protocol: OpenableProtocol, entryId?: string, username?: string) => void;
  onOpenAdHoc?: (context: AdHocCredentialsContext) => void;
}

const ProtocolButtons = ({ proto, host, onConnect, onOpenAdHoc }: ProtocolButtonsProps) => {
  const { t } = useTranslation();
  const protocol = proto.protocol;

  // De-duplicate accounts for display: the same login (username) on the same
  // host is one button, even if the resolver surfaces it from several
  // scope-matched records (root/rscreen can each come back more than once).
  // Accounts without a username (e.g. VNC) are kept distinct by entryId.
  // App accounts are deduped by their launcher target, and any app account
  // without a target is dropped — nothing to open.
  const accounts = presentLaunchAccounts(proto);
  const baseColor = LAUNCH_PROTOCOL_COLOR[protocol] ?? "text-muted-foreground";

  // Open the ad-hoc dialog to fill in the missing secret for an account. Only
  // session protocols can do this; `app` accounts never reach here.
  // Launch an account: app accounts open a launcher target (no proxied
  // session); session protocols (ssh/rdp/vnc) go through the websocket path.
  // An account that requires input opens the fill-in dialog instead.
  const launch = (account: ConnectionLaunchAccount) => {
    const input = sessionAccountInput(host, protocol, account);
    if (input) {
      onOpenAdHoc?.(input);
      return;
    }
    const result = launchConnectionAccount(host, protocol, account, onConnect);
    if (protocol === "app") {
      void result.then((launchResult) => notifyAppLaunchResult(launchResult, t));
    }
  };

  // No presentable account for this protocol on this target -> render nothing.
  // (e.g. a cashier has SSH + VNC but no RDP, so no phantom RDP button.) The
  // launch-options endpoint emits an entry for every protocol regardless of
  // config, but an empty `accounts` list means no connection-config applies —
  // and the backend connect path requires that config, so there is nothing the
  // UI can offer here without it.
  if (accounts.length === 0) return null;

  // Layout: shared/inherited accounts are direct connect buttons (always
  // visible); personal accounts collapse into a caret dropdown next to them.
  // Both groups stay reachable. App protocol has no personal accounts, so it
  // always renders flat.
  const sharedAccounts = accounts.filter((a) => a.kind === "shared");
  const personalAccounts = accounts.filter((a) => a.kind === "personal");

  return (
    <div className="flex">
      {sharedAccounts.map((account, index) => (
        <AccountButton
          key={`${account.entryId}:${account.label}`}
          account={account}
          protocol={protocol}
          baseColor={baseColor}
          onLaunch={launch}
          // Square the inner edge when a personal-accounts caret follows so the
          // shared button and caret read as one segmented control.
          flushRight={personalAccounts.length > 0 && index === sharedAccounts.length - 1}
        />
      ))}
      {personalAccounts.length > 0 && (
        <PersonalAccountsMenu
          accounts={personalAccounts}
          protocol={protocol}
          baseColor={baseColor}
          attached={sharedAccounts.length > 0}
          onLaunch={launch}
        />
      )}
    </div>
  );
};

// ── Sub-components ────────────────────────────────────────────────────────────

interface AccountButtonProps {
  account: ConnectionLaunchAccount;
  protocol: ConnectionLaunchProtocol["protocol"];
  baseColor: string;
  onLaunch: (account: ConnectionLaunchAccount) => void;
  /** Square the right edge to butt against a following caret dropdown. */
  flushRight?: boolean;
}

const AccountButton = ({ account, protocol, baseColor, onLaunch, flushRight = false }: AccountButtonProps) => {
  const { t } = useTranslation();
  const accentStyle = account.color ? { borderLeftColor: account.color, borderLeftWidth: "2px" } : undefined;
  // An account that requires input opens the fill-in dialog (still actionable),
  // so the title invites credentials instead of marking the button dead.
  const title = account.requiresInput
    ? `${connectionAccountTitle(account)} - ${t("connections.connectAs.requiresInput")}`
    : connectionAccountTitle(account);
  return (
    <Button
      variant="outline"
      size="sm"
      className={cn(
        "h-9 text-sm gap-2 transition-colors",
        flushRight && "rounded-r-none border-r-0",
        baseColor,
      )}
      style={accentStyle}
      onClick={() => onLaunch(account)}
      aria-label={`${LAUNCH_PROTOCOL_LABEL[protocol]} ${connectionAccountText(account, protocol)}`}
      title={protocol === "app" ? `${title} · ${t("connections.panes.externalLaunch")}` : title}
    >
      <SessionProtocolIcon protocol={protocol} />
      {connectionAccountText(account, protocol)}
    </Button>
  );
};

interface PersonalAccountsMenuProps {
  accounts: ConnectionLaunchAccount[];
  protocol: string;
  baseColor: string;
  /** True when shared buttons precede it (square the left edge). */
  attached: boolean;
  onLaunch: (account: ConnectionLaunchAccount) => void;
}

const PersonalAccountsMenu = ({ accounts, protocol, baseColor, attached, onLaunch }: PersonalAccountsMenuProps) => {
  const { t } = useTranslation();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          className={cn("h-9 px-1.5 text-xs", attached ? "rounded-l-none" : "gap-1.5", baseColor)}
          aria-label={t("connections.connectAs.personal")}
        >
          {!attached && <Circle className="h-3 w-3" />}
          <ChevronDown className="h-3 w-3" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-[11rem]">
        <DropdownMenuLabel className="text-xs font-medium text-muted-foreground px-2 py-1">
          {t("connections.connectAs.personal")}
        </DropdownMenuLabel>
        <DropdownMenuGroup>
          {accounts.map((account) => (
            <AccountMenuItem
              key={account.entryId}
              account={account}
              protocol={protocol}
              onLaunch={onLaunch}
            />
          ))}
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
};

interface AccountMenuItemProps {
  account: ConnectionLaunchAccount;
  protocol: string;
  onLaunch: (account: ConnectionLaunchAccount) => void;
}

const AccountMenuItem = ({ account, protocol, onLaunch }: AccountMenuItemProps) => {
  const { t } = useTranslation();
  const sourceChip = account.kind === "personal"
    ? account.source.credentialSetName
    : account.source.recordName
      ? `${account.source.recordName}${account.source.scopeType ? ` · ${account.source.scopeType}` : ""}`
      : undefined;

  return (
    <DropdownMenuItem
      className="flex items-center gap-2 text-xs px-2 py-1.5 cursor-pointer"
      onClick={() => onLaunch(account)}
    >
      {/* Color dot or placeholder */}
      <span
        className="h-2 w-2 rounded-full shrink-0"
        style={account.color ? { backgroundColor: account.color } : undefined}
      >
        {!account.color && <Circle className="h-2 w-2 text-muted-foreground/40" />}
      </span>

      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-1.5">
          <span className="font-medium text-foreground truncate">{connectionAccountText(account, protocol)}</span>
          {account.isDefault && (
            <span className="text-xs text-muted-foreground border border-border/50 rounded px-1 shrink-0">
              {t("connections.connectAs.defaultAccount")}
            </span>
          )}
          {account.requiresInput && (
            <KeyRound
              className="h-3 w-3 text-status-warning shrink-0"
              aria-label={t("connections.connectAs.requiresInput")}
            />
          )}
        </div>
        {sourceChip && (
          <span className="text-xs text-muted-foreground truncate block">{sourceChip}</span>
        )}
        {account.username && (
          <span className="text-xs text-muted-foreground/70 font-mono truncate block">{account.username}</span>
        )}
      </div>
    </DropdownMenuItem>
  );
};

export default ConnectButtons;
