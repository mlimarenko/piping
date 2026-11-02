import { useMemo, useState, type ReactNode } from "react";
import { useNavigate, useParams } from "react-router";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Route } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { RequireCapability } from "@/shared/auth/permissions";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import EmptyState from "@/shared/ui-shared/EmptyState";
import PageState from "@/shared/ui-shared/PageState";
import { Button } from "@/shared/ui-primitives/button";
import FormField from "@/shared/ui-shared/FormField";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import { Input } from "@/shared/ui-primitives/input";
import { Label } from "@/shared/ui-primitives/label";
import { Switch } from "@/shared/ui-primitives/switch";
import { notify } from "@/shared/lib/notifications";
import { listResources } from "@/features/inventory/api";
import {
  CONNCONF_SCOPE_TYPES,
  type ConnConfScopeType,
} from "@/features/connections/connection-config/api";
import { SCOPE_ENTITY, SCOPE_I18N_KEY } from "@/features/connections/connection-config/scopeCatalog";
import {
  createProxyNodeAssignment,
  listProxyNodeAssignments,
  listProxyNodes,
  updateProxyNodeAssignment,
} from "@/features/connections/proxies-api";
import type { ProxyNode, ProxyNodeAssignment } from "@/shared/api/types";
import type { components } from "@/shared/api/schema.gen";
import type { ApiError } from "@/shared/api/client";
import { PROXY_NODE_NEW_ROUTE, PROXY_NODES_ROUTE } from "./proxyRecordRoutes";

type UpdateProxyNodeAssignmentRequest = components["schemas"]["UpdateProxyNodeAssignmentRequest"];

interface AssignmentFormData {
  proxyNodeId: string;
  scopeType: ConnConfScopeType;
  scopeId: string;
  weight: string;
  enabled: boolean;
}
interface AssignmentFormErrors { proxyNodeId?: string; scopeId?: string; weight?: string; }

const emptyAssignmentForm: AssignmentFormData = {
  proxyNodeId: "",
  scopeType: "region",
  scopeId: "",
  weight: "0",
  enabled: true,
};

type ScopeOptions = Record<Exclude<ConnConfScopeType, "global">, { id: string; name: string }[]>;

function formFromAssignment(assignment: ProxyNodeAssignment): AssignmentFormData {
  return {
    proxyNodeId: assignment.proxy_node_id,
    scopeType: assignment.scope_type as ConnConfScopeType,
    scopeId: assignment.scope_id ?? "",
    weight: String(assignment.weight),
    enabled: assignment.enabled,
  };
}

