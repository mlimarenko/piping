import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import PageState from "@/shared/ui-shared/PageState";
import { useQuery } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronRight,
  Layers3,
  Loader2,
  PlayCircle,
  ServerCog,
  ShieldCheck,
} from "lucide-react";

import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import { Input } from "@/shared/ui-primitives/input";
import { Label } from "@/shared/ui-primitives/label";
import { Switch } from "@/shared/ui-primitives/switch";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { cn } from "@/shared/lib/utils";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import {
  previewPlaybookTargetSet,
  type PlaybookCatalogItem,
  type PlaybookConnection,
  type PlaybookTargetSet,
  type PlaybookTargetPreviewItem,
  type PlaybookTargetSetPreview,
} from "@/features/playbooks/api";
import HostMultiSelectPicker from "@/features/playbooks/HostMultiSelectPicker";
import { FormField, ToggleField } from "./PlaybookFormFields";
import SecretsPreflightPanel from "@/features/playbooks/SecretsPreflightPanel";
import {
  isPreflightRowBlocking,
  playbookSecretsPreflight,
} from "@/features/automation-secrets/api";
import type { SelectedHost } from "@/features/playbooks/hostSelection";
import type { ConnectionTarget } from "@/shared/api/types";
import {
  defaultLaunchValue,
  parseVarsSchema,
  validateVarsForm,
  varsFormToExtravars,
  type VarSpec,
} from "@/features/playbooks/varsSchema";

const CONNECTIONS: PlaybookConnection[] = ["ssh", "winrm", "local"];

/** Build the initial typed-vars values (defaults) for a set of declared specs. */
function initialVarValues(
  specs: VarSpec[],
): Record<string, string | number | boolean> {
  const values: Record<string, string | number | boolean> = {};
  for (const spec of specs) {
    values[spec.name] = defaultLaunchValue(spec);
  }
  return values;
}

export interface PlaybookLaunchTargetSummary {
  id: string;
  name: string;
  subtitle?: string | null;
  /** Source identity, carried for ad-hoc runs that build a static target set. */
  sourceId?: string | null;
  sourceKind?: "device" | "virtual_machine";
}

export interface PlaybookLaunchIntent {
  playbook?: PlaybookCatalogItem | null;
  jobTemplateId?: string | null;
  launchDisplayName?: string | null;
  targetSetId?: string | null;
  previewTargetSetId?: string | null;
  targetSetName?: string | null;
  targetSetMode?: string | null;
  triggerKind?: "manual" | "bulk";
  staticTargets?: PlaybookLaunchTargetSummary[];
  targetLimit?: number | null;
  timeoutSeconds?: number | null;
  dryRun?: boolean;
  credentialChoice?: string | null;
  metadata?: Record<string, unknown>;
}

export type PlaybookLaunchTargetMode = "target-set" | "hosts";

export interface PlaybookLaunchTargetingConfig {
  mode: PlaybookLaunchTargetMode;
  onModeChange: (mode: PlaybookLaunchTargetMode) => void;
  targetSets: PlaybookTargetSet[];
  selectedTargetSetId: string;
  onSelectedTargetSetIdChange: (targetSetId: string) => void;
  hostOptions: ConnectionTarget[];
  selectedHosts: SelectedHost[];
  onSelectedHostsChange: (hosts: SelectedHost[]) => void;
  hostsLoading?: boolean;
}

interface PlaybookLaunchDialogProps {
  open: boolean;
  intent: PlaybookLaunchIntent | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: (intent: PlaybookLaunchIntent, preview: PlaybookTargetSetPreview | null) => void;
  isSubmitting?: boolean;
  playbookOptions?: PlaybookCatalogItem[];
  selectedPlaybookId?: string;
  onSelectedPlaybookIdChange?: (playbookId: string) => void;
  playbooksLoading?: boolean;
  targeting?: PlaybookLaunchTargetingConfig;
}

interface PlaybookLaunchDialogFormProps extends PlaybookLaunchDialogProps {
  selectedPlaybook: PlaybookCatalogItem | null;
  varSpecs: VarSpec[];
  connectionDefault: PlaybookConnection;
}

function targetSubtitle(item: PlaybookTargetPreviewItem): string {
  return [item.site_name, item.role_name, item.address].filter(Boolean).join(" · ");
}

function targetSetDisplayName(targetSet: PlaybookTargetSet): string {
  return targetSet.name?.trim() || targetSet.mode;
}

