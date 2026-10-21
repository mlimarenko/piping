import { useState } from "react";
import { useInspectorSections } from "./useInspectorSections";
import InspectorSection from "./InspectorSection";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import InspectorFieldLabel from "@/shared/ui-shared/InspectorFieldLabel";
import StatusChip from "@/shared/ui-shared/StatusChip";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import { Button } from "@/shared/ui-primitives/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import {
  Sheet, SheetContent, SheetHeader, SheetTitle, SheetDescription,
} from "@/shared/ui-primitives/sheet";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import {
  X, Terminal, ExternalLink, Network, Map as MapIcon, Building2,
  Cpu, HardDrive, Layers, Link2, FileText, Loader2, Gauge,
  ClipboardList, ListTree,
} from "lucide-react";
import type { OpenableProtocol } from "./launchPresentation";
import ConnectButtons from "./ConnectButtons";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type { AdHocCredentialsContext } from "./AdHocCredentialsDialog";
import { cn } from "@/shared/lib/utils";
import { writeBrowserClipboardText } from "@/shared/lib/clipboard";
import { connectionsHostUrl } from "@/features/connections/workspaceLinks";
import { connectionTargetFallbackIcon } from "@/shared/entity-icon-utils";
import { EntityIcon } from "@/shared/entity-icons";
import HostMetricsDialog from "@/features/observability/HostMetricsDialog";
import HostMetricsPreviewSection from "@/features/observability/HostMetricsPreviewSection";
import HostPlaybooksPreviewSection from "@/features/playbooks/HostPlaybooksPreviewSection";
import type { PlaybookTargetAssignment } from "@/features/playbooks/api";
import type { ConnectionTarget, InventoryEntityKey } from "@/shared/api/types";
import NestedEditorPanel from "@/features/inventory/components/inventory/NestedEditorPanel";
import RenderedConfigContextPanel from "@/features/inventory/components/config-contexts/RenderedConfigContextPanel";
import { relationshipsByTab } from "@/features/inventory/components/inventory/inventoryTypes";
import { notify } from "@/shared/lib/notifications";
import { listAuditLogs } from "@/features/audit/api";
import { inventoryResourceStatusLabel } from "@/features/inventory/resourceStatus";
import { hostStatusMap, hostSubtitle, hostValue } from "./hostPresentation";
import { useDeferredInspectorQueries } from "./useDeferredInspectorQueries";
import { InspectorFieldRow } from "./InspectorFieldRow";

interface Props {
  host: ConnectionTarget;
  onClose: () => void;
  onConnect: (host: ConnectionTarget, protocol: OpenableProtocol, entryId?: string, username?: string) => void;
  /** Opens the ad-hoc credentials dialog (mounted at the workspace level). */
  onOpenAdHoc?: (context: AdHocCredentialsContext) => void;
  /** When true, render full-width as a row below the list instead of a fixed-width side column */
  stacked?: boolean;
  /**
   * Explicit column width in px. When set, the inspector fills that width
   * instead of its default fixed `md:w-80`, so the workspace owns resizing.
   */
  width?: number;
  /** Called when the user selects a playbook from this inspector. The workspace owns confirmation and launch. */
  onPlaybookQuickAction?: (host: ConnectionTarget, item: PlaybookTargetAssignment) => void;
}