function AssignmentEditor({
  assignment,
  proxyOptions,
  optionsByScope,
}: Readonly<{
  assignment?: ProxyNodeAssignment;
  proxyOptions: ProxyNode[];
  optionsByScope: ScopeOptions;
}>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [baseline] = useState<AssignmentFormData>(
    () => assignment ? formFromAssignment(assignment) : { ...emptyAssignmentForm, proxyNodeId: proxyOptions[0]?.id ?? "" },
  );
  const [form, setForm] = useState(baseline);
  const [errors, setErrors] = useState<AssignmentFormErrors>({});
  const back = () => { void navigate(PROXY_NODES_ROUTE); };

  const validateAssignment = (f: AssignmentFormData): AssignmentFormErrors => {
    const errs: AssignmentFormErrors = {};
    if (!f.proxyNodeId) errs.proxyNodeId = t("proxyNodes.errors.proxyNodeRequired");
    if (f.scopeType !== "global" && !f.scopeId) {
      errs.scopeId = t("proxyNodes.errors.scopeRequired");
    }
    if (!Number.isFinite(Number(f.weight))) {
      errs.weight = t("proxyNodes.errors.weightInvalid");
    }
    return errs;
  };

  const invalidate = () => qc.invalidateQueries({ queryKey: ["proxy-node-assignments"] });

  const createMutation = useApiMutation({
    mutationFn: createProxyNodeAssignment,
    onSuccess: () => {
      void invalidate();
      notify("success", t("proxyNodes.assignmentCreated"));
      back();
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const updateMutation = useApiMutation({
    mutationFn: (body: UpdateProxyNodeAssignmentRequest) => updateProxyNodeAssignment(assignment!.id, body),
    onSuccess: () => {
      void invalidate();
      notify("success", t("proxyNodes.assignmentUpdated"));
      back();
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const setField = (key: keyof AssignmentFormData, value: string | boolean) => {
    if (createMutation.isPending || updateMutation.isPending) return;
    setForm(prev => ({ ...prev, [key]: value }));
    if (errors[key as keyof AssignmentFormErrors]) {
      setErrors(prev => ({ ...prev, [key]: undefined }));
    }
  };

  const handleSave = () => {
    const errs = validateAssignment(form);
    setErrors(errs);
    if (Object.keys(errs).length > 0) return;
    const payload = {
      proxy_node_id: form.proxyNodeId,
      scope_type: form.scopeType,
      scope_id: form.scopeType === "global" ? null : form.scopeId,
      weight: Number(form.weight),
      enabled: form.enabled,
    };
    if (assignment) updateMutation.mutate(payload);
    else createMutation.mutate(payload);
  };

  return (
    <EntityEditorPage scope="routed"
      title={t(assignment ? "proxyNodes.editAssignment" : "proxyNodes.addAssignmentTitle")}
      description={t("proxyNodes.assignmentDialogDescription")}
      onBack={back}
      backLabel={t("proxyNodes.backToList")}
      onCancel={back}
      cancelLabel={t("cancel")}
      onSave={handleSave}
      saveLabel={t(assignment ? "saveChanges" : "proxyNodes.addAssignment")}
      saving={createMutation.isPending || updateMutation.isPending}
      dirty={JSON.stringify(form) !== JSON.stringify(baseline)}
    >
      <fieldset disabled={createMutation.isPending || updateMutation.isPending} inert={createMutation.isPending || updateMutation.isPending} className="flex min-w-0 flex-col gap-4">
        <FormField htmlFor="proxy-assignment-node" label={t("proxyNodes.assignmentProxy")} required error={errors.proxyNodeId}>
          <SearchableSelect
            id="proxy-assignment-node"
            aria-label={t("proxyNodes.assignmentProxy")}
            value={form.proxyNodeId}
            onValueChange={(value) => setField("proxyNodeId", value)}
            placeholder={t("proxyNodes.pickProxyNode")}
            searchPlaceholder={t("search")}
            items={proxyOptions.map((node) => ({
              id: node.id,
              label: node.name,
              searchText: `${node.name} ${node.slug ?? ""} ${node.base_url ?? ""}`,
            }))}
          />
        </FormField>
        <div>
          <FormSelect<ConnConfScopeType>
            label={t("proxyNodes.assignmentScope")}
            required
            value={form.scopeType}
            onValueChange={(value) => {
              if (createMutation.isPending || updateMutation.isPending) return;
              setForm(prev => ({ ...prev, scopeType: value, scopeId: "" }));
              setErrors(prev => ({ ...prev, scopeId: undefined }));
            }}
            options={CONNCONF_SCOPE_TYPES.map((scope) => ({
              value: scope,
              label: t(`connectionConfig.scope.${SCOPE_I18N_KEY[scope]}`),
            }))}
          />
        </div>
        <FormField htmlFor="proxy-assignment-target" label={t("proxyNodes.assignmentTarget")} required={form.scopeType !== "global"} error={errors.scopeId}>
          {form.scopeType === "global" ? (
            <Input
              id="proxy-assignment-target"
              value={t("connectionConfig.scope.appliesAll")}
              disabled
            />
          ) : (
            <EntitySelect
              id="proxy-assignment-target"
              aria-label={t("proxyNodes.assignmentTarget")}
              value={form.scopeId}
              onValueChange={(value) => setField("scopeId", value)}
              triggerClassName="text-sm"
              placeholder={t("proxyNodes.pickScopeTarget")}
              items={(optionsByScope[form.scopeType] ?? []).map((option) => ({
                id: option.id,
                label: option.name,
              }))}
            />
          )}
        </FormField>
        <div className="flex items-end gap-4">
          <FormField className="w-28" htmlFor="proxy-assignment-weight" label={t("proxyNodes.assignmentWeight")} error={errors.weight}>
            <Input
              id="proxy-assignment-weight"
              type="number"
              value={form.weight}
              onChange={e => setField("weight", e.target.value)}
              className="font-mono text-[11px]"
            />
          </FormField>
          <div className="flex items-center gap-2 pb-2">
            <Switch checked={form.enabled} onCheckedChange={v => setField("enabled", v)} id="assignment-enabled" />
            <Label htmlFor="assignment-enabled">{t("enabled")}</Label>
          </div>
        </div>
      </fieldset>
    </EntityEditorPage>
  );
}

export default function ProxyAssignmentEditor() {
  const { id } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const assignmentsQuery = useQuery({
    queryKey: ["proxy-node-assignments", "options"],
    queryFn: () => listProxyNodeAssignments({ limit: 1000, offset: 0 }),
  });
  const proxyOptionsQuery = useQuery({
    queryKey: ["proxy-nodes", "options"],
    queryFn: () => listProxyNodes({ limit: 1000, offset: 0 }),
    staleTime: 60_000,
  });
  const targetableScopes = useMemo(
    () => CONNCONF_SCOPE_TYPES.filter((s) => s !== "global"),
    [],
  );
  const scopeQueries = useQueries({
    queries: targetableScopes.map((scope) => ({
      queryKey: ["inventory", "proxy-assignment-scope", SCOPE_ENTITY[scope]],
      queryFn: () => listResources(SCOPE_ENTITY[scope], { limit: 1000, offset: 0 }),
      staleTime: 60_000,
    })),
  });
  const optionsByScope = useMemo<ScopeOptions>(() => {
    const map = {} as ScopeOptions;
    targetableScopes.forEach((scope, index) => {
      map[scope] = (scopeQueries[index]?.data?.items ?? []).map((r) => ({
        id: r.id,
        name: r.name,
      }));
    });
    return map;
  }, [targetableScopes, scopeQueries]);

  const renderEditorState = (content: ReactNode) => (
    <EntityEditorPage scope="routed"
      title={t(id ? "proxyNodes.editAssignment" : "proxyNodes.addAssignmentTitle")}
      description={t("proxyNodes.assignmentDialogDescription")}
      onBack={() => { void navigate(PROXY_NODES_ROUTE); }}
      backLabel={t("proxyNodes.backToList")}
      cancelLabel={t("cancel")}>
      {content}
    </EntityEditorPage>
  );

  if (assignmentsQuery.isPending || proxyOptionsQuery.isPending) {
    return renderEditorState(<PageState kind="loading" title={t("loading")} />);
  }
  if (assignmentsQuery.isError || proxyOptionsQuery.isError) {
    return renderEditorState(<PageState kind="error" title={t("proxyNodes.loadFailed")} action={<Button variant="outline" onClick={() => { void assignmentsQuery.refetch(); void proxyOptionsQuery.refetch(); }}>{t("retry")}</Button>} />);
  }
  const assignment = assignmentsQuery.data.items.find(candidate => candidate.id === id);
  if (id && !assignment) {
    return renderEditorState(<PageState kind="empty" title={t("proxyNodes.notFound")} />);
  }
  const proxyOptions = proxyOptionsQuery.data.items;
  if (!assignment && proxyOptions.length === 0) {
    return renderEditorState(<EmptyState icon={Route} title={t("proxyNodes.noAssignments")}
      description={t("proxyNodes.noAssignmentsNeedNode")}
      actionLabel={t("proxyNodes.addNode")} onAction={() => { void navigate(PROXY_NODE_NEW_ROUTE); }} />);
  }
  return <RequireCapability check={p => p.can(id ? "UPDATE" : "CREATE", "PROXY_NODE")}>
    <AssignmentEditor key={id ?? "create"} assignment={assignment} proxyOptions={proxyOptions} optionsByScope={optionsByScope} />
  </RequireCapability>;
}
