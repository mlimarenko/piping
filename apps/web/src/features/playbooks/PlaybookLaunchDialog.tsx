import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";
import { Button } from "@/shared/ui-primitives/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import FormSection from "@/shared/ui-shared/FormSection";
import PageState from "@/shared/ui-shared/PageState";
import type { components } from "@/shared/api/schema.gen";
import type { AutomationSecret } from "@/features/automation-secrets/api";
import type { PlaybookLaunchTargetProjection } from "./hostSelection";
import type { JobTemplateVarsSchema, PlaybookTargetSetPreview } from "./api";
import { FormField } from "./PlaybookFormFields";
import {
  evaluateLaunchReadiness,
  LAUNCH_TARGET_LIMIT,
  type LaunchBlocker,
  type LaunchBlockerCode,
  type LaunchResource,
} from "./launchReadiness";
import { playbookTargetSetPreviewHref } from "./playbookNavigation";
import { AutomationHelpLink } from "@/shared/ui-shared/AutomationHelpLink";
import { GitSourceSummary } from "./GitSourceSummary";
import ParametersEditor from "./ParametersEditor";
import { effectiveParameters, jobVarsSchema, missingRequiredParameters, parameterErrors, parseParameterValues, type Extravars } from "./varsSchema";

// The dialog selects a persisted job; fixed targets remain caller-owned.
// Project and revision are descriptive; the server resolves the configuration.
export interface PlaybookLaunchIntent {
  jobTemplateId: string;
  extraVars?: Extravars;
}

interface PlaybookLaunchDialogProps {
  open: boolean;
  intent: PlaybookLaunchIntent | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: (intent: PlaybookLaunchIntent) => void;
  jobTemplates: components["schemas"]["JobTemplate"][];
  projects: components["schemas"]["Project"][];
  onSelectedJobTemplateIdChange: (id: string) => void;
  loading?: boolean;
  isSubmitting?: boolean;
  canLaunch: boolean;
  quick?: boolean;
  varsSchema?: LaunchResource<JobTemplateVarsSchema>;
  fixedTargets?: readonly PlaybookLaunchTargetProjection[];
  /** Preview of the saved managed target set, owned by the caller's query. */
  preview?: LaunchResource<PlaybookTargetSetPreview>;
  /** Named automation secrets, owned by the caller's query. */
  secrets?: LaunchResource<AutomationSecret[]>;
  onRetryComposition?: () => void;
  referenceError?: string;
  launchError?: string;
  onRetry?: () => void;
}

/** Blockers explained in one place; loading and load failures own their section. */
const ALERT_BLOCKERS = new Set<LaunchBlockerCode>([
  "no-permission",
  "target-set-over-limit",
  "unbound-address",
  "empty-selection",
  "missing-secrets",
  "secrets-loading",
  "secrets-failed",
]);

const BLOCKER_MESSAGES: Record<LaunchBlockerCode, string> = {
  "no-permission": "playbooks.launchDraft.blockers.noPermission",
  "target-set-loading": "playbooks.launchDraft.blockers.targetSetLoading",
  "target-set-failed": "playbooks.launchDraft.blockers.targetSetFailed",
  "empty-target-set": "playbooks.launchDraft.blockers.emptyTargetSet",
  "empty-selection": "playbooks.launchDraft.blockers.emptySelection",
  "fixed-targets-unsupported": "playbooks.launchDraft.fixedTargetsConstraint",
  "target-set-over-limit": "playbooks.launchDraft.blockers.targetSetOverLimit",
  "unbound-address": "playbooks.launchDraft.blockers.unboundAddress",
  "secrets-loading": "playbooks.launchDraft.blockers.secretsLoading",
  "secrets-failed": "playbooks.launchDraft.blockers.secretsFailed",
  "missing-secrets": "playbooks.launchDraft.blockers.missingSecrets",
};

export default function PlaybookLaunchDialog(props: Readonly<PlaybookLaunchDialogProps>) {
  return <LaunchDialog {...props} />;
}