const ConnectionInspector = ({
  host,
  onClose,
  onConnect,
  onOpenAdHoc,
  stacked = false,
  width,
  onPlaybookQuickAction,
}: Props) => {
  const { t } = useTranslation();
  const { can } = usePermissions();
  // Hide the whole Connect section (header included) from a READ-without-CONNECT
  // role so it never reads as an empty/orphaned section. CONNECT is held by
  // every seed role; this only affects custom roles.
  const canConnect = can("CONNECT", "CONNECTION_TARGET");
  const { expandedSections, toggleSection } = useInspectorSections();
  const [showDetails, setShowDetails] = useState(false);
  const [showMetrics, setShowMetrics] = useState(false);
  // Inline nested-CRUD editor (Device → Interfaces → IP, VM → VM Interface → IP)
  // mounted in a side drawer — the real pain entry point: edit a device's linked
  // records without leaving the connections workspace.
  const [showNested, setShowNested] = useState(false);
  const nestedEntity: InventoryEntityKey =
    host.targetKind === "virtual_machine" ? "Virtual Machines" : "Devices";
  const hasNestedEditor = (relationshipsByTab[nestedEntity] ?? []).length > 0;
  const canEditInventory = can("UPDATE", "INVENTORY");
  const [hasConfiguredMetrics, setHasConfiguredMetrics] = useState(false);
  const [hasEligiblePlaybooks, setHasEligiblePlaybooks] = useState(false);

  // Reset the metrics panel when the inspected host changes. Done during render
  // via a key sentinel (React's recommended alternative to a setState-in-effect)
  // so switching hosts never triggers a cascading re-render.
  const [prevHostId, setPrevHostId] = useState(host.id);
  if (prevHostId !== host.id) {
    setPrevHostId(host.id);
    setHasConfiguredMetrics(false);
    setHasEligiblePlaybooks(false);
    setShowMetrics(false);
    setShowNested(false);
  }
  const deferredQueriesEnabled = useDeferredInspectorQueries(host.id);

  return (
    <>
      <section
        className={cn(
          "@container flex min-h-0 flex-1 flex-col overflow-hidden bg-background text-sm",
          stacked
            ? "w-full h-full min-h-0"
            : "w-full md:w-80",
        )}
        style={!stacked && width ? { width } : undefined}
      >
        {/* Identity shares the scroll viewport so long names cannot displace details. */}
        <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="shrink-0 p-3 border-b border-border/30">
          <div className="flex items-center justify-between mb-1.5">
            <div className="flex items-center gap-2 min-w-0">
              <div className="h-9 w-9 rounded-md surface-inset flex items-center justify-center shrink-0">
                <EntityIcon
                  fallback={connectionTargetFallbackIcon(host)}
                  iconName={host.roleIconName}
                  customDataUrl={host.roleIconDataUrl}
                  className="h-3.5 w-3.5 text-primary"
                  iconClassName="h-3.5 w-3.5"
                />
              </div>
              <div className="min-w-0">
                <h2 className="text-sm font-semibold text-foreground [overflow-wrap:anywhere]">{host.name}</h2>
                <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">{hostSubtitle([host.site, host.tenant])}</p>
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              {/* Shareable workspace deep-link to this host. */}
              <button
                type="button"
                onClick={() => {
                  void writeBrowserClipboardText(connectionsHostUrl(host.id)).then(
                    () => notify("success", t("connections.inspector.linkCopied")),
                    () => undefined,
                  );
                }}
                className="inline-flex h-9 w-9 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                aria-label={t("connections.inspector.copyLink")}
                title={t("connections.inspector.copyLink")}
              >
                <Link2 className="h-3.5 w-3.5" />
              </button>
              <Button variant="ghost" size="icon-sm" onClick={onClose} aria-label={t("close")}>
                <X className="h-4 w-4" />
              </Button>
            </div>
          </div>
        </div>

        <div>
          <HostMetricsPreviewSection
            host={host}
            kind={host.targetKind === "virtual_machine" ? "vm" : "device"}
            visible={false}
            enabled={deferredQueriesEnabled}
            onConfiguredMetricsChange={setHasConfiguredMetrics}
          />
          <HostPlaybooksPreviewSection
            host={host}
            visible={false}
            enabled={deferredQueriesEnabled}
            onAvailabilityChange={setHasEligiblePlaybooks}
          />

          {canConnect && (
            <InspectorSection title={t("connections.inspector.connections")} icon={Terminal} sectionKey="connect" expanded={expandedSections} onToggle={toggleSection}>
              <ConnectButtons host={host} onConnect={onConnect} onOpenAdHoc={onOpenAdHoc} />
            </InspectorSection>
          )}

          {hasEligiblePlaybooks && (
            <InspectorSection title={t("playbooks.inspector.title")} icon={ClipboardList} sectionKey="playbooks" expanded={expandedSections} onToggle={toggleSection}>
              <HostPlaybooksPreviewSection
                host={host}
                enabled={deferredQueriesEnabled}
                layout="buttons"
                onAvailabilityChange={setHasEligiblePlaybooks}
                onLaunch={(item) => onPlaybookQuickAction?.(host, item)}
              />
            </InspectorSection>
          )}

          <InspectorSection title={t("connections.inspector.info")} icon={Cpu} sectionKey="overview" expanded={expandedSections} onToggle={toggleSection}>
            <InspectorFieldRow t={t} icon={Cpu} label={t("connections.columns.role")} help={connectionFieldHelp(t, "role")} value={host.role} />
            <InspectorFieldRow t={t} icon={Layers} label={t("connections.columns.platform")} help={connectionFieldHelp(t, "platform")} value={host.platform} />
            <InspectorFieldRow t={t} icon={HardDrive} label={t("connections.columns.type")} help={connectionFieldHelp(t, "type")} value={host.deviceType} />
            <InspectorFieldRow t={t} icon={Network} label={t("connections.columns.primaryIp")} help={connectionFieldHelp(t, "primaryIp")} value={host.primaryIp} mono />
          </InspectorSection>

          <InspectorSection title={t("connections.inspector.metadata")} icon={Building2} sectionKey="relations" expanded={expandedSections} onToggle={toggleSection}>
            <InspectorFieldRow t={t} icon={Building2} label={t("connections.columns.site")} help={connectionFieldHelp(t, "site")} value={host.site} />
            <InspectorFieldRow t={t} icon={Building2} label={t("connections.columns.tenant")} help={connectionFieldHelp(t, "tenant")} value={host.tenant} />
            <InspectorFieldRow t={t} icon={MapIcon} label={t("connections.filtersPanel.region")} help={connectionFieldHelp(t, "region")} value={host.region} />
            <InspectorFieldRow t={t} icon={Layers} label={t("connections.filtersPanel.cluster")} help={connectionFieldHelp(t, "cluster")} value={host.cluster} />
          </InspectorSection>

          {/* Config context — rendered, inheritance-resolved. Collapsed by
              default so it only fetches when opened. */}
          <InspectorSection title={t("configContexts.rendered.section")} icon={Layers} sectionKey="config-context" expanded={expandedSections} onToggle={toggleSection}>
            <RenderedConfigContextPanel
              kind={host.targetKind === "virtual_machine" ? "virtual-machine" : "device"}
              entityId={(host.targetKind === "virtual_machine" ? host.vmId : host.deviceId) ?? host.id}
              showHeader={false}
            />
          </InspectorSection>

          {hasConfiguredMetrics && (
            <InspectorSection title={t("observability.selectedMetrics")} icon={Gauge} sectionKey="metrics" expanded={expandedSections} onToggle={toggleSection}>
              <HostMetricsPreviewSection
                host={host}
                kind={host.targetKind === "virtual_machine" ? "vm" : "device"}
                onConfiguredMetricsChange={setHasConfiguredMetrics}
              />
            </InspectorSection>
          )}
        </div>

        </div>
        {/* Footer actions. The CCTV console is opened from the connect panel
            (the `CCTV` launch button), like SSH/RDP/VNC — no separate footer
            entry, so the device has one consistent open flow. */}
        <div className="p-2 border-t border-border/30 flex gap-1">
          {hasConfiguredMetrics && (
            <Button variant="ghost" size="sm" className="h-auto min-h-9 whitespace-normal text-sm gap-1 flex-1" onClick={() => setShowMetrics(true)}>
              <Gauge className="h-3 w-3" /> {t("observability.metrics")}
            </Button>
          )}
          {hasNestedEditor && (
            <Button variant="ghost" size="sm" className="h-auto min-h-9 whitespace-normal text-sm gap-1 flex-1" onClick={() => setShowNested(true)}>
              <ListTree className="h-3 w-3" /> {t("nestedEditor.title")}
            </Button>
          )}
          <Button variant="ghost" size="sm" className="h-auto min-h-9 whitespace-normal text-sm gap-1 flex-1" onClick={() => setShowDetails(true)}>
            <ExternalLink className="h-3 w-3" /> {t("auditLog.detailsLabel")}
          </Button>
        </div>
      </section>

      {/* Details Dialog */}
      <HostDetailsDialog host={host} open={showDetails} onOpenChange={setShowDetails} />
      {hasConfiguredMetrics && (
        <HostMetricsDialog host={host} open={showMetrics} onOpenChange={setShowMetrics} />
      )}

      {/* Nested-CRUD editor drawer — edit the device/VM's linked records inline. */}
      {hasNestedEditor && (
        <Sheet open={showNested} onOpenChange={setShowNested}>
          <SheetContent side="right" className="w-full overflow-y-auto sm:max-w-md">
            <SheetHeader>
              <SheetTitle className="flex items-center gap-2 text-sm">
                <ListTree className="h-4 w-4 text-primary" />
                {t("nestedEditor.title")} — {host.name}
              </SheetTitle>
              <SheetDescription className="text-xs">
                {t("connections.inspector.detailsDescription", { name: host.name })}
              </SheetDescription>
            </SheetHeader>
            <div className="mt-4">
              {showNested && (
                <NestedEditorPanel
                  key={host.id}
                  entity={nestedEntity}
                  rootRealId={host.id}
                  readOnly={!canEditInventory}
                />
              )}
            </div>
          </SheetContent>
        </Sheet>
      )}
    </>
  );
};

