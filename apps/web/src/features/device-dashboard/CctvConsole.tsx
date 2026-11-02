import { useState, type ElementType, type ReactNode } from "react";
import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  Clock,
  Cpu,
  HardDrive,
  MonitorPlay,
  Users,
} from "lucide-react";
import { connectionFailureFromApiError } from "@/features/connections/connectionFailure";
import ConnectionFailureOverlay from "@/features/connections/components/connections/ConnectionFailureOverlay";
import { ApiError } from "@/shared/api/client";
import { cn } from "@/shared/lib/utils";
import SurfaceCard, { SurfaceCardHeader, SurfaceCardContent } from "@/shared/ui-shared/SurfaceCard";
import StatusChip from "@/shared/ui-shared/StatusChip";
import { Skeleton } from "@/shared/ui-primitives/skeleton";
import { Badge } from "@/shared/ui-primitives/badge";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import {
  getDeviceCapabilities,
  getDeviceChannels,
  getDeviceDisk,
  getDeviceInfo,
  getDeviceRecording,
  getDeviceUsers,
  type DeviceCapabilities,
  type DeviceChannel,
  type DeviceDisk,
  type DeviceInfo,
  type DeviceRecording,
  type DeviceUsers,
} from "@/features/device-dashboard/api";
import DeviceUserSection from "@/features/device-dashboard/DeviceUserSection";
import { formatBytes } from "@/features/device-dashboard/format";
import CameraControl from "@/features/device-dashboard/CameraControl";
import ClipExport, { type ExportSeed } from "@/features/device-dashboard/ClipExport";
import ArchivePlayer from "@/features/device-dashboard/ArchivePlayer";
import { retryOn5xx } from "@/shared/api/hooks";

// A deterministic config error (no credentials) never becomes readable by
// retrying; only genuine 5xx are worth another attempt.
type DiskEntry = DeviceDisk["disks"][number];
type CctvTab = "cameras" | "archive" | "users" | "device";

/**
 * Which sections this caller may see on this device.
 *
 * Each flag is the AND of a per-target permission and a driver capability:
 * the operator must be allowed to look, and the device must be able to answer.
 * Derived in one place so a section cannot appear in the tab strip while its
 * data query stays disabled (or the reverse).
 */
interface VisibleSections {
  showLive: boolean;
  showIdentity: boolean;
  showArchive: boolean;
  showUsers: boolean;
  showDisk: boolean;
  /** The Device tab is a container for identity and disk health. */
  showDevice: boolean;
}

function visibleSections(capabilities: DeviceCapabilities | undefined): VisibleSections {
  const permissions = capabilities?.permissions;
  const showLive =
    permissions?.view === true &&
    capabilities != null &&
    (capabilities.streaming || capabilities.snapshot || capabilities.channels);
  const showIdentity = permissions?.view === true && capabilities?.info === true;
  const showDisk = permissions?.settings === true && capabilities?.disk === true;
  return {
    showLive,
    showIdentity,
    showArchive: permissions?.recordings === true && capabilities?.recording === true,
    showUsers: permissions?.users === true && capabilities?.users === true,
    showDisk,
    showDevice: showIdentity || showDisk,
  };
}

/** The tab strip for the sections this caller can actually reach. */
function availableTabs(sections: VisibleSections, t: TFunction) {
  const candidates = [
    {
      value: "cameras" as const,
      show: sections.showLive,
      label: t("deviceDashboard.tabs.cameras", "Cameras"),
      icon: MonitorPlay,
    },
    {
      value: "archive" as const,
      show: sections.showArchive,
      label: t("deviceDashboard.tabs.archive", "Archive"),
      icon: Clock,
    },
    {
      value: "users" as const,
      show: sections.showUsers,
      label: t("deviceDashboard.tabs.users", "Users"),
      icon: Users,
    },
    {
      value: "device" as const,
      show: sections.showDevice,
      label: t("deviceDashboard.tabs.device", "Device"),
      icon: Cpu,
    },
  ];
  return candidates
    .filter((tab) => tab.show)
    .map(({ value, label, icon }) => ({ value, label, icon }));
}