function LaunchDialog({ open, intent, onOpenChange, onConfirm, jobTemplates, projects, onSelectedJobTemplateIdChange, loading = false, isSubmitting = false, canLaunch, quick = false, varsSchema, fixedTargets, preview, secrets, onRetryComposition, referenceError, launchError, onRetry }: Readonly<PlaybookLaunchDialogProps>) {
  const { t } = useTranslation();
  const [extraVars, setExtraVars] = useState("{}");
  const [parametersValid, setParametersValid] = useState(true);
  const focusReturn = useRef<HTMLElement | null>(null);
  useEffect(() => { setExtraVars("{}"); setParametersValid(true); }, [open, intent?.jobTemplateId]);
  const job = jobTemplates.find((item) => item.id === intent?.jobTemplateId);
  const project = projects.find((item) => item.id === job?.project_id);
  const schema = jobVarsSchema(job?.vars_schema, varsSchema?.status === "ready" ? varsSchema.data : undefined);
  const schemaReady = job?.vars_schema?.kind !== "project_file" || varsSchema?.status === "ready";
  let jobValues: Extravars = {};
  let savedParametersValid = true;
  try { jobValues = parseParameterValues(JSON.stringify(job?.options.extra_vars ?? {})); } catch { savedParametersValid = false; }
  const overrides = parseParameterValues(extraVars);
  const effective = effectiveParameters(schema, jobValues, overrides);
  const missingFields = missingRequiredParameters(schema, effectiveParameters(schema, jobValues, {})).map((spec) => spec.name);
  const readiness = evaluateLaunchReadiness({
    jobReady: Boolean(job && job.playbook_paths.length > 0 && project && (job.revision.kind === "fixed" ? job.revision.revision_id : project.current_revision_id)),
    canLaunch,
    targetSetId: job?.inventory.kind === "managed" ? job.inventory.target_set_id : null,
    adHocTargets: fixedTargets !== undefined,
    fixedTargetCount: fixedTargets?.length ?? 0,
    preview,
    secretRefs: job?.options.secret_refs ?? {},
    secrets,
  });
  const launchable = readiness.launchable && !loading && !isSubmitting && referenceError === undefined && schemaReady && savedParametersValid && parametersValid && Object.keys(parameterErrors(schema, effective)).length === 0;
  const alerts: LaunchBlocker[] = readiness.blockers.filter((blocker) => ALERT_BLOCKERS.has(blocker.code));
  const composition = readiness.composition;
  const previewFailed = job?.inventory.kind === "managed" && fixedTargets === undefined && preview?.status === "failed";
  return <Dialog open={open} onOpenChange={isSubmitting ? undefined : onOpenChange}>
    <DialogContent className="max-h-[90dvh] overflow-y-auto" onOpenAutoFocus={() => { focusReturn.current = document.activeElement instanceof HTMLElement ? document.activeElement : null; }} onCloseAutoFocus={(event) => { if (focusReturn.current?.isConnected) { event.preventDefault(); focusReturn.current.focus(); } }}>
      <DialogHeader><DialogTitle>{t("playbooks.launchDraft.title", { defaultValue: "Launch job template" })}</DialogTitle><DialogDescription>{t("playbooks.launchDraft.saved", { defaultValue: "The saved job configuration is used for this run. No unsaved editor changes are included." })}</DialogDescription></DialogHeader>
      {referenceError !== undefined && <div role="alert" className="flex flex-wrap items-center gap-2 text-sm text-status-warning"><span>{referenceError}</span>{onRetry && <Button type="button" variant="outline" size="sm" disabled={loading || isSubmitting} onClick={onRetry}>{t("retry")}</Button>}</div>}
      {launchError && <p role="alert" className="text-sm text-destructive">{launchError}</p>}
      <FormField label={t("playbooks.scheduleDraft.job", { defaultValue: "Job template" })}><SearchableSelect value={intent?.jobTemplateId ?? ""} onValueChange={onSelectedJobTemplateIdChange} disabled={loading || isSubmitting} items={jobTemplates.filter((item) => Boolean(item.project_id)).map((item) => ({ id: item.id, label: item.name }))} placeholder={t("playbooks.scheduleDraft.chooseJob", { defaultValue: "Select a job template" })} searchPlaceholder={t("search")} /></FormField>
      {job && <dl className="grid min-w-0 gap-2 text-sm sm:grid-cols-2"><div className="min-w-0"><dt className="text-muted-foreground">{t("playbooks.launchDraft.jobId", { defaultValue: "Job ID" })}</dt><dd className="break-all">{job.id}</dd></div><div className="min-w-0"><dt className="text-muted-foreground">{t("playbooks.launchDraft.project", { defaultValue: "Project" })}</dt><dd className="break-all">{project?.name ?? job.project_id}</dd></div>{project?.source.kind !== "git" && <div className="min-w-0"><dt className="text-muted-foreground">{t("playbooks.launchDraft.revision", { defaultValue: "Project revision" })}</dt><dd className="break-all">{(job.revision.kind === "fixed" ? job.revision.revision_id : project?.current_revision_id) ?? t("playbooks.launchDraft.revisionPending", { defaultValue: "Not available" })}</dd></div>}</dl>}
      {job && project?.source.kind === "git" && <GitSourceSummary project={project} selection={job.revision} disabled={isSubmitting} />}
      {fixedTargets !== undefined && <FormSection title={t("playbooks.launchDraft.fixedTargets", { defaultValue: "Selected hosts" })} description={t("playbooks.launchDraft.fixedTargetsConstraint", { defaultValue: "Selected hosts must belong to the job's saved managed inventory. Native inventory does not support host selection." })}>
        <p className="text-sm text-muted-foreground">{t("playbooks.launchDraft.fixedTargetsCount", { count: fixedTargets.length, defaultValue: "Selected hosts: {{count}}" })}</p>
        <ul className="max-h-40 overflow-y-auto text-sm">{fixedTargets.map((target) => <li key={target.id}>{target.name}{target.subtitle && <span className="text-muted-foreground"> — {target.subtitle}</span>}</li>)}</ul>
      </FormSection>}
      {job && schemaReady && <ParametersEditor key={`${job.id}:${open}`} schema={schema} value={extraVars} onChange={setExtraVars} onValidityChange={setParametersValid} inherited={jobValues} fields={quick ? missingFields : undefined} disabled={isSubmitting} />}
      {!schemaReady && <p role="status" className="text-sm text-status-warning">{t(varsSchema?.status === "failed" ? "playbooks.parameters.schemaFailed" : "playbooks.parameters.schemaLoading", { defaultValue: varsSchema?.status === "failed" ? "The parameter schema could not be loaded. Reopen the launch to retry." : "Loading parameter declarations…" })}</p>}
      {!savedParametersValid && <p role="alert" className="text-sm text-destructive">{t("playbooks.parameters.savedInvalid", { defaultValue: "The saved parameters are invalid. Edit the job before launching." })}</p>}
      {job && fixedTargets === undefined && <FormSection title={t("playbooks.launchDraft.targets", { defaultValue: "Run targets" })} description={job.inventory.kind === "managed" ? t("playbooks.launchDraft.targetsHint", { defaultValue: "Resolved from the saved target set now; the server resolves it again at launch." }) : undefined}
        action={composition ? <Link className="text-sm font-medium text-primary underline-offset-4 hover:underline" to={playbookTargetSetPreviewHref(composition.targetSetId)}>{t("playbooks.launchDraft.targetPreview", { defaultValue: "Open full preview" })}</Link> : undefined}>
        {job.inventory.kind === "native" ? <p className="text-sm text-muted-foreground">{t("playbooks.launchDraft.nativeTargets", { defaultValue: "This job runs against its native inventory; Piping does not resolve a host list." })}</p>
          : previewFailed ? <PageState kind="error" title={t(BLOCKER_MESSAGES["target-set-failed"])} action={onRetryComposition && <Button type="button" variant="outline" size="sm" disabled={isSubmitting} onClick={onRetryComposition}>{t("retry")}</Button>} />
            : !composition ? <PageState kind="loading" title={t(BLOCKER_MESSAGES["target-set-loading"])} />
              : composition.total === 0 ? <PageState kind="empty" title={t(BLOCKER_MESSAGES["empty-target-set"])} description={t("playbooks.launchDraft.emptyTargetSetHint", { defaultValue: "A saved set that resolves to no hosts would queue a run that touches nothing." })} />
                : <>
                  <dl className="grid gap-2 text-sm sm:grid-cols-3">
                    <div><dt className="text-muted-foreground">{t("playbooks.launchDraft.targetSet", { defaultValue: "Target set" })}</dt><dd className="break-words">{composition.name}</dd></div>
                    <div><dt className="text-muted-foreground">{t("playbooks.launchDraft.targetMode", { defaultValue: "Mode" })}</dt><dd>{composition.mode === "static" ? t("playbooks.launchDraft.targetModeStatic", { defaultValue: "Fixed set" }) : t("playbooks.launchDraft.targetModeDynamic", { defaultValue: "Dynamic selector" })}</dd></div>
                    <div><dt className="text-muted-foreground">{t("playbooks.launchDraft.targetCount", { defaultValue: "Hosts" })}</dt><dd className="tabular-nums">{composition.total}</dd></div>
                  </dl>
                  <p className="text-xs text-muted-foreground">{t("playbooks.launchDraft.targetLimitHint", { limit: LAUNCH_TARGET_LIMIT, defaultValue: "One launch is limited to {{limit}} hosts." })}</p>
                  <ol className="max-h-40 space-y-1 overflow-y-auto text-sm">{composition.sample.map((item) => <li key={`${item.target_kind}:${item.target_id}`} className="flex flex-wrap items-baseline gap-2"><span className="font-medium">{item.name}</span><span className="text-muted-foreground">{item.address?.trim() || t("playbooks.launchDraft.noAddress", { defaultValue: "No bound address" })}</span></li>)}</ol>
                  {composition.total > composition.sample.length && <p className="text-xs text-muted-foreground">{t("playbooks.launchDraft.targetSampleTruncated", { shown: composition.sample.length, total: composition.total, defaultValue: "Showing the first {{shown}} of {{total}} hosts." })}</p>}
                </>}
      </FormSection>}
      {alerts.length > 0 && <ul role="alert" className="space-y-1 text-sm text-status-warning">{alerts.map((blocker) => <li key={blocker.code}>{t(BLOCKER_MESSAGES[blocker.code], { count: blocker.count ?? 0, limit: LAUNCH_TARGET_LIMIT })}</li>)}</ul>}
      <div className="flex justify-start"><AutomationHelpLink article="jobs-and-runs" hash="launch-checks" /></div>
      <DialogFooter><Button variant="outline" onClick={() => onOpenChange(false)} disabled={isSubmitting}>{t("cancel")}</Button><Button disabled={!launchable} onClick={() => { if (launchable && intent) onConfirm({ jobTemplateId: intent.jobTemplateId, ...(Object.keys(overrides).length ? { extraVars: overrides } : {}) }); }}>{t("playbooks.launchDraft.launch", { defaultValue: "Launch" })}</Button></DialogFooter>
    </DialogContent>
  </Dialog>;
}
