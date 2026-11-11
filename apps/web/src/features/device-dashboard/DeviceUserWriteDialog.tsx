import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useIsMutating, useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { AlertTriangle, CheckCircle2, Loader2, ShieldOff, XCircle } from "lucide-react";

import { ApiError } from "@/shared/api/client";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import { Button } from "@/shared/ui-primitives/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import StatusChip from "@/shared/ui-shared/StatusChip";
import {
  createDeviceUsers,
  deleteDeviceUsers,
  previewDeviceUsers,
  type DeviceUserPreviewAction,
  type DeviceUserPreviewItem,
  type DeviceUsersOutcome,
  type DeviceUsersPreview,
} from "@/features/device-dashboard/api";
import {
  isPermissionDeniedError,
  toPreviewBody,
  type PendingDeviceUserWrite,
} from "@/features/device-dashboard/deviceUserWrites";

/** Non-scary notice shown when the caller lacks the cctv write permission. */
export const WritesDisabledBanner = ({ className }: { className?: string }) => {
  const { t } = useTranslation();
  return (
    <div
      className={cn(
        "flex items-start gap-2 rounded-md border border-status-warning/25 bg-card px-3 py-2 text-xs text-status-warning",
        className,
      )}
    >
      <ShieldOff className="mt-0.5 h-3.5 w-3.5 shrink-0" />
      <div className="space-y-0.5 text-left">
        <p className="font-medium">
          {t("deviceDashboard.writesDisabledTitle", "No permission to change this device")}
        </p>
        <p className="text-status-warning/80">
          {t(
            "deviceDashboard.writesDisabledBody",
            "Your account does not have the CCTV write permission for this device, so no changes will reach it. You can still preview what a change would do.",
          )}
        </p>
      </div>
    </div>
  );
};

const ACTION_STATUS: Record<
  DeviceUserPreviewAction,
  "positive" | "warning" | "negative" | "neutral"
> = {
  create: "positive",
  skip: "neutral",
  delete: "warning",
  missing: "negative",
};

function useActionLabels(): Record<DeviceUserPreviewAction, string> {
  const { t } = useTranslation();
  return {
    create: t("deviceDashboard.previewCreate", "Create"),
    skip: t("deviceDashboard.previewSkip", "Skip"),
    delete: t("deviceDashboard.previewDelete", "Delete"),
    missing: t("deviceDashboard.previewMissing", "Missing"),
  };
}

interface DeviceUserWriteDialogProps {
  pending: PendingDeviceUserWrite | null;
  targetId: string;
  onClose: () => void;
  onReturnFocus?: () => boolean;
  /** Update the initiating form only while its action is still current. */
  onApplied: () => void;
  /** Lift the no-permission signal so the section can disable its controls. */
  onWritesDisabled: () => void;
}

/**
 * Two-step device user write: a dry-run PREVIEW (per-name verdicts) then an
 * explicit Confirm that applies via `:create` / `:delete`. The bulk outcome
 * (succeeded / failed per account) is rendered so partial success is visible.
 * When the caller lacks the cctv write permission (`writes_enabled = false` on
 * the preview, or a 403 on apply) the banner shows and Confirm is disabled.
 */
