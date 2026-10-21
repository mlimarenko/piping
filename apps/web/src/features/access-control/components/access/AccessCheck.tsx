import { useApiMutation } from "@/shared/api/hooks";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import FormField from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import { Button } from "@/shared/ui-primitives/button";
import { Loader2, Search, Shield, CheckCircle, XCircle, ChevronRight } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import { useAccessData } from "./AccessDataContext";
import { checkAccess } from "@/features/access-control/api";
import type { AccessCheckResponse } from "@/shared/api/types";
import type { ApiError } from "@/shared/api/client";

const AccessCheck = () => {
  const { t } = useTranslation();
  const { users, resourceTypes, allActions } = useAccessData();
  const [selectedUser, setSelectedUser] = useState("");
  const [selectedAction, setSelectedAction] = useState("");
  const [selectedResourceType, setSelectedResourceType] = useState("");
  const [result, setResult] = useState<AccessCheckResponse | null>(null);

  const checkMutation = useApiMutation({
    mutationFn: () =>
      checkAccess({
        user: selectedUser,
        action: selectedAction,
        resourceType: selectedResourceType || undefined,
      }),
    onSuccess: (response) => setResult(response),
    onError: (err: ApiError) => {
      notify("error", err.message || t("accessControl.accessCheck.evaluateFailed"));
      setResult(null);
    },
  });

  const evaluate = () => {
    if (!selectedUser || !selectedAction || !selectedResourceType) return;
    checkMutation.mutate();
  };

  const evaluated = checkMutation.isSuccess || checkMutation.isError;

  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <SurfaceCard className="overflow-hidden">
        <div className="flex items-center gap-2 px-4 py-2.5 border-b border-border">
          <Search className="size-4 text-primary" />
          <h2 className="text-sm font-medium text-foreground">{t("accessControl.accessCheck.title")}</h2>
        </div>
        <div className="p-4 space-y-3">
           <FormField
             htmlFor="access-check-subject"
            label={t("accessControl.accessCheck.subjectLabel")}
            required
            labelClassName="text-xs text-muted-foreground"
          >
             <EntitySelect
               id="access-check-subject"
              items={users.map(u => ({ id: u.id, label: `${u.username} (${u.email})` }))}
              value={selectedUser}
              onValueChange={setSelectedUser}
              placeholder={t("accessControl.accessCheck.subjectPlaceholder")}
              triggerClassName="h-9 text-sm"
            />
          </FormField>
          <FormSelect
            label={t("accessControl.accessCheck.actionLabel")}
            required
            value={selectedAction}
            onValueChange={setSelectedAction}
            options={allActions.map((action) => ({
              value: action,
              label: t(`accessControl.policies.actionLabels.${action.toLowerCase()}`, {
                defaultValue: action,
              }),
            }))}
            placeholder={t("accessControl.accessCheck.actionPlaceholder")}
            triggerClassName="h-9 text-sm"
            labelClassName="text-xs text-muted-foreground"
          />
          <FormSelect
            label={t("accessControl.policies.resource")}
            required
            value={selectedResourceType}
            onValueChange={setSelectedResourceType}
            options={resourceTypes.map((resourceType) => ({
              value: resourceType,
              label: t(`accessControl.policies.resourceTypes.${resourceType}`, {
                defaultValue: resourceType,
              }),
            }))}
            placeholder={t("accessControl.policies.resource")}
            triggerClassName="h-9 text-sm"
            labelClassName="text-xs text-muted-foreground"
          />
          <Button
            className="w-full"
            onClick={evaluate}
            disabled={!selectedUser || !selectedAction || !selectedResourceType || checkMutation.isPending}
          >
            {checkMutation.isPending ? (
              <Loader2 className="animate-spin" />
            ) : (
              <>
                <Shield /> {t("accessControl.accessCheck.evaluate")}
              </>
            )}
          </Button>
        </div>
      </SurfaceCard>

      <SurfaceCard className="overflow-hidden">
        <div className="flex items-center gap-2 px-4 py-2.5 border-b border-border">
          <Shield className="size-4 text-primary" />
          <h2 className="text-sm font-medium text-foreground">{t("accessControl.accessCheck.results")}</h2>
        </div>
        <div className="p-4">
          {!evaluated || !result ? (
            <div className="flex w-full flex-col items-center justify-center py-12 text-center">
              <Shield className="mb-4 h-8 w-8 text-muted-foreground" />
              <p className="mx-auto max-w-xs text-xs text-muted-foreground text-balance">{t("accessControl.accessCheck.noResult")}</p>
            </div>
          ) : (
            <div className="space-y-4 animate-fade-in">
              <div className={cn("flex items-center gap-3 p-3 rounded-lg border",
                result.decision === "allow" ? "bg-status-positive/5 border-status-positive/15" : "bg-status-negative/5 border-status-negative/15"
              )}>
                {result.decision === "allow" ? <CheckCircle className="h-5 w-5 text-status-positive" /> : <XCircle className="h-5 w-5 text-status-negative" />}
                <div>
                  <p className={cn("text-sm font-semibold", result.decision === "allow" ? "text-status-positive" : "text-status-negative")}>
                    {result.decision === "allow" ? t("accessControl.accessCheck.allowed").toUpperCase() : t("accessControl.accessCheck.denied").toUpperCase()}
                  </p>
                  <p className="text-xs text-muted-foreground">
                    {result.matchedPolicies.length > 0
                      ? t("accessControl.accessCheck.matchingRules") + `: ${result.matchedPolicies.length}`
                      : t("accessControl.policies.noRules")}
                  </p>
                </div>
              </div>

              {result.matchedPolicies.length > 0 && (
                <div>
                  <h4 className="text-xs font-medium text-muted-foreground mb-2 uppercase tracking-wider">{t("accessControl.accessCheck.matchingRules")}</h4>
                  <div className="space-y-1">
                    {result.matchedPolicies.map(p => (
                      <div key={p} className="flex items-center gap-2 px-3 py-1.5 rounded-md surface-inset">
                        <CheckCircle className="size-4 text-status-positive" />
                        <span className="text-xs text-foreground">{p}</span>
                        <ChevronRight className="size-4 text-muted-foreground ml-auto" />
                      </div>
                    ))}
                  </div>
                </div>
              )}

              <div className="grid grid-cols-2 gap-3">
                <div>
                  <h4 className="text-xs font-medium text-muted-foreground mb-2 uppercase tracking-wider">{t("accessControl.tabs.roles")}</h4>
                  <div className="flex flex-wrap gap-1">
                    {result.viaRoles.length > 0
                      ? result.viaRoles.map(r => <span key={r} className="surface-chip px-2 py-0.5 rounded text-xs">{r}</span>)
                      : <span className="text-xs text-muted-foreground italic">—</span>}
                  </div>
                </div>
                <div>
                  <h4 className="text-xs font-medium text-muted-foreground mb-2 uppercase tracking-wider">{t("accessControl.tabs.groups")}</h4>
                  <div className="flex flex-wrap gap-1">
                    {result.viaGroups.length > 0
                      ? result.viaGroups.map(g => <span key={g} className="surface-chip px-2 py-0.5 rounded text-xs">{g}</span>)
                      : <span className="text-xs text-muted-foreground italic">—</span>}
                  </div>
                </div>
              </div>
            </div>
          )}
        </div>
      </SurfaceCard>
    </div>
  );
};

export default AccessCheck;
