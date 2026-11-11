import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2 } from "lucide-react";

import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Badge } from "@/shared/ui-primitives/badge";
import { FormField } from "./PlaybookFormFields";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import type { AutomationSecret } from "@/features/automation-secrets/api";
import {
  MAX_SECRET_ENV,
  validateEnvName,
  type SecretEnvRow,
} from "@/features/playbooks/secretsPolicy";

/**
 * Authoring list for a playbook's declared runtime secrets — the ONLY channel
 * through which a run can export a secret into the child process env. Each row
 * maps an `ENV_NAME` (validated as a reserved-safe uppercase identifier) to a
 * named secret picked by `key`. The parent serializes the rows into
 * `token_policy.runtime.secrets.env`; this is purely an authoring surface.
 */
function PlaybookSecretsEditor({
  rows,
  onChange,
  secrets,
  secretsLoading = false,
  readOnly = false,
}: Readonly<{
  rows: SecretEnvRow[];
  onChange: (rows: SecretEnvRow[]) => void;
  secrets: AutomationSecret[];
  secretsLoading?: boolean;
  readOnly?: boolean;
}>) {
  const { t } = useTranslation();

  const secretByKey = useMemo(() => {
    const map = new Map<string, AutomationSecret>();
    for (const secret of secrets) map.set(secret.key, secret);
    return map;
  }, [secrets]);

  const secretItems = useMemo(
    () =>
      secrets.map((secret) => ({
        id: secret.key,
        label: secret.description
          ? `${secret.key} — ${secret.description}`
          : secret.key,
        searchText: [secret.key, secret.description].filter(Boolean).join(" "),
      })),
    [secrets],
  );

  // A duplicate ENV name silently overwrites on save (last-wins), so surface it
  // as an advisory warning on every offending row.
  const duplicateEnvNames = useMemo(() => {
    const counts = new Map<string, number>();
    for (const row of rows) {
      const name = row.envName.trim();
      if (name) counts.set(name, (counts.get(name) ?? 0) + 1);
    }
    return new Set([...counts.entries()].filter(([, n]) => n > 1).map(([k]) => k));
  }, [rows]);

  const update = (index: number, patch: Partial<SecretEnvRow>) => {
    if (readOnly) return;
    onChange(rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  };
  const remove = (index: number) => {
    if (readOnly) return;
    onChange(rows.filter((_, i) => i !== index));
  };
  const add = () => {
    if (readOnly) return;
    onChange([...rows, { id: `secret-env-new-${rows.length}-${Date.now()}`, envName: "", key: "" }]);
  };

  const atLimit = rows.length >= MAX_SECRET_ENV;

  const envErrorText = (row: SecretEnvRow): string | null => {
    const error = validateEnvName(row.envName);
    if (row.envName.trim() === "") return null; // don't nag an untouched fresh row
    if (error === "pattern") return t("playbooks.page.secrets.errors.pattern");
    if (error === "reserved") return t("playbooks.page.secrets.errors.reserved");
    if (duplicateEnvNames.has(row.envName.trim())) {
      return t("playbooks.page.secrets.errors.duplicate");
    }
    return null;
  };

  return (
    <div className="space-y-2">
      <p className="text-xs leading-snug text-muted-foreground">
        {t("playbooks.page.secrets.reservedHint")}
      </p>

      {rows.length === 0 ? (
        <p className="rounded-md border border-dashed border-border bg-background px-3 py-3 text-sm text-muted-foreground">
          {t("playbooks.page.secrets.empty")}
        </p>
      ) : (
        <div className="space-y-2">
          {rows.map((row, index) => {
            const selected = row.key ? secretByKey.get(row.key) : undefined;
            const missingValue = Boolean(selected) && selected?.has_value === false;
            const unknownSecret = Boolean(row.key) && !selected && !secretsLoading;
            const error = envErrorText(row);
            return (
              <div
                key={row.id}
                className="space-y-2 rounded-md border border-border bg-background p-3"
              >
                <div className="grid gap-2 sm:grid-cols-12">
                  <FormField
                    label={t("playbooks.page.secrets.envName")}
                    className="sm:col-span-5"
                    labelClassName="text-sm font-medium text-foreground"
                  >
                    <Input
                      className="h-9 font-mono text-sm"
                      value={row.envName}
                      disabled={readOnly}
                      placeholder="GRAD_TOKEN"
                      aria-label={t("playbooks.page.secrets.envName")}
                      aria-invalid={error ? true : undefined}
                      onChange={(e) => update(index, { envName: e.target.value })}
                    />
                    {error ? (
                      <p className="mt-1 text-xs text-status-warning">{error}</p>
                    ) : null}
                  </FormField>
                  <FormField
                    label={t("playbooks.page.secrets.secret")}
                    className="sm:col-span-6"
                    labelClassName="text-sm font-medium text-foreground"
                  >
                    <SearchableSelect
                      value={row.key}
                      disabled={readOnly}
                      isLoading={secretsLoading}
                      onValueChange={(value) => update(index, { key: value })}
                      placeholder={t("playbooks.page.secrets.selectSecret")}
                      searchPlaceholder={t("search")}
                      triggerClassName="h-9"
                      selectedLabel={unknownSecret ? row.key : undefined}
                      items={secretItems}
                      emptyText={t("playbooks.page.secrets.noSecrets")}
                    />
                    <div className="mt-1 flex flex-wrap gap-1">
                      {missingValue ? (
                        <Badge variant="status-staging" className="rounded-md">
                          {t("playbooks.page.secrets.noValue")}
                        </Badge>
                      ) : null}
                      {unknownSecret ? (
                        <Badge variant="status-failed" className="rounded-md">
                          {t("playbooks.page.secrets.unknownSecret")}
                        </Badge>
                      ) : null}
                    </div>
                  </FormField>
                  <div className="flex items-start justify-end sm:col-span-1">
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="size-9 text-muted-foreground hover:text-destructive"
                      disabled={readOnly}
                      aria-label={t("playbooks.page.secrets.removeSecret")}
                      onClick={() => remove(index)}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {!readOnly ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="gap-2"
          disabled={atLimit}
          onClick={add}
        >
          <Plus className="h-3.5 w-3.5" />
          {t("playbooks.page.secrets.addSecret")}
        </Button>
      ) : null}
      {atLimit ? (
        <p className="text-xs text-muted-foreground">
          {t("playbooks.page.secrets.limitReached", { max: MAX_SECRET_ENV })}
        </p>
      ) : null}
    </div>
  );
}

export default PlaybookSecretsEditor;