/**
 * The per-section device reads, plus the two things every caller derives from
 * them: the archive channel list and the whole-device failure verdict.
 *
 * Each read is gated on its own section flag, so a section the caller cannot
 * see costs no device call. Extracted from the console component because these
 * five queries and their error arbitration are one concern — "what did the
 * device tell us" — separate from how the console lays that out.
 */
function useDeviceSectionData(
  targetId: string,
  enabled: boolean,
  sections: VisibleSections,
  capabilities: DeviceCapabilities | undefined,
) {
  const { showIdentity, showArchive, showUsers, showDisk } = sections;
  const infoQuery = useQuery({
    queryKey: ["device", targetId, "info"],
    queryFn: () => getDeviceInfo(targetId),
    enabled: enabled && showIdentity,
    retry: retryOn5xx,
  });
  const usersQuery = useQuery({
    queryKey: ["device", targetId, "users"],
    queryFn: () => getDeviceUsers(targetId),
    enabled: enabled && showUsers,
    retry: retryOn5xx,
  });
  const diskQuery = useQuery({
    queryKey: ["device", targetId, "disk"],
    queryFn: () => getDeviceDisk(targetId),
    enabled: enabled && showDisk,
    retry: retryOn5xx,
  });
  const recordingQuery = useQuery({
    queryKey: ["device", targetId, "recording"],
    queryFn: () => getDeviceRecording(targetId),
    enabled: enabled && showArchive,
    retry: retryOn5xx,
  });
  // Named channels for the archive camera picker. Shares its key with the
  // camera panel's query, so opening Archive reuses an already-cached list
  // rather than paying a second device call.
  const channelsQuery = useQuery({
    queryKey: ["device", targetId, "channels"],
    queryFn: () => getDeviceChannels(targetId),
    enabled: enabled && showArchive && Boolean(capabilities?.channels),
    retry: retryOn5xx,
  });
  // Fall back to the bare channel count when the family cannot enumerate names.
  const archiveChannels: DeviceChannel[] =
    channelsQuery.data?.channels ??
    Array.from({ length: Math.max(infoQuery.data?.channels ?? 1, 1) }, (_unused, index) => ({
      index: index + 1,
      name: null,
      enabled: null,
      ptz_support: "unknown" as const,
      recording: null,
    }));

  // Hoist to the full-page notice only when the device could not be read at
  // all. A 400 on a single section (op unsupported by this family/firmware)
  // is a normal per-device outcome — the remaining cards must still render.
  const gated: UseQueryResult<unknown, unknown>[] = [];
  if (showIdentity) gated.push(infoQuery);
  if (showUsers) gated.push(usersQuery);
  if (showDisk) gated.push(diskQuery);
  if (showArchive) gated.push(recordingQuery);
  const allFailed = gated.length > 0 && gated.every((query) => query.isError);
  const configurationError = allFailed
    ? firstConfigurationError(gated.map((query) => query.error))
    : undefined;

  return {
    infoQuery,
    usersQuery,
    diskQuery,
    recordingQuery,
    archiveChannels,
    configurationError,
  };
}

/**
 * Re-run a query from a retry control.
 *
 * The returned promise is deliberately not awaited: a refetch reports its
 * outcome through the query's own `isError`/`error` state, which is what the
 * surrounding section already renders from, so there is nothing for the caller
 * to do with it. The rejection is swallowed rather than left floating so a
 * transport failure cannot surface as an unhandled rejection.
 */
function retry(query: Pick<UseQueryResult<unknown, unknown>, "refetch">): void {
  query.refetch().catch(() => undefined);
}

function firstConfigurationError(errors: unknown[]): ApiError | undefined {
  return errors.find(
    (error): error is ApiError =>
      error instanceof ApiError &&
      ["configuration", "authentication"].includes(connectionFailureFromApiError(error).kind),
  );
}

export interface CctvConsoleProps {
  /** Connection-target id of the recorder/camera to manage. */
  targetId: string;
  className?: string;
  /**
   * Whether this console's session tab is the active one. Forwarded like the
   * SSH/RDP/VNC viewers so a backgrounded CCTV tab in the SAME window (which
   * stays document-visible) pauses its live streams and stops starving the
   * ~6-per-origin browser connection pool. Defaults true for the standalone route.
   */
  active?: boolean;
}

