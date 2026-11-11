import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2 } from "lucide-react";

import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import type { AutomationSecret } from "@/features/automation-secrets/api";

import { FormField } from "./PlaybookFormFields";
import { formatStringArray, formatStringMap, jobDraftErrorDefaults, stringArrayOrNull, stringMapOrNull, type JobDraftError } from "./jobTemplateModel";

export function JobFieldError({ code }: Readonly<{ code?: JobDraftError }>) {
  const { t } = useTranslation();
  if (!code) return null;
  return <p className="text-xs text-status-warning">{t(`playbooks.jobDraft.errors.${code}`, { defaultValue: jobDraftErrorDefaults[code] })}</p>;
}

function uniqueName(prefix: string, taken: string[]): string {
  const names = new Set(taken);
  let index = taken.length + 1;
  while (names.has(`${prefix}_${index}`)) index += 1;
  return `${prefix}_${index}`;
}

function RowRemoveButton({ label, onClick, disabled }: Readonly<{ label: string; onClick: () => void; disabled?: boolean }>) {
  return (
    <Button type="button" variant="ghost" size="icon" className="size-9 text-muted-foreground hover:text-destructive" aria-label={label} disabled={disabled} onClick={onClick}>
      <Trash2 className="h-3.5 w-3.5" />
    </Button>
  );
}

type EntrypointsFieldProps = Readonly<{
  value: string;
  onChange: (value: string) => void;
  files: string[];
  filesLoading?: boolean;
  filesError?: string;
  projectSelected: boolean;
  projectRevisionMissing: boolean;
  error?: JobDraftError;
  disabled?: boolean;
}>;

/** Pick playbook entrypoints from the project's saved files; several paths are an ordered list. */
export function EntrypointsField({ value, onChange, files, filesLoading = false, filesError, projectSelected, projectRevisionMissing, error, disabled = false }: EntrypointsFieldProps) {
  const { t } = useTranslation();
  const paths = stringArrayOrNull(value);
  const selected = paths ?? [];
  const available = useMemo(() => {
    const selectedPaths = stringArrayOrNull(value) ?? [];
    return files.filter((path) => !selectedPaths.includes(path)).map((path) => ({ id: path, label: path }));
  }, [files, value]);
  const placeholder = !projectSelected
    ? t("playbooks.jobDraft.selectProjectFirst", { defaultValue: "Select a project first" })
    : filesError
      ? t("playbooks.jobDraft.filesUnavailable", { defaultValue: "Project files could not be loaded" })
      : projectRevisionMissing
        ? t("playbooks.jobDraft.noProjectRevision", { defaultValue: "The project has no saved revision" })
        : t("playbooks.jobDraft.addEntrypoint", { defaultValue: "Add an entrypoint" });

  return (
    <FormField label={t("playbooks.jobDraft.entrypoints")} className="md:col-span-2">
      {paths === null ? <p className="text-xs text-status-warning">{t("playbooks.jobDraft.fixInvalidJson", { defaultValue: "Fix the entrypoint JSON in advanced options." })}</p> : null}
      {selected.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("playbooks.jobDraft.noEntrypoints", { defaultValue: "No entrypoints selected yet." })}</p>
      ) : (
        <ul className="divide-y rounded-md border">
          {selected.map((path, index) => (
            <li key={`${path}-${index}`} className="flex items-center justify-between gap-2 px-3 py-2">
              <code className="min-w-0 break-all text-sm">{path}</code>
              <RowRemoveButton
                label={t("playbooks.jobDraft.removeEntrypoint", { defaultValue: "Remove {{path}}", path })}
                disabled={disabled}
                onClick={() => onChange(formatStringArray(selected.filter((_, row) => row !== index)))}
              />
            </li>
          ))}
        </ul>
      )}
      <SearchableSelect
        value=""
        aria-label={t("playbooks.jobDraft.entrypoints")}
        items={available}
        disabled={disabled || !projectSelected || projectRevisionMissing || Boolean(filesError) || filesLoading}
        isLoading={filesLoading}
        onValueChange={(path) => { if (path) onChange(formatStringArray([...selected, path])); }}
        placeholder={placeholder}
        searchPlaceholder={t("search")}
        emptyText={t("playbooks.jobDraft.noProjectFiles", { defaultValue: "No project files match" })}
      />
      {filesError ? <p className="text-xs text-status-warning">{filesError}</p> : null}
      <JobFieldError code={error} />
    </FormField>
  );
}

type EnvironmentFieldProps = Readonly<{
  value: string;
  onChange: (value: string) => void;
  error?: JobDraftError;
  disabled?: boolean;
}>;

