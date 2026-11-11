import { useState, type ReactNode } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { collectAllPages } from "@/shared/api/pagination";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import PageState from "@/shared/ui-shared/PageState";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Switch } from "@/shared/ui-primitives/switch";
import { Button } from "@/shared/ui-primitives/button";
import { createProfile, updateProfile, getProfile, listExporters, listMetricSets, listQuerySources, listAlertSources, type MonitoringProfile, type MonitoringProfileRequest, type ExporterCatalogItem } from "./api";
import { PROFILES_KEY, invalidateMonitoringProfileCaches } from "./keys";
import { PROFILES_BASE } from "./ProfilesScreen";

interface ItemDraft { key: string; exporter_id: string; job: string; scheme: string; port: string; metrics_path: string; labels: string; enabled: boolean }
const newItem = (): ItemDraft => ({ key: crypto.randomUUID(), exporter_id: "", job: "", scheme: "", port: "", metrics_path: "", labels: "{}", enabled: true });

export function ProfileForm({ editing, exporters, links, refreshError }: { editing: MonitoringProfile | null; exporters: ExporterCatalogItem[]; links: { sets: { id: string; name: string }[]; sources: { id: string; name: string }[]; alerts: { id: string; name: string }[] }; refreshError?: ReactNode }) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const allowed = can(editing ? "UPDATE" : "CREATE", "MONITORING");
  const [baseline] = useState(() => ({ name: editing?.name ?? "", slug: editing?.slug ?? "", description: editing?.description ?? "", enabled: editing?.enabled ?? true, metric_set_id: editing?.metric_set_id ?? "", query_source_id: editing?.query_source_id ?? "", alert_source_id: editing?.alert_source_id ?? "", items: editing?.items.map(item => ({ key: item.id, exporter_id: item.exporter_id, job: item.job ?? "", scheme: item.scheme ?? "", port: item.port == null ? "" : String(item.port), metrics_path: item.metrics_path ?? "", labels: JSON.stringify(item.labels ?? {}, null, 2), enabled: item.enabled })) ?? [newItem()] }));
  const [draft, setDraft] = useState(baseline);
  const [validation, setValidation] = useState<string | null>(null);
  const mutation = useApiMutation({
    mutationFn: (body: MonitoringProfileRequest) => {
      if (!can(editing ? "UPDATE" : "CREATE", "MONITORING")) throw new Error("Permission removed");
      return editing ? updateProfile(editing.id, body) : createProfile(body);
    },
    onSuccess: profile => {
      qc.setQueryData([...PROFILES_KEY, profile.id], profile);
      void invalidateMonitoringProfileCaches(qc);
      void qc.invalidateQueries({ queryKey: [...PROFILES_KEY, profile.id] });
      void navigate(`${PROFILES_BASE}/${profile.id}`);
    },
  });
  const set = <K extends keyof typeof draft>(key: K, value: typeof draft[K]) => setDraft(current => ({ ...current, [key]: value }));
  const setItem = <K extends keyof ItemDraft>(key: string, field: K, value: ItemDraft[K]) => setDraft(current => ({ ...current, items: current.items.map(item => item.key === key ? { ...item, [field]: value } : item) }));
  const back = () => { if (!mutation.isPending) void navigate(editing ? `${PROFILES_BASE}/${editing.id}` : "/observability?tab=profiles"); };
  const submit = () => {
    if (!allowed || mutation.isPending) return;
    setValidation(null);
    try {
      const items = draft.items.map((item, index) => {
        const labels: unknown = JSON.parse(item.labels || "{}");
        if (!labels || typeof labels !== "object" || Array.isArray(labels)) throw new Error("labels");
        const port = item.port.trim() ? Number(item.port) : null;
        if (port !== null && (!Number.isInteger(port) || port < 1 || port > 65535)) throw new Error("port");
        return { exporter_id: item.exporter_id, enabled: item.enabled, job: item.job.trim() || null, scheme: item.scheme || null, port, metrics_path: item.metrics_path.trim() || null, labels, sort_order: index };
      });
      mutation.mutate({ ...draft, name: draft.name.trim(), slug: draft.slug.trim(), description: draft.description.trim() || null, metric_set_id: draft.metric_set_id || null, query_source_id: draft.query_source_id || null, alert_source_id: draft.alert_source_id || null, items });
    } catch { setValidation(t("observability.profiles.invalidItem")); }
  };
  const reference = (field: "metric_set_id" | "query_source_id" | "alert_source_id", label: string, options: { id: string; name: string }[]) => <FormSelect label={label} value={draft[field] || "none"} onValueChange={value => set(field, value === "none" ? "" : value)} options={[{ value: "none", label: "—" }, ...options.map(option => ({ value: option.id, label: option.name }))]} />;
  return <EntityEditorPage scope="routed" title={t(editing ? "observability.profiles.edit" : "observability.profiles.new")} onBack={back} backLabel={t("back")} onCancel={back} cancelLabel={t("cancel")} onSave={submit} saveLabel={t(editing ? "save" : "create")} saving={mutation.isPending}
    saveDisabled={!allowed || !draft.name.trim() || !draft.slug.trim() || !draft.items.length || draft.items.some(item => !item.exporter_id) || mutation.isPending} dirty={JSON.stringify(draft) !== JSON.stringify(baseline)} dirtyMessage={t("editor.unsavedChanges")} error={validation || mutation.isError || refreshError ? <>{validation ?? (mutation.isError ? t("observability.workflow.saveFailed") : undefined)}{refreshError}</> : undefined}>
    <fieldset disabled={!allowed || mutation.isPending} className="grid min-w-0 gap-4">
      <div className="grid min-w-0 gap-4 sm:grid-cols-2">
        <FormField htmlFor="profile-name" label={t("name")}><FormFieldControl><Input id="profile-name" required value={draft.name} onChange={event => set("name", event.target.value)} /></FormFieldControl></FormField>
        <FormField htmlFor="profile-slug" label="Slug"><FormFieldControl><Input id="profile-slug" required value={draft.slug} onChange={event => set("slug", event.target.value)} /></FormFieldControl></FormField>
      </div>
      <FormField htmlFor="profile-description" label={t("description")}><FormFieldControl><Textarea id="profile-description" value={draft.description} onChange={event => set("description", event.target.value)} /></FormFieldControl></FormField>
      <FormField htmlFor="profile-enabled" label={t("enabled")}><FormFieldControl><Switch id="profile-enabled" checked={draft.enabled} onCheckedChange={value => set("enabled", value)} /></FormFieldControl></FormField>
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {reference("metric_set_id", t("observability.profiles.metricSet"), links.sets)}
        {reference("query_source_id", t("observability.profiles.querySource"), links.sources)}
        {reference("alert_source_id", t("observability.profiles.alertSource"), links.alerts)}
      </div>
      <h2 className="text-sm font-semibold">{t("observability.profiles.items")}</h2>
      {draft.items.map((item, index) => <fieldset key={item.key} className="grid min-w-0 gap-4 rounded-md border border-border p-4">
        <legend className="px-1 text-sm font-medium">{t("observability.profiles.item", { index: index + 1 })}</legend>
        <div className="grid min-w-0 gap-4 sm:grid-cols-2 xl:grid-cols-3">
          <FormSelect label={t("observability.sd.exporter")} value={item.exporter_id} onValueChange={value => setItem(item.key, "exporter_id", value)} options={exporters.map(exporter => ({ value: exporter.id, label: exporter.name }))} required />
          <FormField htmlFor={`${item.key}-job`} label="Job"><FormFieldControl><Input id={`${item.key}-job`} value={item.job} onChange={event => setItem(item.key, "job", event.target.value)} /></FormFieldControl></FormField>
          <FormField htmlFor={`${item.key}-port`} label={t("observability.sd.port")}><FormFieldControl><Input id={`${item.key}-port`} type="number" min={1} max={65535} value={item.port} onChange={event => setItem(item.key, "port", event.target.value)} /></FormFieldControl></FormField>
          <FormSelect label={t("observability.sd.scheme")} value={item.scheme || "default"} onValueChange={value => setItem(item.key, "scheme", value === "default" ? "" : value)} options={[{ value: "default", label: t("observability.workflow.profileDefaults") }, { value: "http", label: "HTTP" }, { value: "https", label: "HTTPS" }]} />
          <FormField htmlFor={`${item.key}-path`} label={t("observability.sd.metricsPath")}><FormFieldControl><Input id={`${item.key}-path`} value={item.metrics_path} onChange={event => setItem(item.key, "metrics_path", event.target.value)} /></FormFieldControl></FormField>
          <FormField htmlFor={`${item.key}-enabled`} label={t("enabled")}><FormFieldControl><Switch id={`${item.key}-enabled`} checked={item.enabled} onCheckedChange={value => setItem(item.key, "enabled", value)} /></FormFieldControl></FormField>
        </div>
        <FormField htmlFor={`${item.key}-labels`} label={t("observability.sd.labels")}><FormFieldControl><Textarea id={`${item.key}-labels`} className="font-mono" value={item.labels} onChange={event => setItem(item.key, "labels", event.target.value)} /></FormFieldControl></FormField>
        <Button variant="outline" className="justify-self-start" onClick={() => set("items", draft.items.filter(current => current.key !== item.key))}><Trash2 className="size-4" />{t("observability.profiles.removeItem", { index: index + 1 })}</Button>
      </fieldset>)}
      <Button variant="outline" className="justify-self-start" onClick={() => set("items", [...draft.items, newItem()])}><Plus className="size-4" />{t("observability.profiles.addItem")}</Button>
    </fieldset>
  </EntityEditorPage>;
}