/**
 * The device-management mini-admin: a capability-driven grid (live view + PTZ,
 * identity, recording, disk health, user accounts) over the device REST plane —
 * `/api/v1/cctv/{targetId}/*` for the video panels and the neutral
 * `/api/v1/device-fleet/{targetId}/users*` routes for accounts. Rendered both as
 * a workspace console tab (the primary flow, opened from the connect panel like
 * SSH/RDP/VNC) and, wrapped with a page header, at the standalone
 * `/connections/devices/:id` route.
 */
export const CctvConsole = ({ targetId, className, active = true }: CctvConsoleProps) => {
  const { t } = useTranslation();
  const enabled = !!targetId;
  const [activeTab, setActiveTab] = useState<CctvTab>("cameras");

  // Static capability set (no device I/O): the control shape the camera panel
  // renders from. Kept separate from the live reads so an offline device still
  // yields the panel skeleton rather than blanking the page.
  const capabilitiesQuery = useQuery({
    queryKey: ["device", targetId, "capabilities"],
    queryFn: () => getDeviceCapabilities(targetId),
    enabled,
    retry: retryOn5xx,
  });
  const capabilities = capabilitiesQuery.data;
  const sections = visibleSections(capabilities);
  const { showLive, showIdentity, showArchive, showUsers, showDisk, showDevice } = sections;
  const tabs = availableTabs(sections, t);
  const selectedTab = tabs.some((tab) => tab.value === activeTab)
    ? activeTab
    : (tabs[0]?.value ?? "cameras");

  // User management is one explicit `device_users:connection_target` permission.
  // Driver capabilities still decide whether editing an existing account is supported.
  const canCreateUsers = enabled && showUsers;
  const canUpdateUsers = enabled && showUsers && capabilities?.user_update === true;
  const canDeleteUsers = enabled && showUsers;
  const { infoQuery, usersQuery, diskQuery, recordingQuery, archiveChannels, configurationError } =
    useDeviceSectionData(targetId, enabled, sections, capabilities);

  // A failed capability read blanks the entire console, and silently. Every
  // section is gated on `capabilities`, so when that read fails none of them is
  // visible — which also means `configurationError` cannot fire, because it
  // only reports when a VISIBLE section fails. With no tabs and no notice the
  // operator gets a page title over empty space.
  //
  // This is not the rare case: a device that is offline, or onboarded without
  // credentials yet, answers `capabilities` with a 400 and lands here. The
  // device is what failed, so it belongs in the same full-page notice the
  // section failures already use — which knows how to say "check credentials"
  // rather than showing nothing.
  const consoleFailure = capabilitiesQuery.isError
    ? capabilitiesQuery.error
    : configurationError;

  const retryAll = () => {
    for (const query of [capabilitiesQuery, infoQuery, usersQuery, diskQuery, recordingQuery])
      retry(query);
  };

  // Three states, named rather than nested: a chain of ternaries in the markup
  // hides which one you are looking at.
  if (consoleFailure) {
    return (
      <div className={cn("flex h-full min-h-0 flex-col", className)}>
        <div className="relative min-h-64 flex-1">
          <ConnectionFailureOverlay
            failure={connectionFailureFromApiError(consoleFailure)}
            onRetry={retryAll}
          />
        </div>
      </div>
    );
  }

  // Until capabilities arrive there are no tabs to draw. Without this the
  // console flashes an empty frame on every open.
  if (capabilitiesQuery.isPending && enabled) {
    return (
      <div className={cn("flex h-full min-h-0 flex-col", className)}>
        <div className="flex min-h-64 flex-1 flex-col gap-3 p-4" aria-busy="true">
          <Skeleton className="h-10 w-72" />
          <Skeleton className="min-h-0 flex-1" />
        </div>
      </div>
    );
  }

  return (
    <div className={cn("flex h-full min-h-0 flex-col", className)}>
      <Tabs
          value={selectedTab}
          onValueChange={(value) => setActiveTab(value as CctvTab)}
          className="flex min-h-0 flex-1 flex-col"
        >
          <div className="sticky top-0 z-20 shrink-0 bg-background px-4 py-3">
            <SectionTabs
              items={tabs}
            />
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto p-4">
            {showLive && (
              // `h-full min-h-0`: the live picture is the one tab whose content
              // should fit the pane instead of setting its height. Without a
              // definite height here the card's `max-h-full` resolves against an
              // auto-height Radix wrapper and does nothing, so at 1440x900 the
              // frame ended 27px below the fold and the pane scrolled.
              <TabsContent
                value="cameras"
                forceMount
                className="mt-0 flex h-full min-h-0 flex-col data-[state=inactive]:hidden"
              >
                <CameraControl
                  key={targetId}
                  targetId={targetId}
                  capabilities={capabilities}
                  infoChannelCount={infoQuery.data?.channels}
                  active={active && selectedTab === "cameras"}
                />
              </TabsContent>
            )}
            {showArchive && (
              <TabsContent value="archive" forceMount className="mt-0 data-[state=inactive]:hidden">
                <RecordingCard
                  query={recordingQuery}
                  targetId={targetId}
                  canExport={Boolean(capabilities?.export && capabilities.permissions.export)}
                  // Watching history is the `cctv:recordings` grant the tab
                  // itself is gated on; it needs the same driver archive path
                  // export uses, so a family without one hides the player.
                  canWatch={Boolean(capabilities?.export)}
                  channels={archiveChannels}
                  channelCount={infoQuery.data?.channels ?? null}
                />
              </TabsContent>
            )}
            {showUsers && (
              <TabsContent value="users" forceMount className="mt-0 data-[state=inactive]:hidden">
                <UsersCard
                  query={usersQuery}
                  targetId={targetId}
                  canCreate={canCreateUsers}
                  canUpdate={canUpdateUsers}
                  canDelete={canDeleteUsers}
                  hasAuthorityCatalog={Boolean(capabilities?.authority_catalog)}
                />
              </TabsContent>
            )}
            {showDevice && (
              <TabsContent value="device" forceMount className="mt-0 grid grid-cols-1 gap-4 data-[state=inactive]:hidden xl:grid-cols-2">
                {showIdentity && <IdentityCard query={infoQuery} />}
                {showDisk && <DiskCard query={diskQuery} />}
              </TabsContent>
            )}
          </div>
      </Tabs>
    </div>
  );
};

