import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import SurfaceCard, { SurfaceCardHeader, SurfaceCardContent } from "@/shared/ui-shared/SurfaceCard";
import PageState from "@/shared/ui-shared/PageState";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Switch } from "@/shared/ui-primitives/switch";
import { Loader2, Play, RefreshCw, Save } from "lucide-react";
import { notify } from "@/shared/lib/notifications";
import { getSsoConfig, testSsoConnection, updateSsoConfig } from "@/features/access-control/api";
import type { ApiError } from "@/shared/api/client";
import type { SsoConfig } from "@/shared/api/types";

const emptyConfig: SsoConfig = {
  enabled: false,
  issuerUrl: "",
  clientId: "",
  clientSecret: "",
  redirectUrl: "",
  scopes: "openid profile email groups",
  groupClaim: "groups",
};

interface SsoConfigEditorProps {
  initial: SsoConfig;
  saving: boolean;
  testing: boolean;
  refreshing: boolean;
  canSave: boolean;
  onSave: (payload: Partial<SsoConfig>) => void;
  onTest: () => void;
  onRefresh: () => void;
}

/**
 * Editable SSO config form. The draft is seeded from the fetched config via the
 * `initial` prop and lives in local state. The parent remounts this component
 * (via a `key` keyed on the query's `dataUpdatedAt`) whenever fresh server data
 * lands, which re-seeds the draft and clears the secret-dirty flag — the same
 * behavior the previous useEffect-based sync produced, without setState-in-effect.
 */
const SsoConfigEditor = ({
  initial,
  saving,
  testing,
  refreshing,
  canSave,
  onSave,
  onTest,
  onRefresh,
}: SsoConfigEditorProps) => {
  const { t } = useTranslation();
  const fieldId = useId();
  const [form, setForm] = useState<SsoConfig>(() => ({ ...initial, clientSecret: "" }));
  const [secretDirty, setSecretDirty] = useState(false);

  const setField = <K extends keyof SsoConfig>(key: K, value: SsoConfig[K]) => {
    setForm((prev) => ({ ...prev, [key]: value }));
  };

  const handleSave = () => {
    const payload: Partial<SsoConfig> = {
      enabled: form.enabled,
      issuerUrl: form.issuerUrl,
      clientId: form.clientId,
      redirectUrl: form.redirectUrl,
      scopes: form.scopes,
      groupClaim: form.groupClaim,
      // Carry the non-UI fields through unchanged so a save never resets them.
      provisioningMode: form.provisioningMode,
      clientSecretEnabled: form.clientSecretEnabled,
    };
    if (secretDirty) payload.clientSecret = form.clientSecret;
    onSave(payload);
  };

  return (
    <>
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="sm:col-span-2 flex items-center justify-between">
          <label htmlFor={`${fieldId}-enabled`} className="text-sm font-medium">{t("accessControl.sso.enabled")}</label>
          <Switch
            id={`${fieldId}-enabled`}
            checked={form.enabled}
            onCheckedChange={(v) => setField("enabled", v)}
          />
        </div>
        <div>
          <label htmlFor={`${fieldId}-issuer`} className="text-sm font-medium mb-2 block">
            {t("accessControl.sso.issuerUrl")}
          </label>
          <Input
            id={`${fieldId}-issuer`}
            placeholder="https://…"
            value={form.issuerUrl}
            onChange={(e) => setField("issuerUrl", e.target.value)}
          />
        </div>
        <div>
          <label htmlFor={`${fieldId}-client`} className="text-sm font-medium mb-2 block">
            {t("accessControl.sso.clientId")}
          </label>
          <Input
            id={`${fieldId}-client`}
            value={form.clientId}
            onChange={(e) => setField("clientId", e.target.value)}
          />
        </div>
        <div>
          <label htmlFor={`${fieldId}-secret`} className="text-sm font-medium mb-2 block">
            {t("accessControl.sso.clientSecret")}
          </label>
          <Input
            id={`${fieldId}-secret`}
            type="password"
            placeholder={form.enabled ? t("userSettings.credentialUnchanged") : "••••••"}
            value={form.clientSecret ?? ""}
            onChange={(e) => {
              setField("clientSecret", e.target.value);
              setSecretDirty(true);
            }}
          />
        </div>
        <div>
          <label htmlFor={`${fieldId}-redirect`} className="text-sm font-medium mb-2 block">
            {t("accessControl.sso.redirectUri")}
          </label>
          <Input
            id={`${fieldId}-redirect`}
            readOnly
            value={form.redirectUrl ?? ""}
          />
        </div>
        <div>
          <label htmlFor={`${fieldId}-scopes`} className="text-sm font-medium mb-2 block">
            {t("accessControl.sso.scopes")}
          </label>
          <Input
            id={`${fieldId}-scopes`}
            value={form.scopes}
            onChange={(e) => setField("scopes", e.target.value)}
          />
        </div>
        <div>
          <label htmlFor={`${fieldId}-claim`} className="text-sm font-medium mb-2 block">
            {t("accessControl.users.authSources.oidc")}
          </label>
          <Input
            id={`${fieldId}-claim`}
            value={form.groupClaim}
            onChange={(e) => setField("groupClaim", e.target.value)}
          />
        </div>
      </div>
      <div className="flex flex-wrap gap-2 mt-4">
        {canSave && (
          <Button
            className="gap-1"
            onClick={handleSave}
            disabled={saving}
          >
            {saving ? (
              <Loader2 className="animate-spin" />
            ) : (
              <>
                <Save /> {t("save")}
              </>
            )}
          </Button>
        )}
        <Button
          variant="outline"
          className="gap-1"
          onClick={onTest}
          disabled={testing}
        >
          {testing ? (
            <Loader2 className="animate-spin" />
          ) : (
            <>
              <Play /> {t("inventory.sync.testConnection")}
            </>
          )}
        </Button>
        <Button
          variant="outline"
          className="gap-1"
          onClick={onRefresh}
          disabled={refreshing}
        >
          <RefreshCw className={refreshing ? "animate-spin" : undefined} />{" "}
          {t("refresh")}
        </Button>
      </div>
    </>
  );
};