/** Environment variables as repeatable name/value rows. */
export function EnvironmentField({ value, onChange, error, disabled = false }: EnvironmentFieldProps) {
  const { t } = useTranslation();
  const environment = stringMapOrNull(value);
  const rows = environment ? Object.entries(environment) : [];
  const commit = (entries: [string, string][]) => onChange(formatStringMap(Object.fromEntries(entries)));

  return (
    <FormField label={t("playbooks.jobDraft.environment", { defaultValue: "Environment variables" })} className="md:col-span-2">
      {environment === null ? <p className="text-xs text-status-warning">{t("playbooks.jobDraft.fixInvalidJson", { defaultValue: "Fix the entrypoint JSON in advanced options." })}</p> : null}
      {rows.length > 0 ? (
        <div className="space-y-2">
          {rows.map(([name, item], index) => (
            <div key={index} className="flex items-center gap-2">
              <Input
                className="h-9 font-mono text-sm"
                value={name}
                disabled={disabled || environment === null}
                aria-label={t("playbooks.jobDraft.variableName", { defaultValue: "Variable name {{number}}", number: index + 1 })}
                onChange={(event) => commit(rows.map((row, i) => (i === index ? [event.target.value, row[1]] : row)))}
              />
              <Input
                className="h-9 text-sm"
                value={item}
                disabled={disabled || environment === null}
                aria-label={t("playbooks.jobDraft.variableValue", { defaultValue: "Value {{number}}", number: index + 1 })}
                onChange={(event) => commit(rows.map((row, i) => (i === index ? [row[0], event.target.value] : row)))}
              />
              <RowRemoveButton
                label={t("playbooks.jobDraft.removeVariable", { defaultValue: "Remove variable {{name}}", name: name || String(index + 1) })}
                disabled={disabled || environment === null}
                onClick={() => commit(rows.filter((_, row) => row !== index))}
              />
            </div>
          ))}
        </div>
      ) : null}
      <div>
        <Button type="button" variant="outline" size="sm" className="gap-2" disabled={disabled || environment === null} onClick={() => commit([...rows, [uniqueName("ENV", rows.map(([name]) => name)), ""]])}>
          <Plus className="h-3.5 w-3.5" />
          {t("playbooks.jobDraft.addVariable", { defaultValue: "Add variable" })}
        </Button>
      </div>
      <JobFieldError code={error} />
    </FormField>
  );
}

type SecretReferencesFieldProps = Readonly<{
  value: string;
  onChange: (value: string) => void;
  secrets: AutomationSecret[];
  secretsLoading?: boolean;
  secretsError?: string;
  error?: JobDraftError;
  disabled?: boolean;
}>;

/** Secret references map an environment name to a named secret ID; the value is never shown. */
export function SecretReferencesField({ value, onChange, secrets, secretsLoading = false, secretsError, error, disabled = false }: SecretReferencesFieldProps) {
  const { t } = useTranslation();
  const references = stringMapOrNull(value);
  const rows = references ? Object.entries(references) : [];
  const commit = (entries: [string, string][]) => onChange(formatStringMap(Object.fromEntries(entries)));
  const secretById = useMemo(() => new Map(secrets.map((secret) => [secret.id, secret])), [secrets]);
  const secretItems = useMemo(() => secrets.map((secret) => ({
    id: secret.id,
    label: secret.description ? `${secret.key} — ${secret.description}` : secret.key,
    searchText: [secret.key, secret.description].filter(Boolean).join(" "),
  })), [secrets]);

  return (
    <FormField label={t("playbooks.jobDraft.secretReferences", { defaultValue: "Secret references" })} className="md:col-span-2">
      <p className="text-xs text-muted-foreground">{t("playbooks.jobDraft.secretReferencesHint", { defaultValue: "Each row exports a named secret into the run environment. Values are never shown." })}</p>
      {references === null ? <p className="text-xs text-status-warning">{t("playbooks.jobDraft.fixInvalidJson", { defaultValue: "Fix the entrypoint JSON in advanced options." })}</p> : null}
      {rows.length > 0 ? (
        <div className="space-y-2">
          {rows.map(([name, secretId], index) => {
            const selected = secretById.get(secretId);
            return (
              <div key={index} className="flex items-center gap-2">
                <Input
                  className="h-9 min-w-0 flex-1 font-mono text-sm"
                  value={name}
                  disabled={disabled || references === null}
                  aria-label={t("playbooks.jobDraft.secretVariableName", { defaultValue: "Secret variable name {{number}}", number: index + 1 })}
                  onChange={(event) => commit(rows.map((row, i) => (i === index ? [event.target.value, row[1]] : row)))}
                />
                <SearchableSelect
                  value={secretId}
                  items={secretItems}
                  disabled={disabled || references === null}
                  isLoading={secretsLoading}
                  selectedLabel={selected ? undefined : secretId}
                  onValueChange={(id) => commit(rows.map((row, i) => (i === index ? [row[0], id] : row)))}
                  placeholder={t("playbooks.page.secrets.selectSecret")}
                  searchPlaceholder={t("search")}
                  triggerClassName="h-9 min-w-0 flex-1"
                  emptyText={t("playbooks.page.secrets.noSecrets")}
                />
                <RowRemoveButton
                  label={t("playbooks.jobDraft.removeSecretReference", { defaultValue: "Remove secret reference {{name}}", name: name || String(index + 1) })}
                  disabled={disabled || references === null}
                  onClick={() => commit(rows.filter((_, row) => row !== index))}
                />
              </div>
            );
          })}
        </div>
      ) : null}
      {secretsError ? <p className="text-xs text-status-warning">{secretsError}</p> : null}
      <div>
        <Button type="button" variant="outline" size="sm" className="gap-2" disabled={disabled || references === null || secretsLoading} onClick={() => commit([...rows, [uniqueName("SECRET", rows.map(([name]) => name)), secrets[0]?.id ?? ""]])}>
          <Plus className="h-3.5 w-3.5" />
          {t("playbooks.jobDraft.addSecretReference", { defaultValue: "Add secret reference" })}
        </Button>
      </div>
      <JobFieldError code={error} />
    </FormField>
  );
}