// ── Shared card scaffolding ──────────────────────────────

interface CardShellProps {
  icon: ElementType;
  title: string;
  children: ReactNode;
  className?: string;
}

const CardShell = ({ icon: Icon, title, children, className }: CardShellProps) => (
  <SurfaceCard className={cn("flex flex-col", className)}>
    <SurfaceCardHeader title={<span className="flex items-center gap-2"><Icon className="size-4 text-primary" aria-hidden="true" />{title}</span>} />
    <SurfaceCardContent className="min-h-0 flex-1">{children}</SurfaceCardContent>
  </SurfaceCard>
);

/**
 * Per-section state gate. Renders a skeleton while loading, a small inline error
 * for genuine failures (credentials 400s are hoisted to the page-level notice),
 * and otherwise the section content.
 */
function SectionState<T>({
  query,
  children,
  skeletonRows = 3,
}: Readonly<{
  query: UseQueryResult<T, unknown>;
  children: (data: T) => ReactNode;
  skeletonRows?: number;
}>): ReactNode {
  if (query.isPending) {
    return (
      <div className="space-y-2">
        {Array.from({ length: skeletonRows }, (_, index) => `skeleton-${index}`).map((key) => (
          <Skeleton key={key} className="h-8 w-full" />
        ))}
      </div>
    );
  }
  if (query.isError) {
    const failure = connectionFailureFromApiError(query.error);
    return (
      <ConnectionFailureOverlay
        failure={failure}
        onRetry={failure.kind === "unsupported" ? undefined : () => retry(query)}
        presentation="section"
      />
    );
  }
  return <>{children(query.data)}</>;
}

// ── Identity ─────────────────────────────────────────────

const IdentityCard = ({ query }: { query: UseQueryResult<DeviceInfo, unknown> }) => {
  const { t } = useTranslation();
  const dash = "—";
  return (
    <CardShell icon={Cpu} title={t("deviceDashboard.identity", "Device identity")}>
      <SectionState query={query} skeletonRows={4}>
        {(info) => (
          <dl className="grid grid-cols-2 gap-x-4 gap-y-3">
            <Field label={t("deviceDashboard.model", "Model")} value={info.model?.trim() || dash} />
            <Field
              label={t("deviceDashboard.firmware", "Firmware")}
              value={info.firmware?.trim() || dash}
              mono
            />
            <Field
              label={t("deviceDashboard.serial", "Serial")}
              value={info.serial?.trim() || dash}
              mono
            />
            <Field
              label={t("deviceDashboard.channels", "Channels")}
              value={info.channels != null ? String(info.channels) : dash}
            />
            <Field
              label={t("deviceDashboard.deviceTime", "Device time")}
              value={formatTimestamp(info.device_time) || dash}
              className="col-span-2"
            />
          </dl>
        )}
      </SectionState>
    </CardShell>
  );
};

