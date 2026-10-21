import { useTranslation } from "react-i18next";
import { AlertTriangle, Check, KeyRound, X } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import {
  isPreflightRowBlocking,
  type SecretPreflightEntry,
} from "@/features/automation-secrets/api";

/**
 * Advisory secret-grant preflight shown above the launch button. Lists each
 * secret a playbook version declares with a per-caller C3 verdict (✓/✗) and a
 * value-present signal. Purely presentational — the caller decides whether to
 * block the launch (the server gates independently regardless). Renders nothing
 * when the playbook declares no secrets.
 */
function SecretsPreflightPanel({
  entries,
}: Readonly<{
  entries: SecretPreflightEntry[];
}>) {
  const { t } = useTranslation();
  if (entries.length === 0) return null;

  const blocked = entries.some(isPreflightRowBlocking);

  return (
    <div
      className={cn(
        "rounded-md border px-3 py-2 text-sm",
        blocked
          ? "border-status-warning/35 bg-status-warning/10"
          : "border-status-positive/30 bg-status-positive/10",
      )}
    >
      <div className="flex items-center gap-1.5 text-xs font-medium text-foreground">
        <KeyRound className="h-3.5 w-3.5 shrink-0" />
        {t("playbooks.launchDialog.secrets.title")}
      </div>
      <ul className="mt-1.5 space-y-1">
        {entries.map((entry) => {
          const noAccess = !entry.allowed;
          const noValue = !entry.has_value;
          const ok = !noAccess && !noValue;
          const statusKey = noAccess
            ? "playbooks.launchDialog.secrets.noAccess"
            : "playbooks.launchDialog.secrets.noValue";
          return (
            <li key={entry.env_name} className="flex items-center gap-2">
              {ok ? (
                <Check className="h-3 w-3 shrink-0 text-status-positive" />
              ) : (
                <X className="h-3 w-3 shrink-0 text-status-warning" />
              )}
              <span className="font-mono text-foreground">{entry.env_name}</span>
              <span className="truncate text-muted-foreground">{entry.key}</span>
              {noAccess || noValue ? (
                <span className="text-status-warning">{t(statusKey)}</span>
              ) : null}
            </li>
          );
        })}
      </ul>
      {blocked ? (
        <p className="mt-1.5 flex items-start gap-1.5 text-status-warning">
          <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
          {t("playbooks.launchDialog.secrets.blockedHint")}
        </p>
      ) : null}
    </div>
  );
}

export default SecretsPreflightPanel;
