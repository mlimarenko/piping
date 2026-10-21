import { useRef, useState, type ReactNode } from "react";
import { useLocation, useNavigate, useParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Key, Loader2 } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import EmptyState from "@/shared/ui-shared/EmptyState";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import FormSection from "@/shared/ui-shared/FormSection";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { Input } from "@/shared/ui-primitives/input";
import { Label } from "@/shared/ui-primitives/label";
import { notify } from "@/shared/lib/notifications";
import {
  createMyCredential,
  listMyCredentials,
  updateMyCredential,
} from "@/features/user-settings/api";
import type { CredentialProtocol, PersonalCredential } from "@/shared/api/types";
import { userSettingsSectionHref } from "./userSettingsRoutes";

const PROTOCOLS: CredentialProtocol[] = ["SSH", "RDP", "VNC"];

interface CredentialForm {
  name: string;
  protocol: CredentialProtocol;
  username: string;
  domain: string;
  password: string;
  privateKey: string;
  isDefault: boolean;
}

const emptyCredentialForm: CredentialForm = {
  name: "",
  protocol: "SSH",
  username: "",
  domain: "",
  password: "",
  privateKey: "",
  isDefault: false,
};

function formFromCredential(credential: PersonalCredential): CredentialForm {
  return {
    name: credential.name,
    protocol: credential.protocol,
    username: credential.username ?? "",
    domain: credential.domain ?? "",
    password: "",
    privateKey: "",
    isDefault: credential.isDefault,
  };
}