const Field = ({
  label,
  value,
  mono,
  className,
}: {
  label: string;
  value: string;
  mono?: boolean;
  className?: string;
}) => (
  <div className={cn("min-w-0", className)}>
    <dt className="text-sm font-medium text-muted-foreground">{label}</dt>
    <dd className={cn("truncate text-sm text-foreground", mono && "font-mono text-[0.6875rem]")} title={value}>
      {value}
    </dd>
  </div>
);

// ── Recording ────────────────────────────────────────────

const RecordingCard = ({
  query,
  targetId,
  canExport,
  canWatch,
  channels,
  channelCount,
}: {
  query: UseQueryResult<DeviceRecording, unknown>;
  targetId: string;
  canExport: boolean;
  /** Driver and permissions allow streaming recorded footage back. */
  canWatch: boolean;
  channels: readonly DeviceChannel[];
  channelCount: number | null;
}) => {
  const { t } = useTranslation();
  const [exportSeed, setExportSeed] = useState<ExportSeed | null>(null);
  return (
    <CardShell icon={Clock} title={t("deviceDashboard.recording", "Recording")}>
      <SectionState query={query} skeletonRows={2}>
        {(recording) => (
          <div className="space-y-4">
            <div className="flex flex-col gap-3 rounded-lg border border-border bg-muted p-4 sm:flex-row sm:items-center sm:justify-between">
              <div>
                <p className="text-xs font-medium text-muted-foreground">
                  {t("deviceDashboard.footageWindow", "Footage window")}
                </p>
                <p className="mt-1 text-sm font-medium text-foreground">
                  {recording.oldest || recording.newest
                    ? `${formatTimestamp(recording.oldest) || "—"} → ${formatTimestamp(recording.newest) || "—"}`
                    : t("deviceDashboard.noFootage", "No footage retained")}
                </p>
              </div>
              {canExport && (
                <ClipExport
                  targetId={targetId}
                  channelCount={channelCount}
                  defaultEnd={recording.newest ?? null}
                  seed={exportSeed}
                  onSeedConsumed={() => setExportSeed(null)}
                />
              )}
            </div>

            {canWatch && (
              <ArchivePlayer
                targetId={targetId}
                channels={channels}
                retention={{ oldest: recording.oldest ?? null, newest: recording.newest ?? null }}
                spans={recording.spans}
                canExport={canExport}
                // Carry the operator's chosen window into the export dialog, so
                // "watch this, now save it" does not mean re-entering the range.
                onExport={(channel, startMs, endMs) =>
                  setExportSeed({ channel, startMs, endMs, nonce: Date.now() })
                }
              />
            )}
            <div className="flex items-center justify-between">
              <span className="text-xs text-muted-foreground">
                {t("deviceDashboard.recordingStatus", "Status")}
              </span>
              <StatusChip
                status={recording.active ? "positive" : "neutral"}
                label={
                  recording.active
                    ? t("deviceDashboard.recordingActive", "Recording")
                    : t("deviceDashboard.recordingIdle", "Idle")
                }
              />
            </div>
            <div className="flex items-center justify-between">
              <span className="text-xs text-muted-foreground">
                {t("deviceDashboard.spans", "Recorded spans")}
              </span>
              <Badge variant="status-neutral">{recording.spans.length}</Badge>
            </div>
            {recording.spans.length > 0 && (
              <ol className="divide-y divide-border overflow-hidden rounded-lg border border-border">
                {recording.spans.map((span) => (
                  <li key={`${span.start}-${span.end}`} className="flex items-center justify-between gap-4 px-3 py-2 text-xs">
                    <span className="text-muted-foreground">{formatTimestamp(span.start)}</span>
                    <span aria-hidden className="text-muted-foreground">→</span>
                    <span className="text-right font-medium text-foreground">{formatTimestamp(span.end)}</span>
                  </li>
                ))}
              </ol>
            )}
          </div>
        )}
      </SectionState>
    </CardShell>
  );
};

