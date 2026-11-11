import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { FormField, ToggleField } from "./PlaybookFormFields";
import { decodeAnsibleOptions, emptyAnsibleOptions, encodeAnsibleOptions, type AnsibleOptions } from "./ansibleOptions";

export function AnsibleOptionsFields({ value, onChange, limit, onLimitChange, disabled = false }: Readonly<{
  value: string;
  onChange: (value: string) => void;
  limit: string;
  onLimitChange: (value: string) => void;
  disabled?: boolean;
}>) {
  const { t } = useTranslation();
  const options = decodeAnsibleOptions(value);
  if (!options) return <div className="flex flex-col items-start gap-2 md:col-span-2">
    <p role="alert" className="text-sm text-status-warning">{t("playbooks.ansibleOptions.unsupported", { defaultValue: "This job contains unsupported or conflicting arguments. Clear them and configure the supported options before saving." })}</p>
    <Button type="button" variant="outline" disabled={disabled} onClick={() => onChange(encodeAnsibleOptions(emptyAnsibleOptions()))}>{t("playbooks.ansibleOptions.clear", { defaultValue: "Clear unsupported arguments" })}</Button>
  </div>;
  const set = <K extends keyof AnsibleOptions>(key: K, next: AnsibleOptions[K]) => onChange(encodeAnsibleOptions({ ...options, [key]: next }));
  return <section className="grid min-w-0 gap-4 md:col-span-2 md:grid-cols-2" aria-label={t("playbooks.ansibleOptions.title", { defaultValue: "Ansible options" })}>
    <h2 className="text-sm font-medium md:col-span-2">{t("playbooks.ansibleOptions.title", { defaultValue: "Ansible options" })}</h2>
    <FormField label={t("playbooks.ansibleOptions.tags", { defaultValue: "Tags (comma-separated)" })}><Input value={options.tags} disabled={disabled} onChange={(event) => set("tags", event.target.value)} /></FormField>
    <FormField label={t("playbooks.ansibleOptions.skipTags", { defaultValue: "Skip tags (comma-separated)" })}><Input value={options.skipTags} disabled={disabled} onChange={(event) => set("skipTags", event.target.value)} /></FormField>
    <FormField label={t("playbooks.jobDraft.limit", { defaultValue: "Limit" })}><Input value={limit} disabled={disabled} onChange={(event) => onLimitChange(event.target.value)} /></FormField>
    <FormSelect label={t("playbooks.ansibleOptions.verbosity", { defaultValue: "Verbosity" })} value={String(options.verbosity)} disabled={disabled} onValueChange={(value) => set("verbosity", Number(value))} options={[0, 1, 2, 3, 4].map((level) => ({ value: String(level), label: String(level) }))} />
    <ToggleField label={t("playbooks.ansibleOptions.check", { defaultValue: "Check mode" })} description={t("playbooks.ansibleOptions.checkHint", { defaultValue: "Requests a dry run. Modules and tasks decide how they handle check mode." })} checked={options.check} disabled={disabled} onCheckedChange={(checked) => set("check", checked)} />
    <ToggleField label={t("playbooks.ansibleOptions.diff", { defaultValue: "Show differences" })} checked={options.diff} disabled={disabled} onCheckedChange={(checked) => set("diff", checked)} />
    {options.diff && <p className="text-sm text-muted-foreground md:col-span-2">{t("playbooks.ansibleOptions.diffHint", { defaultValue: "Differences may include file contents in the run output." })}</p>}
    <div className="md:col-span-2"><ToggleField label={t("playbooks.ansibleOptions.become", { defaultValue: "Privilege escalation (become)" })} checked={options.become} disabled={disabled} onCheckedChange={(become) => onChange(encodeAnsibleOptions({ ...options, become, ...(!become ? { becomeUser: "", becomeMethod: "" } : {}) }))} /></div>
    {options.become && <>
      <FormField label={t("playbooks.ansibleOptions.becomeUser", { defaultValue: "Become user" })}><Input value={options.becomeUser} placeholder="root" disabled={disabled} onChange={(event) => set("becomeUser", event.target.value)} /></FormField>
      <FormField label={t("playbooks.ansibleOptions.becomeMethod", { defaultValue: "Become method" })}><Input value={options.becomeMethod} placeholder="sudo" disabled={disabled} onChange={(event) => set("becomeMethod", event.target.value)} /></FormField>
    </>}
    <p className="text-sm text-muted-foreground md:col-span-2">{t("playbooks.ansibleOptions.security", { defaultValue: "Inventory, variables and credentials use their dedicated fields. Interactive prompts and Vault CLI inputs are not supported." })}</p>
  </section>;
}
