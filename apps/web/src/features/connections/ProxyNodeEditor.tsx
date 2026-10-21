import { useState, type ReactNode } from "react";
import { useNavigate, useParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Loader2, Radio } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { RequireCapability } from "@/shared/auth/permissions";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import EmptyState from "@/shared/ui-shared/EmptyState";
import FormField from "@/shared/ui-shared/FormField";
import { Input } from "@/shared/ui-primitives/input";
import { Label } from "@/shared/ui-primitives/label";
import { Switch } from "@/shared/ui-primitives/switch";
import { notify } from "@/shared/lib/notifications";
import {
  createProxyNode,
  listProxyNodes,
  updateProxyNode,
  type CreatedProxyNodeResponse,
} from "@/features/connections/proxies-api";
import type { ProxyNode as BackendProxyNode } from "@/shared/api/types";
import type { components } from "@/shared/api/schema.gen";
import type { ApiError } from "@/shared/api/client";
import ProxyNodeBootstrapDialog from "./ProxyNodeBootstrapDialog";
import { PROXY_NODES_ROUTE } from "./proxyRecordRoutes";

type UpdateProxyNodeRequest = components["schemas"]["UpdateProxyNodeRequest"];

interface FormData { name: string; slug: string; baseUrl: string; enabled: boolean; }
interface FormErrors { name?: string; slug?: string; baseUrl?: string; }
const emptyForm: FormData = { name: "", slug: "", baseUrl: "https://", enabled: true };

function formFromNode(node: BackendProxyNode): FormData {
  return { name: node.name, slug: node.slug, baseUrl: node.base_url, enabled: node.enabled };
}

