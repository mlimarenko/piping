import { useState } from "react";
import { useNavigate } from "react-router";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { usePermissions } from "@/shared/auth/permissionsContext";

import {
  createPlaybookServicePrincipal,
  type CreatePlaybookServicePrincipalRequest,
} from "@/features/playbooks/api";
import { ServicePrincipalEditor } from "@/features/playbooks/ServicePrincipalEditor";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";

/**
 * Routed create page for a playbook service principal. Replaces the in-memory
 * full-page swap with a deep-linkable URL.
 */
export default function ServicePrincipalEditorPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const { can } = usePermissions();

  const [name, setName] = useState("");

  const back = () => {
    void navigate(PLAYBOOK_TAB_ROUTES["service-principals"]);
  };

  const createMutation = useApiMutation({
    mutationFn: (body: CreatePlaybookServicePrincipalRequest) =>
      createPlaybookServicePrincipal(body),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: ["playbooks", "service-principals"],
      });
      back();
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const submit = () => {
    if (!name.trim()) {
      notify("error", t("playbooks.page.errors.servicePrincipalNameRequired"));
      return;
    }
    createMutation.mutate({ name: name.trim() });
  };

  return (
    <ServicePrincipalEditor
      name={name}
      onNameChange={setName}
      canReadPolicy={can("READ", "POLICY")}
      onGrantAccess={() => {
        void navigate("/access-control?tab=access");
      }}
      onSubmit={submit}
      submitting={createMutation.isPending}
      onBack={back}
      dirty={name.trim() !== ""}
    />
  );
}