const AccessSSO = () => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  // View-but-not-edit: a READ-only KEYCLOAK role sees the config (and can run the
  // read-only test connection) but the Save affordance is hidden, not 403-on-click.
  const { can } = usePermissions();
  const canSave = can("UPDATE", "KEYCLOAK");
  const ssoQuery = useQuery({
    queryKey: ["access", "sso"],
    queryFn: () => getSsoConfig(),
  });

  const saveMutation = useApiMutation({
    mutationFn: (body: Partial<SsoConfig>) => updateSsoConfig(body),
    onSuccess: () => {
      notify("success", t("inventory.sync.configSaved"));
      void qc.invalidateQueries({ queryKey: ["access", "sso"] });
    },
    onError: (err: ApiError) => notify("error", err.message || t("inventory.sync.saveFailed")),
  });

  const testMutation = useApiMutation({
    mutationFn: () => testSsoConnection(),
    onSuccess: (result) => {
      if (result.ok) notify("success", t("inventory.sync.connectionSuccessful"));
      else notify("error", result.message || t("inventory.sync.connectionFailed"));
    },
    onError: (err: ApiError) => notify("error", err.message || t("inventory.sync.validationFailed")),
  });

  return (
    <SurfaceCard>
      <SurfaceCardHeader title={t("accessControl.sso.title")} />
      <SurfaceCardContent>
      {ssoQuery.isPending ? (
        <PageState kind="loading" title={t("loading")} />
      ) : (
        <SsoConfigEditor
          // Remount (re-seed the draft) only when the fetched config content
          // actually changes — mirrors the prior `useEffect(..., [ssoQuery.data])`
          // which re-seeded on data-reference change (TanStack structural sharing
          // keeps the reference stable when a refetch returns identical data).
          key={JSON.stringify(ssoQuery.data ?? emptyConfig)}
          initial={ssoQuery.data ?? emptyConfig}
          saving={saveMutation.isPending}
          testing={testMutation.isPending}
          refreshing={ssoQuery.isFetching}
          canSave={canSave}
          onSave={(payload) => saveMutation.mutate(payload)}
          onTest={() => testMutation.mutate()}
          onRefresh={() => ssoQuery.refetch()}
        />
      )}
      </SurfaceCardContent>
    </SurfaceCard>
  );
};

export default AccessSSO;