export default function ProfileEditor() {
  const { id } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const profile = useQuery({ queryKey: [...PROFILES_KEY, id], queryFn: () => getProfile(id!), enabled: !!id });
  const references = useQuery({ queryKey: [...PROFILES_KEY, "editor-references"], queryFn: async () => {
    const [exporters, sets, sources, alerts] = await Promise.all([
      collectAllPages((limit, offset) => listExporters({ limit, offset }), 200),
      collectAllPages((limit, offset) => listMetricSets({ limit, offset }), 200),
      collectAllPages((limit, offset) => listQuerySources({ limit, offset }), 200),
      collectAllPages((limit, offset) => listAlertSources({ limit, offset }), 200),
    ]);
    return { exporters, links: { sets, sources, alerts } };
  } });
  const statePage = (content: ReactNode) => <EntityEditorPage scope="routed" title={t(id ? "observability.profiles.edit" : "observability.profiles.new")} onBack={() => void navigate("/observability?tab=profiles")} backLabel={t("back")} cancelLabel={t("cancel")}>{content}</EntityEditorPage>;
  if (references.isPending || (id && profile.isPending)) return statePage(<PageState kind="loading" title={t("loading")} />);
  if ((references.isError && !references.data) || (id && profile.isError && !profile.data)) return statePage(<PageState kind="error" title={t("observability.entityLoadFailed")} action={<Button onClick={() => { void references.refetch(); if (id) void profile.refetch(); }}>{t("retry")}</Button>} />);
  return <ProfileForm key={id ?? "new"} editing={profile.data ?? null} exporters={references.data.exporters} links={references.data.links} refreshError={references.isError || profile.isError ? <div role="alert">{t("observability.entityLoadFailed")}<Button variant="outline" onClick={() => { void references.refetch(); if (id) void profile.refetch(); }}>{t("retry")}</Button></div> : undefined} />;
}
