import { useRef, useState } from "react";
import { useTranslation, Trans } from "react-i18next";
import StatusChip from "@/shared/ui-shared/StatusChip";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, DialogDescription } from "@/shared/ui-primitives/dialog";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import { KeyRound, Users } from "lucide-react";
import { AccessEntityList } from "./AccessEntityList";
import { useAccessData, useAccessLabels, type UserRow } from "./AccessDataContext";
import EntityRef from "@/shared/ui-shared/EntityRef";
import { listUsers, USERS_QUERY_KEY } from "@/features/access-control/api";
import { USERS_TABLE_CONFIG } from "@/features/access-control/tableConfig";
import { usePermissions } from "@/shared/auth/permissionsContext";

interface FormData {
  username: string; email: string; status: "active" | "inactive" | "suspended"; authSource: "local" | "oidc"; externalId: string; password: string; confirmPassword: string;
}

const emptyForm: FormData = { username: "", email: "", status: "active", authSource: "local", externalId: "", password: "", confirmPassword: "" };

interface FormErrors { username?: string; email?: string; password?: string; confirmPassword?: string; }
interface PasswordFormErrors { password?: string; confirmPassword?: string; }

const AccessUsers = () => {
  const { t } = useTranslation();
  const { createUser, updateUser, changeUserPassword, deleteUser, grants } = useAccessData();
  const labels = useAccessLabels();
  // View-but-not-edit: hide create/edit/password-reset/delete affordances the
  // caller lacks. Password reset is a UPDATE on the user, so it tracks canUpdate.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "USER");
  const canUpdate = can("UPDATE", "USER");
  const canDelete = can("DELETE", "USER");

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingUser, setEditingUser] = useState<UserRow | null>(null);
  const [passwordTarget, setPasswordTarget] = useState<UserRow | null>(null);
  const [form, setForm] = useState<FormData>(emptyForm);
  const [errors, setErrors] = useState<FormErrors>({});
  const [passwordForm, setPasswordForm] = useState({ password: "", confirmPassword: "" });
  const [passwordErrors, setPasswordErrors] = useState<PasswordFormErrors>({});
  const [submitting, setSubmitting] = useState(false);
  const [passwordSubmitting, setPasswordSubmitting] = useState(false);
  const passwordInvoker = useRef<HTMLButtonElement | null>(null);

  const validate = (f: FormData, isNew: boolean): FormErrors => {
    const errs: FormErrors = {};
    if (!f.username.trim()) errs.username = t("accessControl.users.errors.usernameRequired");
    if (!f.email.trim()) errs.email = t("accessControl.users.errors.emailRequired");
    else if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(f.email.trim())) errs.email = t("accessControl.users.errors.emailInvalid");
    if (isNew && f.authSource === "local") {
      if (!f.password) errs.password = t("accessControl.users.errors.passwordRequired");
      else if (f.password.length < 8) errs.password = t("accessControl.users.errors.passwordTooShort");
      if (f.password !== f.confirmPassword) errs.confirmPassword = t("accessControl.users.errors.passwordsDontMatch");
    }
    return errs;
  };

  const openCreate = () => {
    setEditingUser(null);
    setForm(emptyForm);
    setErrors({});
    setDialogOpen(true);
  };
  const openEdit = (user: UserRow) => {
    setEditingUser(user);
    setForm({
      username: user.username,
      email: user.email,
      status: user.status,
      authSource: user.authSource,
      externalId: user.externalId,
      password: "",
      confirmPassword: "",
    });
    setErrors({});
    setDialogOpen(true);
  };
  const openPasswordReset = (user: UserRow, trigger: HTMLButtonElement | null) => {
    // Store the exact row-action trigger (focused by the shared renderer) so the
    // password dialog restores focus to it on dismissal.
    passwordInvoker.current = trigger;
    setPasswordTarget(user);
    setPasswordForm({ password: "", confirmPassword: "" });
    setPasswordErrors({});
  };

  const handleSave = async () => {
    const errs = validate(form, !editingUser);
    setErrors(errs);
    if (Object.keys(errs).length > 0) return;
    setSubmitting(true);
    try {
      if (editingUser) {
        await updateUser(editingUser.id, {
          username: form.username.trim(),
          email: form.email.trim(),
          status: form.status,
          authSource: form.authSource,
          externalId: form.externalId.trim(),
        });
      } else {
        await createUser({
          username: form.username.trim(),
          email: form.email.trim(),
          status: form.status,
          authSource: form.authSource,
          externalId: form.externalId.trim(),
          password: form.authSource === "local" ? form.password : undefined,
        });
      }
      setDialogOpen(false);
    } finally {
      setSubmitting(false);
    }
  };

  const handlePasswordSave = async () => {
    if (!passwordTarget) return;
    const nextErrors: PasswordFormErrors = {};
    if (!passwordForm.password) nextErrors.password = t("accessControl.users.errors.passwordRequired");
    else if (passwordForm.password.length < 8) nextErrors.password = t("accessControl.users.errors.passwordTooShort");
    if (passwordForm.password !== passwordForm.confirmPassword) {
      nextErrors.confirmPassword = t("accessControl.users.errors.passwordsDontMatch");
    }
    setPasswordErrors(nextErrors);
    if (Object.keys(nextErrors).length > 0) return;
    setPasswordSubmitting(true);
    try {
      await changeUserPassword(passwordTarget.id, passwordForm.password);
      setPasswordTarget(null);
      setPasswordForm({ password: "", confirmPassword: "" });
    } finally {
      setPasswordSubmitting(false);
    }
  };

  const setField = <K extends keyof FormData>(key: K, value: FormData[K]) => {
    setForm(prev => ({ ...prev, [key]: value }));
    if (errors[key as keyof FormErrors]) setErrors(prev => ({ ...prev, [key]: undefined }));
  };

  // Show user's roles from grants (from context full list)
  const getUserRoles = (username: string) => grants.filter(g => g.subjectType === "user" && g.subject === username).map(g => g.role);

  const editor = (
    <EntityEditorHost open={dialogOpen} onOpenChange={setDialogOpen}
      title={editingUser ? t("accessControl.users.editUser") : t("accessControl.users.createUser")}
      description={editingUser ? t("accessControl.users.updateDescription") : t("accessControl.users.createDescription")}
      footer={<>
        <Button data-editor-cancel variant="outline" onClick={() => setDialogOpen(false)} disabled={submitting}>{t("cancel")}</Button>
        <Button onClick={handleSave} disabled={submitting}>{editingUser ? t("saveChanges") : t("accessControl.users.createUser")}</Button>
      </>}>
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField htmlFor="access-user-username" label={t("accessControl.users.username")} error={errors.username} required>
          <FormFieldControl><Input id="access-user-username" aria-invalid={!!errors.username} value={form.username} onChange={e => setField("username", e.target.value)} placeholder={t("accessControl.users.usernamePlaceholder")} maxLength={100} /></FormFieldControl>
        </FormField>
        <FormField htmlFor="access-user-email" label={t("accessControl.users.email")} error={errors.email} required>
          <FormFieldControl><Input id="access-user-email" aria-invalid={!!errors.email} type="email" value={form.email} onChange={e => setField("email", e.target.value)} placeholder={t("accessControl.users.emailPlaceholder")} /></FormFieldControl>
        </FormField>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <FormSelect<FormData["status"]>
            label={t("accessControl.users.status")}
            value={form.status}
            onValueChange={(status) => setField("status", status)}
            options={[
              { value: "active", label: t("accessControl.users.statuses.active") },
              { value: "inactive", label: t("accessControl.users.statuses.inactive") },
              { value: "suspended", label: t("accessControl.users.statuses.suspended") },
            ]}
          />
          <FormSelect<FormData["authSource"]>
            label={t("accessControl.users.authSource")}
            value={form.authSource}
            onValueChange={(authSource) => setField("authSource", authSource)}
            options={[
              { value: "local", label: t("accessControl.users.authSources.local") },
              { value: "oidc", label: t("accessControl.users.authSources.oidc") },
            ]}
          />
        </div>
        {form.authSource === "oidc" && (
          <FormField htmlFor="access-user-external-id" label={t("accessControl.users.externalId")}>
            <Input id="access-user-external-id" value={form.externalId} onChange={e => setField("externalId", e.target.value)} placeholder={t("accessControl.users.externalIdPlaceholder")} />
          </FormField>
        )}
        {form.authSource === "local" && !editingUser && (
          <>
            <FormField htmlFor="access-user-password" label={t("accessControl.users.password")} error={errors.password} required>
              <FormFieldControl><Input id="access-user-password" aria-invalid={!!errors.password} type="password" value={form.password} onChange={e => setField("password", e.target.value)} placeholder={t("accessControl.users.passwordPlaceholder")} /></FormFieldControl>
            </FormField>
            <FormField htmlFor="access-user-confirm-password" label={t("accessControl.users.confirmPassword")} error={errors.confirmPassword} required>
              <FormFieldControl><Input id="access-user-confirm-password" aria-invalid={!!errors.confirmPassword} type="password" value={form.confirmPassword} onChange={e => setField("confirmPassword", e.target.value)} placeholder={t("accessControl.users.passwordPlaceholder")} /></FormFieldControl>
            </FormField>
          </>
        )}
      </div>
    </EntityEditorHost>
  );

  return (
    <>
      <AccessEntityList<UserRow>
        tableId="access.users"
        config={USERS_TABLE_CONFIG}
        queryKeyPrefix={USERS_QUERY_KEY}
        listFn={listUsers}
        getRowKey={r => r.id}
        rowLabel={r => r.username}
        searchPlaceholder={t("accessControl.users.searchPlaceholder")}
        emptyState={<EmptyState icon={Users} title={t("accessControl.users.noUsers")} description={t("accessControl.users.noUsersDescription")} />}
        columns={(columnMeta) => [
          { key: "username", label: t("accessControl.users.username"), ...columnMeta("username"), render: (r: UserRow) => <span className="font-medium">{r.username}</span> },
          { key: "email", label: t("accessControl.users.email"), ...columnMeta("email") },
          { key: "status", label: t("status"), ...columnMeta("status"), render: (r: UserRow) => <StatusChip status={r.status === "active" ? "positive" as const : "neutral" as const} label={t(`statuses.${r.status}`, r.status)} /> },
          { key: "roles", label: t("accessControl.users.roles"), ...columnMeta("roles"), render: (r: UserRow) => {
            const userRoles = getUserRoles(r.username);
            return userRoles.length > 0
              ? <div className="flex flex-wrap gap-1">{userRoles.map(role => <span key={role} className="surface-chip px-2 py-1 rounded text-xs"><EntityRef {...labels.role(role)} /></span>)}</div>
              : <span className="text-xs text-muted-foreground">—</span>;
          }},
          { key: "auth_source", label: t("accessControl.users.auth"), ...columnMeta("auth_source"), render: (r: UserRow) => <span className="surface-chip px-1.5 py-0.5 rounded text-xs">{r.authSource}</span> },
        ]}
        canCreate={canCreate}
        canUpdate={canUpdate}
        canDelete={canDelete}
        createLabel={t("accessControl.users.addUser")}
        onCreate={openCreate}
        onEdit={openEdit}
        extraActions={(user) => (
          canUpdate && user.authSource === "local"
            ? [{ key: "password", label: t("accessControl.users.changePassword"), icon: <KeyRound />, onSelect: (u, trigger) => openPasswordReset(u, trigger) }]
            : []
        )}
        editor={editor}
        deleteTitle={t("accessControl.users.deleteTitle")}
        deleteDescription={(u) => (
          <Trans i18nKey="accessControl.users.deleteDescription" values={{ name: u?.username ?? "" }} components={{ 1: <strong /> }} />
        )}
        deleteActionLabel={t("delete")}
        onDelete={deleteUser}
      />

      <Dialog open={!!passwordTarget} onOpenChange={open => !open && setPasswordTarget(null)}>
        <DialogContent className="border border-border bg-card max-h-[90dvh] overflow-y-auto sm:max-w-md" onCloseAutoFocus={(event) => {
          if (passwordInvoker.current?.isConnected) {
            event.preventDefault();
            passwordInvoker.current.focus();
          }
        }}>
          <DialogHeader>
            <DialogTitle>{t("accessControl.users.changePassword")}</DialogTitle>
            <DialogDescription>
              <Trans
                i18nKey="accessControl.users.changePasswordDescription"
                values={{ name: passwordTarget?.username ?? "" }}
                components={{ 1: <strong /> }}
              />
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-4 py-2">
            <FormField htmlFor="access-reset-password" label={t("accessControl.users.newPassword")} error={passwordErrors.password} required>
              <FormFieldControl>
              <Input
                id="access-reset-password"
                aria-invalid={!!passwordErrors.password}
                type="password"
                value={passwordForm.password}
                onChange={e => {
                  setPasswordForm(prev => ({ ...prev, password: e.target.value }));
                  if (passwordErrors.password) setPasswordErrors(prev => ({ ...prev, password: undefined }));
                }}
                placeholder={t("accessControl.users.passwordPlaceholder")}
              />
              </FormFieldControl>
            </FormField>
            <FormField htmlFor="access-reset-confirm-password" label={t("accessControl.users.confirmPassword")} error={passwordErrors.confirmPassword} required>
              <FormFieldControl>
              <Input
                id="access-reset-confirm-password"
                aria-invalid={!!passwordErrors.confirmPassword}
                type="password"
                value={passwordForm.confirmPassword}
                onChange={e => {
                  setPasswordForm(prev => ({ ...prev, confirmPassword: e.target.value }));
                  if (passwordErrors.confirmPassword) setPasswordErrors(prev => ({ ...prev, confirmPassword: undefined }));
                }}
                placeholder={t("accessControl.users.passwordPlaceholder")}
              />
              </FormFieldControl>
            </FormField>
          </div>
          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setPasswordTarget(null)} disabled={passwordSubmitting}>
              {t("cancel")}
            </Button>
            <Button size="sm" onClick={handlePasswordSave} disabled={passwordSubmitting}>
              {t("accessControl.users.savePassword")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};

export default AccessUsers;
