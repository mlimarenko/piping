import { useLayoutEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { usePermissions } from "@/shared/auth/permissionsContext";
import FormSection from "@/shared/ui-shared/FormSection";
import FormField from "@/shared/ui-shared/FormField";
import PageState from "@/shared/ui-shared/PageState";
import { useUnsavedChangesGuard } from "@/shared/ui-shared/useUnsavedChangesGuard";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { Input } from "@/shared/ui-primitives/input";
import { Button } from "@/shared/ui-primitives/button";
import { getSdLabelSettings, updateSdLabelSettings, type SdLabelGroups } from "./api";

const KEY = ["monitoring", "sd-label-settings"] as const;
const GROUPS = ["location", "tenant", "role", "status", "hardware", "network", "contacts", "description", "identifiers"] as const;
const STATUSES = ["device_statuses", "site_statuses"] as const;
const normalize = (text: string) => [...new Set(text.split(",").map(value => value.trim().toLowerCase()).filter(Boolean))].sort();
const texts = (groups: SdLabelGroups) => ({ device_statuses: groups.device_statuses.join(", "), site_statuses: groups.site_statuses.join(", ") });

function LabelForm({ initial }: { initial: SdLabelGroups }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const allowed = can("UPDATE", "MONITORING");
  const permission = useRef(allowed);
  const [baseline, setBaseline] = useState(initial);
  const [draft, setDraft] = useState(initial);
  const latest = useRef(initial);
  useLayoutEffect(() => { permission.current = allowed; latest.current = draft; }, [allowed, draft]);
  const [statusTexts, setStatusTexts] = useState(() => texts(initial));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState(false);
  const [saved, setSaved] = useState(false);
  const running = useRef(false);
  const queued = useRef<SdLabelGroups | null>(null);
  const mounted = useRef(false);
  useLayoutEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; queued.current = null; };
  }, []);
  const dirty = JSON.stringify(draft) !== JSON.stringify(baseline) || STATUSES.some(field => JSON.stringify(normalize(statusTexts[field])) !== JSON.stringify(draft[field]));
  const { discardDialog } = useUnsavedChangesGuard(dirty || error, saving);
  // Clean forms follow refreshed server values. Typed or failed drafts keep
  // their own baseline until a successful write or an explicit exit.
  if (!dirty && !saving && !error && JSON.stringify(initial) !== JSON.stringify(baseline)) {
    setBaseline(initial);
    setDraft(initial);
    setStatusTexts(texts(initial));
  }

  const drain = async () => {
    if (running.current || !permission.current || !mounted.current) return;
    running.current = true;
    setSaving(true);
    setError(false);
    setSaved(false);
    try {
      // Only one request may own the complete groups document at a time.
      // Edits made during that request coalesce into the next document.
      while (queued.current && permission.current && mounted.current) {
        // A cancelled Query ignores late results even when its transport does
        // not consume AbortSignal. Await cancellation before starting a write.
        await qc.cancelQueries({ queryKey: KEY, exact: true });
        if (!permission.current || !mounted.current) break;
        const next = queued.current;
        queued.current = null;
        const response = await updateSdLabelSettings(next);
        // Also cancel reads begun during the PUT before publishing its response.
        // Reads begun after this acknowledgement can reflect legitimate changes.
        await qc.cancelQueries({ queryKey: KEY, exact: true });
        if (mounted.current) setBaseline(next);
        qc.setQueryData(KEY, response);
        void qc.invalidateQueries({ queryKey: ["observability", "service-discovery-preview"] });
      }
      if (mounted.current && !queued.current) setSaved(true);
    } catch {
      if (mounted.current) {
        queued.current = latest.current;
        setError(true);
      }
    } finally {
      running.current = false;
      if (mounted.current) setSaving(false);
    }
  };
  const save = (next: SdLabelGroups) => {
    if (!permission.current) return;
    latest.current = next;
    setDraft(next);
    queued.current = next;
    void drain();
  };
  return <div className="flex min-w-0 flex-col gap-6">
    <FormSection title={t("observability.sd.labelGroups.title")} description={t("observability.sd.labelGroups.intro")} contentClassName="grid gap-4 sm:grid-cols-2">
      {GROUPS.map(key => <FormField key={key} htmlFor={`sd-label-${key}`} label={t(`observability.sd.labelGroups.groups.${key}.title`)} help={t(`observability.sd.labelGroups.groups.${key}.helper`)}>
        <Checkbox id={`sd-label-${key}`} disabled={!allowed} checked={draft[key]} onCheckedChange={checked => save({ ...latest.current, [key]: checked === true })} />
      </FormField>)}
    </FormSection>
    <FormSection title={t("observability.sd.statusFilters.title")} description={t("observability.sd.statusFilters.helper")} contentClassName="grid gap-4 sm:grid-cols-2">
      {STATUSES.map(field => <FormField key={field} htmlFor={`sd-${field}`} label={t(`observability.sd.statusFilters.${field}`)}>
        <Input id={`sd-${field}`} disabled={!allowed} value={statusTexts[field]} onChange={event => { setSaved(false); setStatusTexts(current => ({ ...current, [field]: event.target.value })); }} onBlur={() => {
          const normalized = normalize(statusTexts[field]);
          setStatusTexts(current => ({ ...current, [field]: normalized.join(", ") }));
          if (JSON.stringify(normalized) !== JSON.stringify(latest.current[field])) save({ ...latest.current, [field]: normalized });
        }} />
        <div className="flex flex-wrap gap-2">{["active", "offline", "planned"].map(status => <label key={status} className="inline-flex min-h-10 items-center gap-2 rounded-md border border-border px-2 text-sm">
          <Checkbox disabled={!allowed} checked={draft[field].includes(status)} onCheckedChange={checked => {
            const next = new Set(normalize(statusTexts[field]));
            if (checked) next.add(status); else next.delete(status);
            const statuses = [...next].sort();
            setStatusTexts(current => ({ ...current, [field]: statuses.join(", ") }));
            save({ ...latest.current, [field]: statuses });
          }} />{status}
        </label>)}</div>
      </FormField>)}
    </FormSection>
    <div role={error ? "alert" : "status"} aria-live="polite">
      {saving ? t("saving") : error ? <div className="flex flex-wrap items-center gap-2 text-destructive">{t("observability.workflow.saveFailed", { defaultValue: "Could not save. Your draft is retained. Try again." })}<Button variant="outline" disabled={!allowed} onClick={() => { queued.current = latest.current; void drain(); }}>{t("retry")}</Button></div> : saved && !dirty ? t("observability.sd.labelGroups.saved") : null}
    </div>
    {discardDialog}
  </div>;
}

export default function SdLabelSettings() {
  const { t } = useTranslation();
  const query = useQuery({ queryKey: KEY, queryFn: getSdLabelSettings, retry: false });
  if (query.isPending) return <PageState kind="loading" title={t("loading")} />;
  if (!query.data) return <PageState kind="error" title={t("observability.entityLoadFailed")} action={<Button variant="outline" onClick={() => { void query.refetch(); }}>{t("retry")}</Button>} />;
  return <>{query.isError && <p role="alert">{t("observability.entityLoadFailed")} <Button variant="outline" onClick={() => { void query.refetch(); }}>{t("retry")}</Button></p>}<LabelForm initial={query.data.groups} /></>;
}
