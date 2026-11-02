import { useState } from "react";
import { useTranslation } from "react-i18next";
import { KeyRound, Loader2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import { Button } from "@/shared/ui-primitives/button";
import { Badge } from "@/shared/ui-primitives/badge";
import SecretField from "@/shared/ui-shared/SecretField";
import type { AutomationSecret } from "@/features/automation-secrets/api";

/**
 * Write-only value editor for a named automation secret. The stored value is
 * NEVER read back (the API returns only `has_value`), so this dialog only ever
 * SETs a new value or CLEARs the existing one — it never displays the current
 * value. The input is masked with `SecretField` (a purely visual mask).
 */
function AutomationSecretValueDialog({
  secret,
  open,
  onOpenChange,
  onSetValue,
  onClearValue,
}: Readonly<{
  secret: AutomationSecret | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSetValue: (value: string) => Promise<void>;
  onClearValue: () => Promise<void>;
}>) {
  const { t } = useTranslation();
  const [value, setValue] = useState<string | undefined>(undefined);
  const [busy, setBusy] = useState<"set" | "clear" | null>(null);
  const [failed, setFailed] = useState(false);
  const [resetKey, setResetKey] = useState<string | null>(null);

  // Reset the (never-prefilled) input during render each time the dialog opens
  // for a (possibly different) secret. Adjusting state during render — rather
  // than syncing in an effect — avoids a second paint with stale input.
  const currentResetKey = open ? (secret?.id ?? "") : null;
  if (currentResetKey !== resetKey) {
    setResetKey(currentResetKey);
    setValue(undefined);
    setFailed(false);
  }

  if (!secret) return null;

  const trimmed = (value ?? "").trim();
  const canSave = trimmed.length > 0 && busy === null;

  const runSet = async () => {
    if (!canSave) return;
    setBusy("set");
    setFailed(false);
    try {
      await onSetValue(trimmed);
      onOpenChange(false);
    } catch {
      // Never reflect a write-only payload or server exception into the UI.
      setFailed(true);
    } finally {
      setBusy(null);
    }
  };

  const runClear = async () => {
    if (busy !== null || !secret.has_value) return;
    setBusy("clear");
    setFailed(false);
    try {
      await onClearValue();
      onOpenChange(false);
    } catch {
      // Failed requests retain the draft for an explicit retry.
      setFailed(true);
    } finally {
      setBusy(null);
    }
  };

  return (
    <Dialog open={open} onOpenChange={busy === null ? onOpenChange : undefined}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="inline-flex items-center gap-2">
            <KeyRound className="h-4 w-4" />
            {t("automationSecrets.value.title", { key: secret.key })}
          </DialogTitle>
          <DialogDescription>
            {t("automationSecrets.value.description")}
          </DialogDescription>
        </DialogHeader>

        <fieldset disabled={busy !== null} inert={busy !== null} className="flex min-w-0 flex-col gap-3">
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            {secret.has_value ? (
              <Badge variant="status-active" className="rounded text-xs">
                {t("automationSecrets.value.hasValue")}
              </Badge>
            ) : (
              <Badge variant="status-staging" className="rounded text-xs">
                {t("automationSecrets.value.noValue")}
              </Badge>
            )}
            <span>{t("automationSecrets.value.hint")}</span>
          </div>

          <SecretField
            label={t("automationSecrets.value.newValue")}
            value={value}
            onChange={(next) => { if (busy === null) setValue(next); }}
            placeholder={t("automationSecrets.value.placeholder")}
            canReveal
          />
        </fieldset>
        {failed && <p role="alert" className="text-sm text-destructive">{t("unknownError")}</p>}

        <DialogFooter className="gap-2 sm:justify-between">
          {secret.has_value ? (
            <Button
              type="button"
              variant="outline"
              className="text-status-negative hover:text-status-negative"
              disabled={busy !== null}
              onClick={() => void runClear()}
            >
              {busy === "clear" ? (
                <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
              ) : null}
              {t("automationSecrets.value.clear")}
            </Button>
          ) : (
            <span />
          )}
          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={busy !== null}
              onClick={() => onOpenChange(false)}
            >
              {t("cancel")}
            </Button>
            <Button type="button" disabled={!canSave} onClick={() => void runSet()}>
              {busy === "set" ? (
                <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
              ) : null}
              {t("automationSecrets.value.save")}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export default AutomationSecretValueDialog;