function targetSetSearchText(targetSet: PlaybookTargetSet): string {
  return [
    targetSet.name,
    targetSet.mode,
    targetSet.selector ? JSON.stringify(targetSet.selector) : null,
  ]
    .filter(Boolean)
    .join(" ");
}

function selectedHostsToStaticTargets(hosts: SelectedHost[]): PlaybookLaunchTargetSummary[] {
  return hosts.map((host) => ({
    id: host.id,
    name: host.name,
    subtitle: host.subtitle,
    sourceId: host.sourceId,
    sourceKind: host.sourceKind,
  }));
}

function asRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return value as Record<string, unknown>;
}

function riskVariant(risk?: string | null) {
  if (risk === "low") return "status-active";
  if (risk === "medium") return "status-staging";
  return "status-failed";
}

const PLAYBOOK_RUN_MAX_TARGET_LIMIT = 50_000;

function launchResetSignature(
  open: boolean,
  selectedPlaybook: PlaybookCatalogItem | null,
  intent: PlaybookLaunchIntent | null,
): string | null {
  if (!open) return null;
  const staticTargetSig = (intent?.staticTargets ?? [])
    .map((target) => `${target.id}:${target.sourceId ?? ""}:${target.sourceKind ?? ""}`)
    .join(",");
  const metadataSig = intent?.metadata ? JSON.stringify(intent.metadata) : "";
  return [
    selectedPlaybook?.playbook.id ?? "none",
    selectedPlaybook?.current_version?.id ?? "none",
    intent?.targetSetId ?? "",
    intent?.previewTargetSetId ?? "",
    staticTargetSig,
    metadataSig,
    String(intent?.timeoutSeconds),
    String(intent?.dryRun),
  ].join("|");
}

export default function PlaybookLaunchDialog({
  open,
  intent,
  onOpenChange,
  onConfirm,
  isSubmitting = false,
  playbookOptions,
  selectedPlaybookId,
  onSelectedPlaybookIdChange,
  playbooksLoading = false,
  targeting,
}: PlaybookLaunchDialogProps) {
  const selectedPlaybook = useMemo(() => {
    if (playbookOptions && selectedPlaybookId) {
      return playbookOptions.find((item) => item.playbook.id === selectedPlaybookId) ?? null;
    }
    return intent?.playbook ?? null;
  }, [intent?.playbook, playbookOptions, selectedPlaybookId]);
  // The playbook is the single interpreter of its own typed launch variables and
  // default transport; the launch form is generated from these.
  const varSpecs = useMemo(
    () => parseVarsSchema(selectedPlaybook?.playbook.vars_schema),
    [selectedPlaybook?.playbook.vars_schema],
  );
  const connectionDefault: PlaybookConnection =
    selectedPlaybook?.playbook.connection_default ?? "ssh";
  const formKey = launchResetSignature(open, selectedPlaybook, intent) ?? "closed";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <PlaybookLaunchDialogForm
        key={formKey}
        open={open}
        intent={intent}
        onOpenChange={onOpenChange}
        onConfirm={onConfirm}
        isSubmitting={isSubmitting}
        playbookOptions={playbookOptions}
        selectedPlaybookId={selectedPlaybookId}
        onSelectedPlaybookIdChange={onSelectedPlaybookIdChange}
        playbooksLoading={playbooksLoading}
        targeting={targeting}
        selectedPlaybook={selectedPlaybook}
        varSpecs={varSpecs}
        connectionDefault={connectionDefault}
      />
    </Dialog>
  );
}

