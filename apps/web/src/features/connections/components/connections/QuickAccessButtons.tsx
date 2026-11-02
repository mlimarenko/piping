import { useMemo, useState, type SyntheticEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ClipboardList, ChevronDown, Terminal } from "lucide-react";
import { useTranslation } from "react-i18next";

import { usePermissions } from "@/shared/auth/permissionsContext";
import { useConnectionLaunchOptions } from "@/features/connections/hooks";
import {
  getTargetPlaybookAssignments,
  TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT,
  type PlaybookTargetAssignment,
} from "@/features/playbooks/api";
import type {
  ConnectionLaunchAccount,
  ConnectionLaunchOptions,
  ConnectionTarget,
} from "@/shared/api/types";
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
import type { TreeQuickAccessConfig, TreeQuickAccessProtocol } from "./treeConfig";
import {
  LAUNCH_PROTOCOL_COLOR,
  LAUNCH_PROTOCOL_ICON,
  LAUNCH_PROTOCOL_LABEL,
  connectionAccountText,
  launchConnectionAccount,
  notifyAppLaunchResult,
  presentLaunchAccounts,
  type LaunchProtocol,
  type OpenableProtocol,
} from "./launchPresentation";

type QuickAccessVariant = "tree" | "table" | "card";

interface QuickAccessButtonsProps {
  host: ConnectionTarget;
  config: TreeQuickAccessConfig;
  intent?: boolean;
  intentStartedAt?: number | null;
  onIntent?: () => void;
  variant?: QuickAccessVariant;
  className?: string;
  onConnect: (
    host: ConnectionTarget,
    protocol: OpenableProtocol,
    entryId?: string,
    username?: string,
  ) => void;
  onPlaybookQuickAction?: (host: ConnectionTarget, item: PlaybookTargetAssignment) => void;
}

interface ConnectionQuickAction {
  kind: "connection";
  key: string;
  protocol: LaunchProtocol;
  account: ConnectionLaunchAccount;
}

interface PlaybookQuickAction {
  kind: "playbook";
  key: string;
  item: PlaybookTargetAssignment;
}

type QuickAction = ConnectionQuickAction | PlaybookQuickAction;

const QUERY_KEY_PREFIX = ["connections", "launch-options"] as const;

const variantClass: Record<QuickAccessVariant, string> = {
  tree: "ml-auto",
  table: "ml-auto",
  card: "absolute bottom-2 right-2 z-10 rounded-md border border-border bg-card p-0.5",
};

function stopRowEvent(event: SyntheticEvent) {
  event.stopPropagation();
}

function isConnectionQuickAccessProtocol(
  protocol: TreeQuickAccessProtocol,
): protocol is Exclude<TreeQuickAccessProtocol, "playbook"> {
  return protocol !== "playbook";
}

function isEligiblePlaybookQuickAction(item: PlaybookTargetAssignment): boolean {
  return (
    item.assignment.quick_action_enabled &&
    item.playbook.playbook.status === "active" &&
    item.playbook.playbook.risk === "low" &&
    item.playbook.current_version !== null &&
    item.playbook.current_version !== undefined
  );
}

/** Fresh data is data resolved at or after the recorded hover-intent timestamp;
 *  a stale prefetch must not satisfy the intent. */
function freshAfter<T>(data: T | undefined, updatedAt: number, since: number | null): T | undefined {
  return since !== null && updatedAt >= since ? data : undefined;
}