function NodeEditor({ node }: Readonly<{ node?: BackendProxyNode }>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [baseline] = useState<FormData>(() => node ? formFromNode(node) : emptyForm);
  const [form, setForm] = useState(baseline);
  const [errors, setErrors] = useState<FormErrors>({});
  const [bootstrap, setBootstrap] = useState<CreatedProxyNodeResponse | null>(null);
  const back = () => { void navigate(PROXY_NODES_ROUTE); };

  const validate = (f: FormData): FormErrors => {
    const errs: FormErrors = {};
    if (!f.name.trim()) errs.name = t("proxyNodes.errors.nameRequired");
    if (!f.slug.trim()) errs.slug = t("proxyNodes.errors.slugRequired");
    else if (!/^[a-z0-9-]+$/.test(f.slug.trim())) errs.slug = t("proxyNodes.errors.slugFormat");
    if (!f.baseUrl.trim()) errs.baseUrl = t("proxyNodes.errors.baseUrlRequired");
    else if (!/^https?:\/\/.+/.test(f.baseUrl.trim())) errs.baseUrl = t("proxyNodes.errors.urlInvalid");
    return errs;
  };

  const invalidate = () => qc.invalidateQueries({ queryKey: ["proxy-nodes"] });

  const createMutation = useApiMutation({
    mutationFn: createProxyNode,
    onSuccess: (created) => {
      void invalidate();
      notify("success", t("proxyNodes.nodeCreated", { name: created.node.name }));
      setBootstrap(created);
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const updateMutation = useApiMutation({
    mutationFn: (body: UpdateProxyNodeRequest) => updateProxyNode(node!.id, body),
    onSuccess: (updated) => {
      void invalidate();
      notify("success", t("proxyNodes.nodeUpdated", { name: updated.name }));
      back();
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const setField = (key: keyof FormData, value: string | boolean) => {
    if (createMutation.isPending || updateMutation.isPending) return;
    setForm(prev => ({ ...prev, [key]: value }));
    if (errors[key as keyof FormErrors]) setErrors(prev => ({ ...prev, [key]: undefined }));
  };

  const autoSlug = (name: string) => {
    if (createMutation.isPending || updateMutation.isPending) return;
    setForm(prev => node
      ? { ...prev, name }
      : { ...prev, name, slug: name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "") });
    if (errors.name) setErrors(prev => ({ ...prev, name: undefined }));
  };

  const handleSave = () => {
    const errs = validate(form);
    setErrors(errs);
    if (Object.keys(errs).length > 0) return;
    const payload = {
      name: form.name.trim(),
      slug: form.slug.trim(),
      base_url: form.baseUrl.trim(),
      enabled: form.enabled,
    };
    if (node) updateMutation.mutate(payload);
    else createMutation.mutate(payload);
  };

  return (
    <>
      <EntityEditorPage scope="routed"
        width="compact"
        title={t(node ? "proxyNodes.editNode" : "proxyNodes.addNodeTitle")}
        description={t(node ? "proxyNodes.updateDescription" : "proxyNodes.addDescription")}
        onBack={back}
        backLabel={t(node ? "proxyNodes.backToNodes" : "proxyNodes.backToList")}
        onCancel={back}
        cancelLabel={t("cancel")}
        onSave={handleSave}
        saveLabel={t(node ? "saveChanges" : "proxyNodes.addNode")}
        saving={createMutation.isPending || updateMutation.isPending}
        dirty={JSON.stringify(form) !== JSON.stringify(baseline)}
      >
        <fieldset disabled={createMutation.isPending || updateMutation.isPending || !!bootstrap} inert={createMutation.isPending || updateMutation.isPending || !!bootstrap} className="flex min-w-0 flex-col gap-4">
          <FormField htmlFor="proxy-node-name" label={t("name")} required error={errors.name}>
            <Input id="proxy-node-name" value={form.name} onChange={e => autoSlug(e.target.value)} placeholder={t("proxyNodes.nodeName")} maxLength={100} />
          </FormField>
          <FormField htmlFor="proxy-node-slug" label={t("proxyNodes.slug")} required error={errors.slug}>
            <Input id="proxy-node-slug" value={form.slug} onChange={e => setField("slug", e.target.value)} placeholder={t("proxyNodes.slugPlaceholder")} className="font-mono text-[11px]" maxLength={100} />
          </FormField>
          <FormField htmlFor="proxy-node-url" label={t("proxyNodes.baseUrl")} required error={errors.baseUrl}>
            <Input id="proxy-node-url" value={form.baseUrl} onChange={e => setField("baseUrl", e.target.value)} placeholder={t("proxyNodes.baseUrlPlaceholder")} className="font-mono text-[11px]" maxLength={500} />
          </FormField>
          <div className="flex items-center gap-2 pt-1">
            <Switch checked={form.enabled} onCheckedChange={v => setField("enabled", v)} id="node-enabled" />
            <Label htmlFor="node-enabled">{t("enabled")}</Label>
          </div>
        </fieldset>
      </EntityEditorPage>
      <ProxyNodeBootstrapDialog
        bootstrap={bootstrap}
        onClose={() => { setBootstrap(null); back(); }}
      />
    </>
  );
}

export default function ProxyNodeEditor() {
  const { id } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery({
    queryKey: ["proxy-nodes", "options"],
    queryFn: () => listProxyNodes({ limit: 1000, offset: 0 }),
  });
  const back = () => { void navigate(PROXY_NODES_ROUTE); };
  const renderState = (content: ReactNode) => <EntityEditorPage scope="routed"
    width="compact" title={t(id ? "proxyNodes.editNode" : "proxyNodes.addNodeTitle")}
    description={t(id ? "proxyNodes.updateDescription" : "proxyNodes.addDescription")}
    onBack={back} backLabel={t(id ? "proxyNodes.backToNodes" : "proxyNodes.backToList")}
    onCancel={back} cancelLabel={t("cancel")}>
    {content}
  </EntityEditorPage>;
  if (query.isPending) return renderState(<output className="flex justify-center p-8"><Loader2 className="size-6 animate-spin" aria-label={t("loading")} /></output>);
  if (query.isError) return renderState(<EmptyState icon={Radio} title={t("proxyNodes.loadFailed")} actionLabel={t("retry")} onAction={() => { void query.refetch(); }} />);
  const node = query.data.items.find(candidate => candidate.id === id);
  if (id && !node) return renderState(<EmptyState icon={Radio} title={t("proxyNodes.notFound")} />);
  return <RequireCapability check={p => p.can(id ? "UPDATE" : "CREATE", "PROXY_NODE")}>
    <NodeEditor key={id ?? "create"} node={node} />
  </RequireCapability>;
}