function PlaybookLaunchDialogForm({
  open,
  intent,
  onOpenChange,
  onConfirm,
  isSubmitting = false,
  playbookOptions,
  selectedPlaybookId,
  onSelectedPlaybookIdChange,
  playbooksLoading = false,
  targeting,
  selectedPlaybook,
  varSpecs,
  connectionDefault,
}: PlaybookLaunchDialogFormProps) {
  const { t } = useTranslation();

  const [varValues, setVarValues] = useState<
    Record<string, string | number | boolean>
  >(() => initialVarValues(varSpecs));
  const [varErrors, setVarErrors] = useState<Record<string, string>>({});
  const [connection, setConnection] = useState<PlaybookConnection>(connectionDefault);
  const [rawExtravars, setRawExtravars] = useState("{}");
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [rawError, setRawError] = useState<string | null>(null);
  const [timeoutValue, setTimeoutValue] = useState(() =>
    intent?.timeoutSeconds ? String(intent.timeoutSeconds) : "",
  );
  const [dryRun, setDryRun] = useState(() => Boolean(intent?.dryRun));
  const selectedTargetSet = targeting?.targetSets.find(
    (targetSet) => targetSet.id === targeting.selectedTargetSetId,
  );
  const targetingEnabled = Boolean(targeting);
  const effectiveTargetSetId = targetingEnabled
    ? targeting?.mode === "target-set"
      ? targeting.selectedTargetSetId || null
      : null
    : intent?.targetSetId ?? null;
  const previewTargetSetId = targetingEnabled
    ? effectiveTargetSetId
    : intent?.previewTargetSetId ?? intent?.targetSetId ?? null;
  const staticTargets =
    targetingEnabled && targeting?.mode === "hosts"
      ? selectedHostsToStaticTargets(targeting.selectedHosts)
      : intent?.staticTargets ?? [];
  const effectiveTargetSetName = targetingEnabled
    ? targeting?.mode === "hosts"
      ? t("playbooks.page.runDialog.adHocLabel", { count: staticTargets.length })
      : selectedTargetSet
        ? targetSetDisplayName(selectedTargetSet)
        : null
    : intent?.targetSetName ?? null;
  const effectiveTargetSetMode = targetingEnabled
    ? targeting?.mode === "target-set"
      ? selectedTargetSet?.mode ?? null
      : "static"
    : intent?.targetSetMode ?? null;
  const effectiveTriggerKind =
    targetingEnabled && targeting?.mode === "hosts"
      ? "bulk"
      : intent?.triggerKind ?? "manual";
  const effectiveMetadata = targetingEnabled
    ? {
        ...(intent?.metadata ?? {}),
        source: targeting?.mode === "hosts" ? "playbooks.multi_host" : "playbooks.catalog_default",
      }
    : intent?.metadata ?? {};
  const hasStaticTargets = staticTargets.length > 0;
  const hasServerPreview = Boolean(previewTargetSetId) && !hasStaticTargets;
  const previewQuery = useQuery({
    queryKey: ["playbooks", "launch-preview", previewTargetSetId],
    queryFn: () => previewPlaybookTargetSet(previewTargetSetId ?? "", 100),
    enabled: open && hasServerPreview,
  });
  const preview = hasServerPreview ? previewQuery.data ?? null : null;

  // Advisory C3 secret preflight: block-and-explain BEFORE queuing when the
  // caller lacks a grant or a declared secret has no value. Server gates
  // independently, so a pending/failed preflight never blocks a launch.
  const preflightPlaybookId = selectedPlaybook?.playbook.id ?? null;
  const preflightVersionId = selectedPlaybook?.current_version?.id ?? null;
  const secretsPreflightQuery = useQuery({
    queryKey: ["playbooks", "secrets-preflight", preflightPlaybookId, preflightVersionId],
    queryFn: () => playbookSecretsPreflight(preflightPlaybookId ?? "", preflightVersionId),
    enabled: open && Boolean(preflightPlaybookId) && Boolean(preflightVersionId),
  });
  const preflightEntries = secretsPreflightQuery.data?.secrets ?? [];
  const secretsBlocked = preflightEntries.some(isPreflightRowBlocking);
  const targetCount = hasStaticTargets ? staticTargets.length : preview?.total ?? 0;
  const targetRows = hasStaticTargets
    ? staticTargets.slice(0, 24)
    : (preview?.items ?? []).slice(0, 24).map((item) => ({
        id: item.target_id,
        name: item.name,
        subtitle: targetSubtitle(item),
      }));
  const hiddenTargetCount = Math.max(0, targetCount - targetRows.length);
  const targetLimit = targetCount > 0 ? Math.min(targetCount, PLAYBOOK_RUN_MAX_TARGET_LIMIT) : 0;
  const exceedsTargetLimit = targetCount > PLAYBOOK_RUN_MAX_TARGET_LIMIT;
  const timeoutSeconds = timeoutValue.trim()
    ? Number.parseInt(timeoutValue.trim(), 10)
    : null;
  const timeoutInvalid =
    timeoutValue.trim() !== "" &&
    (!Number.isFinite(timeoutSeconds) || (timeoutSeconds ?? 0) <= 0);
  const playbookRunnable =
    selectedPlaybook?.playbook.status === "active" && Boolean(selectedPlaybook.current_version);
  const previewBlocked =
    hasServerPreview && (previewQuery.isPending || previewQuery.isError || !preview);
  const canConfirm =
    Boolean(selectedPlaybook) &&
    playbookRunnable &&
    !isSubmitting &&
    !previewBlocked &&
    targetCount > 0 &&
    !exceedsTargetLimit &&
    !timeoutInvalid &&
    !secretsBlocked;
  const resolvedIntent: PlaybookLaunchIntent | null =
    intent && selectedPlaybook
      ? {
          ...intent,
          playbook: selectedPlaybook,
          targetSetId: effectiveTargetSetId,
          previewTargetSetId,
          targetSetName: effectiveTargetSetName,
          targetSetMode: effectiveTargetSetMode,
          triggerKind: effectiveTriggerKind,
          staticTargets,
          targetLimit: targetLimit > 0 ? targetLimit : null,
          timeoutSeconds,
          dryRun,
          credentialChoice: "auto",
          metadata: effectiveMetadata,
        }
      : null;
  const hasPlaybookPicker = Boolean(playbookOptions);
  const reviewStep = hasPlaybookPicker ? 2 : 1;
  const optionsStep = reviewStep + 1;
  const zeroTargetState = (() => {
    if (targeting?.mode === "hosts") {
      return {
        label: t("playbooks.launchDialog.noHostsSelected"),
        description: t("playbooks.launchDialog.noHostsSelectedDescription"),
      };
    }
    if (targeting?.mode === "target-set" && targeting.targetSets.length === 0) {
      return {
        label: t("playbooks.launchDialog.noTargetSetsReady"),
        description: t("playbooks.launchDialog.noTargetSetsReadyDescription"),
        actionLabel: t("playbooks.launchDialog.switchToHosts"),
        onAction: () => targeting.onModeChange("hosts"),
      };
    }
    if (targeting?.mode === "target-set" && !targeting.selectedTargetSetId) {
      return {
        label: t("playbooks.launchDialog.noTargetSetSelected"),
        description: t("playbooks.launchDialog.noTargetSetSelectedDescription"),
      };
    }
    const canSwitchToHosts = Boolean(targeting);
    if (!targetingEnabled) {
      return {
        label: t("playbooks.launchDialog.noTargets"),
        description: t("playbooks.launchDialog.noFixedTargetsDescription"),
      };
    }
    return {
      label: t("playbooks.launchDialog.noTargets"),
      description: hasServerPreview
        ? t(
            canSwitchToHosts
              ? "playbooks.launchDialog.noTargetsInSetDescription"
              : "playbooks.launchDialog.noTargetsInSetWithoutHostsDescription",
          )
        : t("playbooks.launchDialog.noTargetsDescription"),
      actionLabel: canSwitchToHosts ? t("playbooks.launchDialog.switchToHosts") : undefined,
      onAction: canSwitchToHosts ? () => targeting?.onModeChange("hosts") : undefined,
    };
  })();

  const setVar = (name: string, value: string | number | boolean) => {
    setVarValues((prev) => ({ ...prev, [name]: value }));
    setVarErrors((prev) => {
      if (!(name in prev)) return prev;
      const next = { ...prev };
      delete next[name];
      return next;
    });
  };

  const confirm = () => {
    if (!resolvedIntent) return;

    // Validate the typed variables form (required + number well-formedness).
    const fieldErrors = validateVarsForm(varSpecs, varValues);
    if (Object.keys(fieldErrors).length > 0) {
      setVarErrors(fieldErrors);
      return;
    }

    // Raw extravars is an optional JSON escape that merges OVER the typed
    // values (raw wins), so power users can set vars the form does not surface.
    let rawParsed: Record<string, unknown> = {};
    if (rawExtravars.trim() && rawExtravars.trim() !== "{}") {
      let parsed: unknown;
      try {
        parsed = JSON.parse(rawExtravars);
      } catch {
        setRawError(t("playbooks.launchDialog.rawExtravarsInvalid"));
        setAdvancedOpen(true);
        return;
      }
      if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
        setRawError(t("playbooks.launchDialog.rawExtravarsNotObject"));
        setAdvancedOpen(true);
        return;
      }
      rawParsed = parsed as Record<string, unknown>;
    }
    setRawError(null);

    const extravars = {
      ...varsFormToExtravars(varSpecs, varValues),
      ...rawParsed,
    };
    const inheritedLaunchOptions = asRecord(resolvedIntent.metadata?.launch_options);
    const inheritedExtravars = asRecord(inheritedLaunchOptions.extravars);

    onConfirm(
      {
        ...resolvedIntent,
        metadata: {
          ...(resolvedIntent.metadata ?? {}),
          launch_options: {
            ...inheritedLaunchOptions,
            // Canonical P3 handoff: the backend reads launch extravars from
            // `metadata.launch_options.extravars` (a flat object) and the
            // per-run transport override from `connection`.
            extravars: {
              ...inheritedExtravars,
              ...extravars,
            },
            connection,
            timeout_seconds: timeoutSeconds,
            dry_run: dryRun,
            credential_choice: "auto",
            target_limit: targetLimit > 0 ? targetLimit : null,
          },
        },
      },
      preview,
    );
  };

  return (
    <DialogContent className="flex max-h-[90dvh] flex-col overflow-y-auto p-0 sm:max-w-3xl">
        <DialogHeader className="border-b border-border/35 px-4 py-3">
          <DialogTitle>{t("playbooks.launchDialog.title")}</DialogTitle>
          <DialogDescription>{t("playbooks.launchDialog.description")}</DialogDescription>
        </DialogHeader>

        <fieldset disabled={isSubmitting} inert={isSubmitting} className="min-h-0 min-w-0 flex-1 space-y-3 overflow-y-auto p-4 stable-scrollbar-gutter">
          {playbookOptions ? (
            <StepShell step={1} title={t("playbooks.launchDialog.playbook")}>
              <div className="space-y-1.5">
                <SearchableSelect
                  value={selectedPlaybookId ?? ""}
                  onValueChange={(value) => onSelectedPlaybookIdChange?.(value)}
                  disabled={isSubmitting || playbookOptions.length === 0}
                  isLoading={playbooksLoading}
                  placeholder={t("playbooks.launchDialog.selectPlaybook")}
                  searchPlaceholder={t("search")}
                  items={playbookOptions.map((item) => ({
                    id: item.playbook.id,
                    label: item.playbook.name,
                    searchText: [item.playbook.slug, item.playbook.description].filter(Boolean).join(" "),
                  }))}
                />
                {playbooksLoading ? (
                  <p className="text-xs text-muted-foreground">
                    {t("loading")}
                  </p>
                ) : playbookOptions.length === 0 ? (
                  <p className="text-xs text-status-warning">
                    {t("playbooks.launchDialog.noRunnablePlaybooks")}
                  </p>
                ) : null}
              </div>
            </StepShell>
          ) : null}

          <StepShell step={reviewStep} title={t("playbooks.launchDialog.targets")}>
            <div className="space-y-3">
              {targeting ? (
                <div className="space-y-3 rounded-md border border-border bg-background p-3">
                  <FormField label={t("playbooks.launchDialog.targeting")}>
                    <div className="grid grid-cols-2 gap-1.5">
                      <TargetModeButton
                        active={targeting.mode === "target-set"}
                        icon={Layers3}
                        label={t("playbooks.launchDialog.modeTargetSet")}
                        hint={t("playbooks.launchDialog.modeTargetSetHint")}
                        onClick={() => targeting.onModeChange("target-set")}
                      />
                      <TargetModeButton
                        active={targeting.mode === "hosts"}
                        icon={ServerCog}
                        label={t("playbooks.launchDialog.modeHosts")}
                        hint={t("playbooks.launchDialog.modeHostsHint")}
                        onClick={() => targeting.onModeChange("hosts")}
                      />
                    </div>
                  </FormField>

                  {targeting.mode === "target-set" ? (
                    <FormField label={t("playbooks.launchDialog.targetSet")}>
                      <SearchableSelect
                        value={targeting.selectedTargetSetId}
                        onValueChange={targeting.onSelectedTargetSetIdChange}
                        disabled={isSubmitting || targeting.targetSets.length === 0}
                        items={targeting.targetSets.map((targetSet) => ({
                          id: targetSet.id,
                          label: targetSetDisplayName(targetSet),
                          searchText: targetSetSearchText(targetSet),
                        }))}
                        placeholder={t(
                          targeting.targetSets.length === 0
                            ? "playbooks.launchDialog.noTargetSets"
                            : "playbooks.launchDialog.selectTargetSet",
                        )}
                        searchPlaceholder={t("search")}
                      />
                    </FormField>
                  ) : (
                    <FormField label={t("playbooks.launchDialog.hosts")}>
                      <HostMultiSelectPicker
                        targets={targeting.hostOptions}
                        selected={targeting.selectedHosts}
                        onChange={targeting.onSelectedHostsChange}
                        loading={targeting.hostsLoading}
                      />
                    </FormField>
                  )}
                </div>
              ) : null}

              <div className="rounded-md border border-border bg-background p-3">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="text-xs text-muted-foreground">
                      {t("playbooks.launchDialog.playbook")}
                    </div>
                    <div className="mt-0.5 truncate text-sm font-semibold text-foreground">
                      {selectedPlaybook?.playbook.name ?? "—"}
                    </div>
                    <div className="mt-1 truncate text-xs text-muted-foreground">
                      {effectiveTargetSetName ?? preview?.target_set.name ?? t("playbooks.launchDialog.fixedSelection")}
                    </div>
                  </div>
                  {selectedPlaybook ? (
                    <Badge variant={riskVariant(selectedPlaybook.playbook.risk)} className="shrink-0 rounded-md">
                      {t(`playbooks.risk.${selectedPlaybook.playbook.risk}`)}
                    </Badge>
                  ) : null}
                </div>
                <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
                  <span
                    className={cn(
                      "rounded px-1.5 py-0.5",
                      targetCount > 0
                        ? "bg-status-positive/10 text-status-positive"
                        : "bg-status-warning/10 text-status-warning",
                    )}
                  >
                    {t("playbooks.launchDialog.summaryTargets", { count: targetCount })}
                  </span>
                  <span>{t("playbooks.launchDialog.summaryConnection", { connection: t(`playbooks.page.connection.${connection}`) })}</span>
                  <span>{t("playbooks.launchDialog.summaryTrigger", { trigger: t(`playbooks.launchDialog.triggers.${effectiveTriggerKind}`) })}</span>
                </div>
              </div>

              {selectedPlaybook && !playbookRunnable ? (
                <p className="rounded-md border border-status-warning bg-card px-3 py-2 text-sm text-status-warning">
                  {t("playbooks.launchDialog.notRunnable")}
                </p>
              ) : null}

              <div className="rounded-md border border-border bg-background">
                <div className="flex items-center justify-between gap-2 border-b border-border/30 px-3 py-2 text-xs">
                  <span className="font-medium text-foreground">{t("playbooks.launchDialog.targets")}</span>
                  <span className="text-muted-foreground">
                    {hasServerPreview && preview
                      ? t("playbooks.launchDialog.showing", {
                          shown: targetRows.length,
                          total: preview.total,
                        })
                      : t("playbooks.launchDialog.selectedCount", { count: targetCount })}
                  </span>
                </div>

                {hasServerPreview && previewQuery.isPending ? (
                  <PageState kind="loading" title={t("playbooks.launchDialog.loadingPreview")} />
                ) : hasServerPreview && previewQuery.isError ? (
                  <TargetRecoveryState
                    label={t("playbooks.launchDialog.previewFailed")}
                    description={t("playbooks.launchDialog.previewFailedDescription")}
                    actionLabel={t("retry")}
                    onAction={() => previewQuery.refetch()}
                  />
                ) : targetCount === 0 ? (
                  <TargetRecoveryState {...zeroTargetState} />
                ) : (
                  <div className="max-h-56 space-y-1 overflow-auto p-2 stable-scrollbar-gutter">
                    {targetRows.map((target) => (
                      <div
                        key={target.id}
                        className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)] gap-2 rounded-md bg-background px-2 py-2 text-sm"
                      >
                        <span className="truncate font-medium text-foreground">{target.name}</span>
                        <span className="truncate text-right text-muted-foreground">
                          {target.subtitle || "—"}
                        </span>
                      </div>
                    ))}
                    {hiddenTargetCount > 0 ? (
                      <div className="px-2 py-1 text-xs text-muted-foreground">
                        {t("playbooks.launchDialog.moreTargets", { count: hiddenTargetCount })}
                      </div>
                    ) : null}
                  </div>
                )}
              </div>

              {exceedsTargetLimit ? (
                <p className="rounded-md border border-status-warning bg-card px-3 py-2 text-sm text-status-warning">
                  {t("playbooks.launchDialog.maxTargetExceeded", {
                    count: targetCount,
                    max: targetLimit,
                  })}
                </p>
              ) : null}
            </div>
          </StepShell>

          <StepShell step={optionsStep} title={t("playbooks.launchDialog.runOptions")}>
            <div className="space-y-3">
              {/* Typed launch variables, generated from the playbook's vars_schema. */}
              {varSpecs.length > 0 ? (
                <div className="space-y-2 rounded-md border border-border bg-background p-3">
                  <div className="text-xs font-medium text-foreground">
                    {t("playbooks.launchDialog.variables")}
                  </div>
                  <div className="grid gap-3 sm:grid-cols-2">
                    {varSpecs.map((spec) => (
                      <VarField
                        key={spec.name}
                        spec={spec}
                        value={varValues[spec.name]}
                        error={varErrors[spec.name]}
                        onChange={(value) => setVar(spec.name, value)}
                      />
                    ))}
                  </div>
                </div>
              ) : null}

              <div className="grid gap-3 rounded-md border border-border bg-background p-3 sm:grid-cols-2">
                <FormField label={t("playbooks.launchDialog.connection")}>
                  <FormSelect
                    value={connection}
                    onValueChange={(value) => setConnection(value)}
                    options={CONNECTIONS.map((option) => ({
                      value: option,
                      label: t(`playbooks.page.connection.${option}`),
                    }))}
                  />
                </FormField>
                <FormField label={t("playbooks.launchDialog.timeoutSeconds")}>
                  <Input
                    className="h-9 text-sm"
                    inputMode="numeric"
                    value={timeoutValue}
                    onChange={(event) => setTimeoutValue(event.target.value)}
                    placeholder={selectedPlaybook?.current_version?.timeout_seconds
                      ? String(selectedPlaybook.current_version.timeout_seconds)
                      : "900"}
                  />
                  {timeoutInvalid ? (
                    <p className="text-xs text-status-warning">
                      {t("playbooks.launchDialog.timeoutInvalid")}
                    </p>
                  ) : null}
                </FormField>

                <div className="sm:col-span-2">
                  <button
                    type="button"
                    className="flex items-center gap-1 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground"
                    aria-expanded={advancedOpen}
                    onClick={() => setAdvancedOpen((prev) => !prev)}
                  >
                    {advancedOpen ? (
                      <ChevronDown className="h-3.5 w-3.5" />
                    ) : (
                      <ChevronRight className="h-3.5 w-3.5" />
                    )}
                    {t("playbooks.launchDialog.advanced")}
                  </button>
                  {advancedOpen ? (
                    <div className="mt-2 space-y-3 rounded-md border border-border bg-background p-3">
                      {/*
                       * Credential = `auto` by design. A run targets a SET of hosts, so the
                       * credential is resolved per-host by the connection policy (the only
                       * semantically correct default for multi-host runs). An explicit
                       * `shared_credential_set`/`connection_config` override is deferred:
                       * there is no frontend-reachable list endpoint for those credential
                       * kinds yet (the user-credentials endpoint is personal-only and does
                       * not match the kinds the playbook run path accepts).
                       */}
                      <div className="flex items-start gap-2 text-xs text-muted-foreground">
                        <ShieldCheck className="mt-0.5 h-3.5 w-3.5 shrink-0 text-primary" />
                        <span>{t("playbooks.launchDialog.credentialHelp")}</span>
                      </div>
                      <ToggleField label={t("playbooks.launchDialog.dryRun")} checked={dryRun}
                        disabled={isSubmitting} onCheckedChange={setDryRun} />
                      <div className="space-y-1.5">
                        <Label className="text-sm font-medium">{t("playbooks.launchDialog.rawExtravars")}</Label>
                        <p className="text-xs text-muted-foreground">
                          {t("playbooks.launchDialog.rawExtravarsHelp")}
                        </p>
                        <Textarea
                          className="min-h-24 font-mono text-xs"
                          value={rawExtravars}
                          aria-label={t("playbooks.launchDialog.rawExtravars")}
                          onChange={(event) => {
                            setRawExtravars(event.target.value);
                            setRawError(null);
                          }}
                        />
                        {rawError ? (
                          <p className="text-xs text-status-warning">{rawError}</p>
                        ) : null}
                      </div>
                    </div>
                  ) : null}
                </div>
              </div>
            </div>
          </StepShell>

          <SecretsPreflightPanel entries={preflightEntries} />

        </fieldset>

        <DialogFooter className="border-t border-border bg-card px-4 py-3">
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={isSubmitting}
          >
            {t("cancel")}
          </Button>
          <Button
            className="gap-1.5"
            disabled={!canConfirm || !resolvedIntent}
            onClick={confirm}
          >
            {isSubmitting ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <PlayCircle className="h-3.5 w-3.5" />
            )}
            {t("playbooks.launchDialog.queueRun", { count: targetCount })}
          </Button>
        </DialogFooter>
    </DialogContent>
  );
}

