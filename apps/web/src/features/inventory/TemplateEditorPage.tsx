import { useTranslation } from "react-i18next";
import { useNavigate, useParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, AlertTriangle } from "lucide-react";

import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { notify } from "@/shared/lib/notifications";
import { Button } from "@/shared/ui-primitives/button";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import type { ApiError } from "@/shared/api/client";
import type { InventoryTemplate } from "@/features/inventory/components/inventory/templateTypes";
import {
  getTemplate as apiGetTemplate,
  createTemplate as apiCreateTemplate,
  updateTemplate as apiUpdateTemplate,
} from "@/features/inventory/api";
import TemplateEditor from "@/features/inventory/components/inventory/TemplateEditor";

const TEMPLATES_QUERY_KEY = ["inventory", "templates"] as const;

/**
 * Route host for the full-page template authoring editor.
 * `/inventory/templates/new`        → create
 * `/inventory/templates/:id/edit`   → edit (fetches the template first)
 */
const TemplateEditorPage = () => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { id } = useParams<{ id?: string }>();
  const isEdit = !!id;

  // View-but-not-edit: gate the save action by capability (CREATE for a new
  // template, UPDATE for an existing one) so a READ-only viewer can't dispatch a
  // mutation that would only 403 server-side.
  const { can } = usePermissions();
  const canSave = isEdit ? can("UPDATE", "INVENTORY") : can("CREATE", "INVENTORY");

  const templateQuery = useQuery({
    queryKey: ["inventory", "template", id],
    queryFn: () => apiGetTemplate(id!),
    enabled: isEdit,
  });

  const goBack = () => { void navigate("/inventory/templates"); };

  const persist = (saved: InventoryTemplate) => {
    void qc.invalidateQueries({ queryKey: TEMPLATES_QUERY_KEY });
    void qc.invalidateQueries({ queryKey: ["inventory", "template", saved.id] });
    notify("success", t("inventory.templates.templateSaved", { name: saved.name }));
    goBack();
  };

  const onError = (err: ApiError) => notify("error", err.message || t("unknownError"));

  const createMutation = useApiMutation({
    mutationFn: (body: Omit<InventoryTemplate, "id" | "updatedAt">) => apiCreateTemplate(body),
    onSuccess: persist,
    onError,
  });

  const updateMutation = useApiMutation({
    mutationFn: ({ tid, body }: { tid: string; body: Partial<InventoryTemplate> }) =>
      apiUpdateTemplate(tid, body),
    onSuccess: persist,
    onError,
  });

  const saving = createMutation.isPending || updateMutation.isPending;

  const handleSave = (tpl: InventoryTemplate) => {
    if (!canSave) return;
    const payload: Omit<InventoryTemplate, "id" | "updatedAt"> = {
      name: tpl.name,
      slug: tpl.slug,
      status: tpl.status,
      description: tpl.description,
      variables: tpl.variables,
      steps: tpl.steps,
    };
    if (isEdit && id) {
      updateMutation.mutate({ tid: id, body: payload });
    } else {
      createMutation.mutate(payload);
    }
  };

  if (isEdit && (templateQuery.isPending || templateQuery.isError || !templateQuery.data)) {
    return (
      <EntityEditorPage scope="routed" width="standard" title={t("inventory.templates.builder.editTitle")}
        onBack={goBack} backLabel={t("inventory.templates.editor.backToList")} cancelLabel={t("cancel")}>
        {templateQuery.isPending ? (
          <output className="flex items-center gap-2"><Loader2 aria-hidden className="size-6 animate-spin text-muted-foreground" />{t("loading")}</output>
        ) : (
          <div className="flex flex-col items-start gap-4">
            <AlertTriangle aria-hidden className="size-8 text-status-negative" />
            <p role="alert" className="text-sm text-foreground">{t("inventory.templates.editor.loadFailed")}</p>
            <Button variant="outline" onClick={() => void templateQuery.refetch()}>{t("retry")}</Button>
          </div>
        )}
      </EntityEditorPage>
    );
  }

  return (
    <TemplateEditor
      initial={isEdit ? (templateQuery.data ?? null) : null}
      saving={saving}
      canSave={canSave}
      onSave={handleSave}
      onCancel={goBack}
    />
  );
};

export default TemplateEditorPage;