const DeviceUserWriteDialog = ({
  pending,
  targetId,
  onClose,
  onReturnFocus,
  onApplied,
  onWritesDisabled,
}: DeviceUserWriteDialogProps) => {
  const { t } = useTranslation();
  const actionLabels = useActionLabels();
  const queryClient = useQueryClient();
  const open = pending !== null;
  const [identity, setIdentity] = useState({ pending, targetId, generation: 0 });
  if (identity.pending !== pending || identity.targetId !== targetId) {
    setIdentity({ pending, targetId, generation: identity.generation + 1 });
  }
  const action = useRef(identity);
  useLayoutEffect(() => {
    action.current = identity;
    return () => { action.current = { ...identity, pending: null }; };
  }, [identity]);
  const applyingTargets = useRef(new Set<string>());
  const pendingWrites = useIsMutating({ mutationKey: ["device-user-write", targetId] });
  type WriteRequest = { write: PendingDeviceUserWrite; targetId: string; generation: number };
  const isCurrent = (request: WriteRequest) =>
    action.current.pending === request.write && action.current.targetId === request.targetId &&
    action.current.generation === request.generation;

  const previewMutation = useMutation({
    mutationFn: ({ write, targetId }: WriteRequest) =>
      previewDeviceUsers(targetId, toPreviewBody(write)),
  });
  const applyMutation = useMutation({
    mutationKey: ["device-user-write", targetId],
    mutationFn: ({ write, targetId }: WriteRequest) =>
      write.kind === "create"
        ? createDeviceUsers(targetId, write.users)
        : deleteDeviceUsers(targetId, write.usernames),
    onSuccess: (_, request) => {
      // The device changed even if its confirmation surface was closed meanwhile.
      void queryClient.invalidateQueries({ queryKey: ["device", request.targetId, "users"] })
        .catch(() => undefined); // Refetch failures belong to the users query's error state.
      if (isCurrent(request)) onApplied();
    },
    onError: (error: unknown, request) => {
      if (!isCurrent(request)) return;
      if (isPermissionDeniedError(error)) {
        onWritesDisabled();
        return;
      }
      notify("error", error instanceof ApiError ? error.message : t("unknownError", "Unknown error"));
    },
    onSettled: (_, __, request) => { applyingTargets.current.delete(request.targetId); },
  });

  // Each new pending (or a close) discards prior state and re-previews. `pending`
  // is a fresh object per action (unique `nonce`), so this fires on every action.
  const { mutate: runPreview, reset: resetPreview } = previewMutation;
  const { reset: resetApply } = applyMutation;
  useEffect(() => {
    resetApply();
    if (pending) runPreview({ write: pending, targetId, generation: identity.generation });
    else resetPreview();
  }, [identity, pending, targetId, runPreview, resetPreview, resetApply]);

  const belongsToAction = (request: WriteRequest | undefined) => request?.write === pending &&
    request?.targetId === targetId && request?.generation === identity.generation;
  const preview = belongsToAction(previewMutation.variables) ? previewMutation.data : undefined;
  const outcome = belongsToAction(applyMutation.variables) ? applyMutation.data : undefined;
  const previewDisabled = preview?.writes_enabled === false;
  const applyBlocked = belongsToAction(applyMutation.variables) && applyMutation.isError && isPermissionDeniedError(applyMutation.error);
  const writesDisabled = previewDisabled || applyBlocked;

  useEffect(() => {
    if (writesDisabled) onWritesDisabled();
  }, [writesDisabled, onWritesDisabled]);

  const confirmDisabled = !preview || previewDisabled || applyBlocked || pendingWrites > 0;

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="sm:max-w-lg" onCloseAutoFocus={onReturnFocus ? (event) => { if (onReturnFocus()) event.preventDefault(); } : undefined}>
        <DialogHeader>
          <DialogTitle>
            {pending?.title ?? t("deviceDashboard.reviewChanges", "Review changes")}
          </DialogTitle>
          <DialogDescription>
            {outcome
              ? t("deviceDashboard.writeResultDescription", "Result of the change on the device.")
              : t(
                  "deviceDashboard.previewDescription",
                  "Review what this change would do before it is applied to the device.",
                )}
          </DialogDescription>
        </DialogHeader>

        <div className="min-h-[4rem] space-y-3">
          <DialogBody
            outcome={outcome}
            preview={preview}
            applyIsError={belongsToAction(applyMutation.variables) && applyMutation.isError}
            applyError={applyMutation.error}
            previewIsError={belongsToAction(previewMutation.variables) && previewMutation.isError}
            previewError={previewMutation.error}
            applyBlocked={applyBlocked}
            writesDisabled={writesDisabled}
            actionLabels={actionLabels}
          />
        </div>

        <DialogFooter>
          {outcome ? (
            <Button size="sm" onClick={onClose}>
              {t("close", "Close")}
            </Button>
          ) : (
            <>
              <Button variant="outline" size="sm" onClick={onClose}>
                {t("cancel", "Cancel")}
              </Button>
              <Button
                size="sm"
                disabled={confirmDisabled}
                onClick={() => {
                  if (!pending || confirmDisabled || applyingTargets.current.has(targetId)) return;
                  applyingTargets.current.add(targetId);
                  applyMutation.mutate({ write: pending, targetId, generation: action.current.generation });
                }}
              >
                {applyMutation.isPending ? (
                  <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
                ) : null}
                {t("deviceDashboard.confirmWrite", "Confirm")}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

function apiErrorMessage(error: unknown, t: (key: string, fallback: string) => string): string {
  return error instanceof ApiError ? error.message : t("unknownError", "Unknown error");
}

const ErrorNotice = ({ message }: { message: string }) => (
  <div className="flex items-start gap-2 rounded-md border border-status-negative/25 bg-card px-3 py-2 text-xs text-status-negative">
    <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
    <span>{message}</span>
  </div>
);

const PreviewTable = ({
  items,
  actionLabels,
  emptyLabel,
}: {
  items: DeviceUserPreviewItem[];
  actionLabels: Record<DeviceUserPreviewAction, string>;
  emptyLabel: string;
}) => {
  const { t } = useTranslation();
  if (items.length === 0) {
    return <p className="py-4 text-center text-xs text-muted-foreground">{emptyLabel}</p>;
  }
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead>
          <tr className="border-b border-border text-xs uppercase tracking-wide text-muted-foreground">
            <th className="py-1.5 pr-3 font-medium">{t("deviceDashboard.userName", "Name")}</th>
            <th className="py-1.5 pr-3 font-medium">{t("deviceDashboard.previewAction", "Action")}</th>
            <th className="py-1.5 font-medium">{t("deviceDashboard.previewReason", "Reason")}</th>
          </tr>
        </thead>
        <tbody>
          {items.map((item, index) => (
            <tr key={`${item.name}-${index}`} className="border-b border-border last:border-0">
              <td className="py-2 pr-3 font-medium text-foreground">{item.name}</td>
              <td className="py-2 pr-3">
                <StatusChip status={ACTION_STATUS[item.action]} label={actionLabels[item.action]} />
              </td>
              <td className="py-2 text-muted-foreground">{item.reason}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
};

const OutcomeView = ({ outcome }: { outcome: DeviceUsersOutcome }) => {
  const { t } = useTranslation();
  return (
    <div className="space-y-2">
      {outcome.succeeded.map((name) => (
        <div
          key={`ok-${name}`}
          className="flex items-center gap-2 rounded-md border border-status-positive/25 bg-card px-3 py-1.5 text-xs text-status-positive"
        >
          <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
          <span className="font-medium text-foreground">{name}</span>
          <span className="text-status-positive/80">
            {t("deviceDashboard.outcomeSucceeded", "Succeeded")}
          </span>
        </div>
      ))}
      {outcome.failed.map((failure) => (
        <div
          key={`fail-${failure.name}`}
          className="flex items-start gap-2 rounded-md border border-status-negative/25 bg-card px-3 py-1.5 text-xs text-status-negative"
        >
          <XCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span className="font-medium text-foreground">{failure.name}</span>
          <span className="text-status-negative/80">{failure.message}</span>
        </div>
      ))}
      {outcome.succeeded.length === 0 && outcome.failed.length === 0 && (
        <p className="py-4 text-center text-xs text-muted-foreground">
          {t("deviceDashboard.outcomeEmpty", "No accounts were changed.")}
        </p>
      )}
    </div>
  );
};

/**
 * Picks the single body view for the current step. Early returns keep the
 * outcome → apply-error → preview-error → preview → loading precedence flat and
 * side-effect-free (behaviour identical to the prior nested-ternary chain).
 */
const DialogBody = ({
  outcome,
  preview,
  applyIsError,
  applyError,
  previewIsError,
  previewError,
  applyBlocked,
  writesDisabled,
  actionLabels,
}: {
  outcome: DeviceUsersOutcome | undefined;
  preview: DeviceUsersPreview | undefined;
  applyIsError: boolean;
  applyError: unknown;
  previewIsError: boolean;
  previewError: unknown;
  applyBlocked: boolean;
  writesDisabled: boolean;
  actionLabels: Record<DeviceUserPreviewAction, string>;
}) => {
  const { t } = useTranslation();

  if (outcome) return <OutcomeView outcome={outcome} />;
  if (applyIsError && !applyBlocked) {
    return <ErrorNotice message={apiErrorMessage(applyError, t)} />;
  }
  if (previewIsError) {
    return <ErrorNotice message={apiErrorMessage(previewError, t)} />;
  }
  if (preview) {
    return (
      <>
        {writesDisabled && <WritesDisabledBanner />}
        <PreviewTable
          items={preview.per_item}
          actionLabels={actionLabels}
          emptyLabel={t("deviceDashboard.previewEmpty", "Nothing to change.")}
        />
      </>
    );
  }
  return (
    <div className="flex items-center gap-2 py-4 text-xs text-muted-foreground">
      <Loader2 className="h-4 w-4 animate-spin" />
      {t("deviceDashboard.previewLoading", "Checking against the device…")}
    </div>
  );
};

export default DeviceUserWriteDialog;