// ── Disk health ──────────────────────────────────────────

const DiskCard = ({ query }: { query: UseQueryResult<DeviceDisk, unknown> }) => {
  const { t } = useTranslation();
  return (
    <CardShell icon={HardDrive} title={t("deviceDashboard.diskHealth", "Disk health")}>
      <SectionState query={query} skeletonRows={2}>
        {(disk) =>
          disk.disks.length === 0 ? (
            <p className="py-6 text-center text-xs text-muted-foreground">
              {t("deviceDashboard.noDisks", "No storage devices reported.")}
            </p>
          ) : (
            <ul className="space-y-3">
              {disk.disks.map((entry, index) => (
                <DiskRow key={`${entry.name}-${index}`} entry={entry} />
              ))}
            </ul>
          )
        }
      </SectionState>
    </CardShell>
  );
};

const DiskRow = ({ entry }: { entry: DiskEntry }) => {
  const { t } = useTranslation();
  const total = Math.max(entry.total_bytes, 0);
  const used = Math.min(Math.max(entry.used_bytes, 0), total || entry.used_bytes);
  const pct = total > 0 ? Math.min(100, Math.round((used / total) * 100)) : 0;
  const errored = entry.is_error || entry.state === "error";
  const barColor = diskBarColor(errored, pct);
  const stateLabels: Record<DiskState, string> = {
    ok: t("deviceDashboard.diskOk", "OK"),
    read_only: t("deviceDashboard.diskReadOnly", "Read-only"),
    error: t("deviceDashboard.diskError", "Error"),
    unknown: t("deviceDashboard.diskUnknown", "Unknown"),
  };
  const label = entry.is_error ? t("deviceDashboard.diskError", "Error") : stateLabels[entry.state];
  return (
    <li className="space-y-1.5">
      <div className="flex items-center justify-between gap-2">
        <span className="truncate font-mono text-sm font-medium text-foreground" title={entry.name}>
          {entry.name}
        </span>
        <StatusChip status={diskStateStatus(entry)} label={label} />
      </div>
      <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
        <div className={cn("h-full rounded-full transition-all", barColor)} style={{ width: `${pct}%` }} />
      </div>
      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <span>
          {formatBytes(used)} / {total > 0 ? formatBytes(total) : "—"}
        </span>
        <span>{total > 0 ? `${pct}%` : "—"}</span>
      </div>
    </li>
  );
};

type DiskState = DiskEntry["state"];

function diskBarColor(errored: boolean, pct: number): string {
  if (errored) return "bg-status-negative";
  if (pct >= 90) return "bg-status-warning";
  return "bg-status-positive";
}

function diskStateStatus(entry: DiskEntry): "positive" | "warning" | "negative" | "neutral" {
  if (entry.is_error) return "negative";
  const map: Record<DiskState, "positive" | "warning" | "negative" | "neutral"> = {
    ok: "positive",
    read_only: "warning",
    error: "negative",
    unknown: "neutral",
  };
  return map[entry.state];
}

// ── Users ────────────────────────────────────────────────

const UsersCard = ({
  query,
  targetId,
  canCreate,
  canUpdate,
  canDelete,
  hasAuthorityCatalog,
}: {
  query: UseQueryResult<DeviceUsers, unknown>;
  targetId: string;
  canCreate: boolean;
  canUpdate: boolean;
  canDelete: boolean;
  hasAuthorityCatalog: boolean;
}) => {
  const { t } = useTranslation();
  return (
    <CardShell icon={Users} title={t("deviceDashboard.users", "User accounts")}>
      <SectionState query={query} skeletonRows={3}>
        {(data) => (
          <DeviceUserSection
            targetId={targetId}
            users={data.users}
            canCreate={canCreate}
            canUpdate={canUpdate}
            canDelete={canDelete}
            hasAuthorityCatalog={hasAuthorityCatalog}
          />
        )}
      </SectionState>
    </CardShell>
  );
};

// ── Formatting ───────────────────────────────────────────

function formatTimestamp(value: string | null | undefined): string {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}