function StepShell({
  step,
  title,
  children,
}: {
  step: number;
  title: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-md border border-border bg-card p-3">
      <div className="flex items-center gap-2">
        <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-muted text-xs font-semibold text-foreground">
          {step}
        </span>
        <h3 className="text-sm font-semibold text-foreground">{title}</h3>
      </div>
      <div className="mt-3">{children}</div>
    </section>
  );
}

function TargetModeButton({
  active,
  icon: Icon,
  label,
  hint,
  onClick,
}: {
  active: boolean;
  icon: typeof Layers3;
  label: string;
  hint: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={cn(
        "flex h-9 items-center gap-2 rounded-md border px-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        active
          ? "border-primary bg-accent text-foreground"
          : "border-border bg-card text-muted-foreground hover:bg-accent",
      )}
      aria-pressed={active}
      title={hint}
      onClick={onClick}
    >
      <Icon className={cn("h-4 w-4 shrink-0", active ? "text-primary" : "text-muted-foreground")} />
      <span className="min-w-0">
        <span className="block truncate text-sm font-medium">{label}</span>
        <span className="sr-only">
          {hint}
        </span>
      </span>
    </button>
  );
}

/** One typed input for a declared launch variable, driven by its `VarSpec`. */
function VarField({
  spec,
  value,
  error,
  onChange,
}: {
  spec: VarSpec;
  value: string | number | boolean | undefined;
  error?: string;
  onChange: (value: string | number | boolean) => void;
}) {
  const { t } = useTranslation();
  const label = spec.label?.trim() || spec.name;
  const errorText = error
    ? error === "required"
      ? t("playbooks.launchDialog.varRequired")
      : t("playbooks.launchDialog.varNumberInvalid")
    : null;

  let control: React.ReactNode;
  if (spec.type === "boolean") {
    control = (
      <div className="flex h-9 items-center">
        <Switch
          checked={value === true}
          onCheckedChange={(checked) => onChange(checked)}
          aria-label={label}
        />
      </div>
    );
  } else if (spec.type === "choice") {
    control = (
      <SearchableSelect
        value={typeof value === "string" ? value : ""}
        onValueChange={(next) => onChange(next)}
        placeholder={t("playbooks.launchDialog.varChoosePlaceholder")}
        searchPlaceholder={t("search")}
        items={(spec.choices ?? []).map((choice) => ({
          id: choice,
          label: choice,
        }))}
        aria-label={label}
      />
    );
  } else {
    control = (
      <Input
        className={cn("h-9 text-sm", spec.type === "number" && "font-mono")}
        inputMode={spec.type === "number" ? "numeric" : undefined}
        value={value === undefined || value === null ? "" : String(value)}
        aria-label={label}
        onChange={(event) => onChange(event.target.value)}
      />
    );
  }

  return (
    <FormField
      label={
        <span className="flex items-center gap-1">
          {label}
          {spec.required ? (
            <span aria-hidden="true" className="text-destructive">
              *
            </span>
          ) : null}
        </span>
      }
    >
      {control}
      {spec.description ? (
        <p className="text-xs leading-snug text-muted-foreground">
          {spec.description}
        </p>
      ) : null}
      {errorText ? <p className="text-xs text-status-warning">{errorText}</p> : null}
    </FormField>
  );
}

function TargetRecoveryState({
  label,
  description,
  actionLabel,
  onAction,
}: {
  label: string;
  description: string;
  actionLabel?: string;
  onAction?: () => void;
}) {
  return (
    <PageState kind="empty" title={label} description={description} action={actionLabel && onAction ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-9 text-sm"
          onClick={onAction}
        >
          {actionLabel}
        </Button>
      ) : undefined} />
  );
}