function CredentialEditor({ credential }: Readonly<{ credential?: PersonalCredential }>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const location = useLocation();
  const nameInput = useRef<HTMLInputElement>(null);
  const [submitAttempted, setSubmitAttempted] = useState(false);
  const [baseline] = useState<CredentialForm>(() => credential ? formFromCredential(credential) : emptyCredentialForm);
  const [form, setForm] = useState(baseline);
  const back = () => { void navigate(userSettingsSectionHref("credentials", location)); };

  const invalidate = () => qc.invalidateQueries({ queryKey: ["user", "credentials"] });

  const createMutation = useApiMutation({
    mutationFn: (body: Omit<PersonalCredential, "id" | "userId">) => createMyCredential(body),
    onSuccess: () => {
      void invalidate();
      notify("success", t("userSettings.credentialSaved"));
      back();
    },
  });

  const updateMutation = useApiMutation({
    mutationFn: (body: Pick<PersonalCredential, "name" | "protocol" | "username" | "domain" | "password" | "privateKey">) =>
      updateMyCredential(credential!.id, body),
    onSuccess: () => {
      void invalidate();
      notify("success", t("userSettings.credentialUpdated"));
      back();
    },
  });

  const handleSave = () => {
    if (createMutation.isPending || updateMutation.isPending) return;
    setSubmitAttempted(true);
    if (!form.name.trim()) {
      nameInput.current?.focus();
      return;
    }
    const payload = {
      name: form.name.trim(),
      protocol: form.protocol,
      isDefault: form.isDefault,
      username: form.username.trim() || undefined,
      domain: form.protocol === "RDP" ? form.domain.trim() || undefined : undefined,
      password: form.password.trim() || undefined,
      privateKey: form.privateKey.trim() || undefined,
    };
    if (credential) {
      updateMutation.mutate({
        name: payload.name,
        protocol: payload.protocol,
        username: payload.username,
        domain: payload.domain,
        password: payload.password,
        privateKey: payload.privateKey,
      });
    } else {
      createMutation.mutate(payload);
    }
  };

  return (
    <EntityEditorPage scope="routed"
      title={t(credential ? "userSettings.editCredential" : "userSettings.addCredential")}
      description={t("userSettings.credentialWriteOnly")}
      onBack={back}
      backLabel={t("back")}
      onCancel={back}
      cancelLabel={t("cancel")}
      onSave={handleSave}
      saveLabel={t(credential ? "save" : "create")}
      saving={createMutation.isPending || updateMutation.isPending}
      dirty={JSON.stringify(form) !== JSON.stringify(baseline)}
      error={createMutation.isError || updateMutation.isError ? <p role="alert">{t(credential ? "userSettings.updateFailed" : "userSettings.saveFailed")}</p> : undefined}
    >
      <FormSection contentClassName="flex flex-col gap-4">
        <div className="grid gap-4 sm:grid-cols-2">
          <FormSelect<CredentialProtocol>
            value={form.protocol}
            onValueChange={(protocol) => setForm({ ...form, protocol })}
            disabled={!!credential}
            label={t("userSettings.protocol")}
            options={PROTOCOLS.map((protocol) => ({ value: protocol, label: protocol }))}
          />
          <FormField htmlFor="credential-name" label={t("name")} required
            error={submitAttempted && !form.name.trim() ? t("userSettings.nameRequired") : undefined}>
            <FormFieldControl>
            <Input
              id="credential-name"
              ref={nameInput}
              aria-required="true"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
            />
            </FormFieldControl>
          </FormField>
        </div>
        {form.protocol !== "VNC" && (
          <div className={form.protocol === "RDP" ? "grid gap-4 sm:grid-cols-2" : "flex flex-col gap-2"}>
            <FormField htmlFor="credential-username" label={t("userSettings.username")}>
              <FormFieldControl>
              <Input
                id="credential-username"
                value={form.username}
                onChange={(e) => setForm({ ...form, username: e.target.value })}
              />
              </FormFieldControl>
            </FormField>
            {form.protocol === "RDP" && (
              <FormField htmlFor="credential-domain" label={t("userSettings.domain")}>
                <FormFieldControl>
                <Input
                  id="credential-domain"
                  placeholder={t("userSettings.domainPlaceholder")}
                  value={form.domain}
                  onChange={(e) => setForm({ ...form, domain: e.target.value })}
                />
                </FormFieldControl>
              </FormField>
            )}
          </div>
        )}
        <FormField htmlFor="credential-password" label={t("userSettings.password")}>
          <FormFieldControl>
          <Input
            id="credential-password"
            type="password"
            placeholder={credential ? t("userSettings.credentialUnchanged") : "••••••"}
            value={form.password}
            onChange={(e) => setForm({ ...form, password: e.target.value })}
          />
          </FormFieldControl>
        </FormField>
        {form.protocol === "SSH" && (
          <FormField htmlFor="credential-private-key" label={t("userSettings.privateKeyOptional")}>
            <FormFieldControl>
            <Input
              id="credential-private-key"
              type="password"
              placeholder={credential ? t("userSettings.credentialUnchanged") : t("userSettings.credentialPrivateKeyPlaceholder")}
              value={form.privateKey}
              onChange={(e) => setForm({ ...form, privateKey: e.target.value })}
            />
            </FormFieldControl>
          </FormField>
        )}
        {!credential && (
          <div className="flex items-center gap-2 pt-1">
            <input
              id="cred-default"
              type="checkbox"
              checked={form.isDefault}
              onChange={(e) => setForm({ ...form, isDefault: e.target.checked })}
            />
            <Label htmlFor="cred-default" className="text-sm">
              {t("userSettings.markDefault")}
            </Label>
          </div>
        )}
      </FormSection>
    </EntityEditorPage>
  );
}

export default function UserCredentialEditor() {
  const { id } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const query = useQuery({
    queryKey: ["user", "credentials", "options"],
    queryFn: () => listMyCredentials(),
  });
  const back = () => { void navigate(userSettingsSectionHref("credentials", location)); };
  const renderState = (content: ReactNode) => <EntityEditorPage scope="routed"
    title={t(id ? "userSettings.editCredential" : "userSettings.addCredential")}
    description={t("userSettings.credentialWriteOnly")} onBack={back} backLabel={t("back")}
    onCancel={back} cancelLabel={t("cancel")}>
    {content}
  </EntityEditorPage>;
  if (query.isPending) return renderState(<output className="flex justify-center p-8"><Loader2 className="size-6 animate-spin" aria-label={t("loading")} /></output>);
  if (query.isError) return renderState(<EmptyState icon={Key} title={t("userSettings.loadFailed")} actionLabel={t("retry")} onAction={() => { void query.refetch(); }} />);
  const credential = query.data.find(candidate => candidate.id === id);
  if (id && !credential) {
    return renderState(<EmptyState icon={Key} title={t("userSettings.credentialNotFound")} />);
  }
  return <CredentialEditor key={id ?? "create"} credential={credential} />;
}
