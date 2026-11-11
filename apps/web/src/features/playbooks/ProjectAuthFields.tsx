import { useTranslation } from "react-i18next";

import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import { Input } from "@/shared/ui-primitives/input";

import { FormField } from "./PlaybookFormFields";
import type { ProjectForm } from "./projectFormModel";
import type { ProjectAuthOptions } from "./useProjectAuthOptions";

function PickerGap({ gap, kind }: Readonly<{ gap: "forbidden" | "error" | null; kind: "secret" | "ssh" }>) {
  const { t } = useTranslation();
  if (!gap) return null;
  return (
    <p className="text-xs text-status-warning">
      {kind === "secret"
        ? t(gap === "forbidden" ? "playbooks.page.projects.workspace.secretsForbidden" : "playbooks.page.projects.workspace.secretsUnavailable", {
            defaultValue: gap === "forbidden"
              ? "You do not have permission to read secrets; enter the secret ID in advanced options."
              : "Secrets could not be loaded; enter the secret ID in advanced options.",
          })
        : t(gap === "forbidden" ? "playbooks.page.projects.workspace.sshRecordsForbidden" : "playbooks.page.projects.workspace.sshRecordsUnavailable", {
            defaultValue: gap === "forbidden"
              ? "You do not have permission to read SSH records; enter the record ID in advanced options."
              : "SSH records could not be loaded; enter the record ID in advanced options.",
          })}
    </p>
  );
}

/** Git credentials for a project: named pickers with the raw identifiers kept in advanced options. */
export function ProjectAuthFields({ form, onChange, options, disabled = false }: Readonly<{
  form: ProjectForm;
  onChange: React.Dispatch<React.SetStateAction<ProjectForm>>;
  options: ProjectAuthOptions;
  disabled?: boolean;
}>) {
  const { t } = useTranslation();
  const patch = (value: Partial<ProjectForm>) => onChange((prev) => ({ ...prev, ...value }));
  const secretSelected = options.secrets.find((secret) => secret.id === form.secretId);
  const sshSelected = options.sshRecords.find((record) => record.id === form.sshRecordId);
  return (
    <>
      <FormSelect label={t("playbooks.page.projects.workspace.authentication", { defaultValue: "Authentication" })} value={form.authKind} onValueChange={(value) => patch({ authKind: value })} options={[{ value: "anonymous", label: "Anonymous" }, { value: "https_basic", label: "HTTPS basic" }, { value: "https_bearer", label: "HTTPS bearer" }, { value: "ssh_record", label: "SSH record" }]} />
      {form.authKind === "https_basic" && <FormField label={t("playbooks.page.projects.workspace.username", { defaultValue: "Username" })}><Input value={form.username} disabled={disabled} onChange={(event) => patch({ username: event.target.value })} /></FormField>}
      {(form.authKind === "https_basic" || form.authKind === "https_bearer") && (
        <FormField label={t("playbooks.page.projects.workspace.secret", { defaultValue: "Secret" })}>
          <SearchableSelect
            value={form.secretId}
            onValueChange={(secretId) => patch({ secretId })}
            disabled={disabled || options.secretsLoading || options.secretsGap !== null}
            isLoading={options.secretsLoading}
            items={options.secrets.map((secret) => ({ id: secret.id, label: secret.description ? `${secret.key} — ${secret.description}` : secret.key, searchText: [secret.key, secret.description].filter(Boolean).join(" ") }))}
            selectedLabel={!secretSelected && form.secretId ? form.secretId : undefined}
            placeholder={t("playbooks.page.projects.workspace.selectSecret", { defaultValue: "Select a secret" })}
            searchPlaceholder={t("search")}
            emptyText={t("playbooks.page.projects.workspace.noSecrets", { defaultValue: "No secrets available" })}
          />
          <PickerGap gap={options.secretsGap} kind="secret" />
          <details className="text-xs text-muted-foreground">
            <summary className="cursor-pointer">{t("playbooks.page.projects.workspace.advancedId", { defaultValue: "Advanced: raw ID" })}</summary>
            <Input className="mt-2 font-mono text-xs" aria-label={t("playbooks.page.projects.workspace.secretId", { defaultValue: "Secret ID" })} value={form.secretId} disabled={disabled} onChange={(event) => patch({ secretId: event.target.value })} />
          </details>
        </FormField>
      )}
      {form.authKind === "ssh_record" && (
        <FormField label={t("playbooks.page.projects.workspace.sshRecord", { defaultValue: "SSH record" })}>
          <SearchableSelect
            value={form.sshRecordId}
            onValueChange={(sshRecordId) => patch({ sshRecordId })}
            disabled={disabled || options.sshRecordsLoading || options.sshRecordsGap !== null}
            isLoading={options.sshRecordsLoading}
            items={options.sshRecords.map((record) => ({ id: record.id, label: record.name, searchText: [record.name, record.protocol].filter(Boolean).join(" ") }))}
            selectedLabel={!sshSelected && form.sshRecordId ? form.sshRecordId : undefined}
            placeholder={t("playbooks.page.projects.workspace.selectSshRecord", { defaultValue: "Select an SSH record" })}
            searchPlaceholder={t("search")}
            emptyText={t("playbooks.page.projects.workspace.noSshRecords", { defaultValue: "No SSH records available" })}
          />
          <PickerGap gap={options.sshRecordsGap} kind="ssh" />
          <details className="text-xs text-muted-foreground">
            <summary className="cursor-pointer">{t("playbooks.page.projects.workspace.advancedId", { defaultValue: "Advanced: raw ID" })}</summary>
            <Input className="mt-2 font-mono text-xs" aria-label={t("playbooks.page.projects.workspace.sshRecordId", { defaultValue: "SSH record ID" })} value={form.sshRecordId} disabled={disabled} onChange={(event) => patch({ sshRecordId: event.target.value })} />
          </details>
        </FormField>
      )}
    </>
  );
}