/* ── Details Dialog ── */
const HostDetailsDialog = ({ host, open, onOpenChange }: { host: ConnectionTarget; open: boolean; onOpenChange: (v: boolean) => void }) => {
  const { t } = useTranslation();
  const activityQuery = useQuery({
    queryKey: ["audit", "host", host.id],
    queryFn: () => listAuditLogs({ targetIds: [host.id], limit: 20 }),
    enabled: open,
  });
  const activityEntries = activityQuery.data?.items ?? [];

  return (
  <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent className="max-w-2xl max-h-[85vh] overflow-y-auto surface border-border/40" description={t("connections.inspector.detailsDescription", { name: host.name })}>
      <DialogHeader>
        <DialogTitle className="flex items-center gap-2 text-base">
          <FileText className="h-4 w-4 text-primary" />
          {t("auditLog.detailsLabel")} — {host.name}
        </DialogTitle>
      </DialogHeader>

      <Tabs defaultValue="general" className="mt-2">
        <SectionTabs
          items={[
            { value: "general", label: t("connections.inspector.info") },
            { value: "activity", label: t("connections.inspector.events") },
          ]}
          wrapperClassName="mb-4"
          className="surface-chip h-9"
          triggerClassName="h-9 px-3 text-sm"
        />

        <TabsContent value="general" className="space-y-1">
          <DetailRow label={t("connections.columns.name")} help={connectionFieldHelp(t, "name")} value={host.name} mono />
          <DetailRow
            label={t("status")}
            help={connectionFieldHelp(t, "status")}
            value={inventoryResourceStatusLabel(t, host.status) || undefined}
            chip={!!host.status}
            chipStatus={host.status ? hostStatusMap[host.status] : undefined}
          />
          <DetailRow label={t("connections.columns.platform")} help={connectionFieldHelp(t, "platform")} value={host.platform} />
          <DetailRow label={t("connections.columns.type")} help={connectionFieldHelp(t, "type")} value={host.deviceType} />
          <DetailRow label={t("connections.columns.role")} help={connectionFieldHelp(t, "role")} value={host.role} />
          <DetailRow label={t("connections.columns.site")} help={connectionFieldHelp(t, "site")} value={host.site} />
          <DetailRow label={t("connections.columns.tenant")} help={connectionFieldHelp(t, "tenant")} value={host.tenant} />
          <DetailRow label={t("connections.filtersPanel.region")} help={connectionFieldHelp(t, "region")} value={host.region} />
          <DetailRow label={t("connections.filtersPanel.cluster")} help={connectionFieldHelp(t, "cluster")} value={host.cluster} />
          <DetailRow label="ID" help={connectionFieldHelp(t, "id")} value={host.id} mono />
        </TabsContent>

        <TabsContent value="activity" className="space-y-2">
          {activityQuery.isPending ? (
            <div className="flex items-center justify-center py-6 text-xs text-muted-foreground">
              <Loader2 className="h-3.5 w-3.5 animate-spin mr-2" /> {t("auditLog.loadingEvents")}
            </div>
          ) : activityEntries.length === 0 ? (
            <p className="text-xs text-muted-foreground text-center py-6">{t("auditLog.noEntries")}</p>
          ) : (
            activityEntries.map((entry) => (
              <div key={entry.id} className="flex items-start gap-3 px-3 py-2 rounded-md hover:bg-accent transition-colors">
                <div className="w-6 h-6 rounded-md surface-inset flex items-center justify-center shrink-0 mt-0.5">
                  <Terminal className="h-3 w-3 text-muted-foreground" />
                </div>
                <div className="flex-1 min-w-0">
                  <p className="text-sm text-foreground">{entry.summary || entry.action}</p>
                  <p className="text-xs text-muted-foreground">
                    {entry.actor || "—"} · {entry.timestamp}
                  </p>
                </div>
              </div>
            ))
          )}
        </TabsContent>
      </Tabs>
    </DialogContent>
  </Dialog>
  );
};

/* ── Shared sub-components ── */

const DetailRow = ({ label, help, value, mono, chip, chipStatus, icon: Icon }: {
  label: string; help?: string; value?: string | null; mono?: boolean; chip?: boolean;
  chipStatus?: "positive" | "info" | "warning" | "negative" | "neutral";
  icon?: React.ElementType;
}) => (
  <div className="grid grid-cols-[minmax(0,42%)_minmax(0,1fr)] items-start gap-2 px-3 py-1.5 rounded-md hover:bg-accent transition-colors">
    <div className="min-w-0 text-xs text-muted-foreground">
      <InspectorFieldLabel icon={Icon} label={label} help={help} />
    </div>
    {chip && chipStatus ? (
      <StatusChip status={chipStatus} label={hostValue(value)} />
    ) : (
      <span className={cn("min-w-0 whitespace-pre-wrap [overflow-wrap:anywhere] text-right text-sm text-foreground", mono && "font-mono text-xs")}>{hostValue(value)}</span>
    )}
  </div>
);


function connectionFieldHelp(t: (key: string) => string, key: string): string {
  return t(`connections.inspector.fieldHelp.${key}`);
}

export default ConnectionInspector;