const QuickAccessButtons = ({
  host,
  config,
  intent = false,
  intentStartedAt = null,
  onIntent,
  variant = "tree",
  className,
  onConnect,
  onPlaybookQuickAction,
}: QuickAccessButtonsProps) => {
  const { t } = useTranslation();
  const { can } = usePermissions();
  // Connection quick-actions require CONNECT; playbook quick-actions do not, so
  // gate only the connection branch below (a READ+PLAYBOOK-without-CONNECT role
  // keeps its playbook quick-actions). CONNECT is held by every seed role, so
  // this only affects custom READ-without-CONNECT roles.
  const canConnect = can("CONNECT", "CONNECTION_TARGET");
  // Playbook quick-actions run a playbook against the target, so suppress them
  // for a role that can read but not EXECUTE PLAYBOOK (view-but-not-edit).
  const canExecutePlaybook = can("EXECUTE", "PLAYBOOK");
  const queryClient = useQueryClient();
  const [localIntentStartedAt, setLocalIntentStartedAt] = useState<number | null>(null);
  const effectiveIntentStartedAt = intentStartedAt ?? localIntentStartedAt;
  const effectiveIntent = intent || effectiveIntentStartedAt !== null;
  const cachedOptions = queryClient.getQueryData<ConnectionLaunchOptions>([
    ...QUERY_KEY_PREFIX,
    host.id,
  ]);
  const needsFreshAppOptions = config.protocols.includes("app");
  const needsConnectionActions = config.protocols.some(isConnectionQuickAccessProtocol);
  const needsPlaybookActions = config.protocols.includes("playbook");
  const launchQuery = useConnectionLaunchOptions(
    host.id,
    needsFreshAppOptions
      ? { enabled: config.enabled && needsConnectionActions && effectiveIntent, staleTime: 0 }
      : { enabled: config.enabled && needsConnectionActions && effectiveIntent },
  );
  const freshOptions = launchQuery.data;
  const freshAppOptions = freshAfter(launchQuery.data, launchQuery.dataUpdatedAt, effectiveIntentStartedAt);
  const fallbackOptions = freshOptions ?? cachedOptions;
  const playbookQuery = useQuery({
    queryKey: ["playbooks", "target-assignments", host.id],
    queryFn: () => getTargetPlaybookAssignments(host.id, TARGET_PLAYBOOK_ASSIGNMENTS_LIMIT),
    enabled: config.enabled && needsPlaybookActions && effectiveIntent,
    retry: false,
    staleTime: 0,
  });
  const freshPlaybookAssignments = freshAfter(playbookQuery.data, playbookQuery.dataUpdatedAt, effectiveIntentStartedAt);

  const actions = useMemo<QuickAction[]>(() => {
    if (!config.enabled) return [];
    return config.protocols.flatMap((protocol): QuickAction[] => {
      if (protocol === "playbook") {
        // Playbook quick-actions execute a playbook; suppress them for a
        // READ-without-EXECUTE role. Connection actions are handled below.
        if (!canExecutePlaybook) return [];
        return (freshPlaybookAssignments?.items ?? [])
          .filter(isEligiblePlaybookQuickAction)
          .map((item) => ({
            kind: "playbook",
            key: `playbook:${item.assignment.id}:${item.playbook.current_version?.id ?? ""}`,
            item,
          }));
      }
      if (!isConnectionQuickAccessProtocol(protocol)) return [];
      // Connection quick-actions (ssh/rdp/vnc/app) need CONNECT; suppress them
      // for a READ-without-CONNECT role. Playbook actions are handled above and
      // stay visible.
      if (!canConnect) return [];
      const options = protocol === "app" ? freshAppOptions : fallbackOptions;
      if (!options) return [];
      const proto = options.protocols.find((candidate) => candidate.protocol === protocol);
      if (!proto) return [];
      return presentLaunchAccounts(proto)
        .filter((account) => !account.requiresInput)
        .map((account) => ({
          kind: "connection",
          key: `${protocol}:${account.entryId}:${account.label}:${account.uri ?? ""}`,
          protocol,
          account,
        }));
    });
  }, [
    canConnect,
    canExecutePlaybook,
    config.enabled,
    config.protocols,
    fallbackOptions,
    freshAppOptions,
    freshPlaybookAssignments?.items,
  ]);

  if (!config.enabled) return null;


  const launchConnection = (action: ConnectionQuickAction) => {
    const result = launchConnectionAccount(host, action.protocol, action.account, onConnect);
    if (action.protocol === "app") {
      void result.then((launchResult) => notifyAppLaunchResult(launchResult, t));
    }
  };

  const launchPlaybook = (action: PlaybookQuickAction) => {
    onPlaybookQuickAction?.(host, action.item);
  };

  const markIntent = () => {
    if (effectiveIntentStartedAt === null) {
      setLocalIntentStartedAt(Date.now());
    }
    onIntent?.();
  };

  if (!canConnect && !canExecutePlaybook) return null;
  const hasConnections = actions.some(action => action.kind === "connection");
  const triggerLabel = t(hasConnections || (canConnect && needsConnectionActions) ? "connections.connectAs.launchMenu" : "connections.connectAs.quickActions");
  const loading = launchQuery.isFetching || playbookQuery.isFetching;
  return (
    <div className={cn("flex shrink-0 items-center", variantClass[variant], className)}>
      <DropdownMenu onOpenChange={open => { if (open) markIntent(); }}>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size={variant === "tree" ? "icon-sm" : "sm"} className={cn("min-h-11 gap-1.5 md:min-h-8", variant !== "tree" && "h-9 whitespace-nowrap px-2 text-sm")} aria-label={triggerLabel} title={triggerLabel}
            onPointerEnter={markIntent} onFocus={markIntent} onClick={stopRowEvent}
            onKeyDown={event => { if (event.key !== "Escape") stopRowEvent(event); }}
            data-testid={`quick-access-${host.id}`}>
            {hasConnections || needsConnectionActions ? <Terminal className="size-3.5" aria-hidden="true" /> : <ClipboardList className="size-3.5" aria-hidden="true" />}
             {variant !== "tree" && <>{triggerLabel}<ChevronDown className="size-3.5" aria-hidden="true" /></>}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-80 max-w-[calc(100vw-1rem)]"
          onClick={stopRowEvent}
          onKeyDown={event => { if (event.key !== "Escape") stopRowEvent(event); }}>
          <DropdownMenuLabel className="break-words text-xs">{host.name}</DropdownMenuLabel>
          <DropdownMenuSeparator />
          {actions.map(action => {
            if (action.kind === "playbook") {
              const title = playbookActionTitle(action.item);
              return <DropdownMenuItem key={action.key} className="min-h-11 items-start gap-2" onSelect={() => launchPlaybook(action)}>
                <ClipboardList className="mt-0.5 size-4 shrink-0 text-primary" aria-hidden="true" />
                <span className="min-w-0 break-words">{t("playbooks.quickActions.menuItem", { name: title })}</span>
              </DropdownMenuItem>;
            }
            const Icon = LAUNCH_PROTOCOL_ICON[action.protocol] ?? Terminal;
            const protocolLabel = LAUNCH_PROTOCOL_LABEL[action.protocol] ?? action.protocol.toUpperCase();
            return <DropdownMenuItem key={action.key} className="min-h-11 items-start gap-2" onSelect={() => launchConnection(action)}>
              <Icon className={cn("mt-0.5 size-4 shrink-0", LAUNCH_PROTOCOL_COLOR[action.protocol])} aria-hidden="true" />
              <span className="min-w-0 flex-1 break-words"><span className="font-medium">{protocolLabel}</span> · {connectionAccountText(action.account, action.protocol)}
                {action.account.kind === "personal" && <span className="block text-xs text-muted-foreground">{t("connections.connectAs.personal")}</span>}
              </span>
            </DropdownMenuItem>;
          })}
          {actions.length === 0 && <DropdownMenuItem disabled className="min-h-11 whitespace-normal">
            {t(loading ? "common.loading" : "connections.connectAs.noQuickActions")}
          </DropdownMenuItem>}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
};

function playbookActionTitle(item: PlaybookTargetAssignment): string {
  return item.assignment.name || item.playbook.playbook.name;
}

export default QuickAccessButtons;
