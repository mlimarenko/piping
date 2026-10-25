import { useState } from "react";
import { useTranslation, Trans } from "react-i18next";
import { useSearchParams } from "react-router";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import FormSection from "@/shared/ui-shared/FormSection";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import { Key } from "lucide-react";
import { AccessEntityList } from "./AccessEntityList";
import { useAccessData, type Role } from "./AccessDataContext";
import { listRoles } from "@/features/access-control/api";
import { ROLES_TABLE_CONFIG } from "@/features/access-control/tableConfig";
import { usePermissions } from "@/shared/auth/permissionsContext";

const AccessRoles = () => {
  const { t } = useTranslation();
  const [, setSearchParams] = useSearchParams();
  const { createRole, updateRole, deleteRole, policies } = useAccessData();
  // View-but-not-edit: hide create/edit/delete affordances the caller can't use.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "ROLE");
  const canUpdate = can("UPDATE", "ROLE");
  const canDelete = can("DELETE", "ROLE");

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<Role | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [nameError, setNameError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [baseline, setBaseline] = useState({ name: "", description: "" });

  const openCreate = () => { setEditing(null); setName(""); setDescription(""); setNameError(""); setBaseline({ name: "", description: "" }); setDialogOpen(true); };
  const openEdit = (r: Role) => { setEditing(r); setName(r.name); setDescription(r.description); setNameError(""); setBaseline({ name: r.name, description: r.description }); setDialogOpen(true); };

  const handleSave = async () => {
    if (submitting) return;
    if (!name.trim()) { setNameError(t("accessControl.roles.errors.nameRequired")); return; }
    setSubmitting(true);
    try {
      if (editing) {
        await updateRole(editing.id, { name: name.trim(), description: description.trim() });
      } else {
        await createRole({ name: name.trim(), description: description.trim() });
      }
      setDialogOpen(false);
    } catch {
      // AccessDataProvider reports the error; retain the draft for correction.
    } finally {
      setSubmitting(false);
    }
  };

  // Count policies that reference this role (from context full list). Policy
  // subjects are canonical `role:{uuid}`, so match on the role id, not name.
  const getPolicyCount = (roleId: string) =>
    policies.filter(p => p.subjects.some(s => s.kind === "role" && s.id === roleId)).length;

  const editor = (
    <EntityEditorHost open={dialogOpen} onOpenChange={next => { if (!submitting) setDialogOpen(next); }}
      title={editing ? t("accessControl.roles.editRole") : t("accessControl.roles.createRole")}
      description={editing ? t("accessControl.roles.updateDescription") : t("accessControl.roles.createDescription")}
      dirty={name !== baseline.name || description !== baseline.description}
      saving={submitting}
      footer={<>
        <Button data-editor-cancel variant="outline" onClick={() => setDialogOpen(false)} disabled={submitting}>{t("cancel")}</Button>
        <Button onClick={handleSave} disabled={submitting}>{editing ? t("saveChanges") : t("accessControl.roles.createRole")}</Button>
      </>}
    >
      <FormSection contentClassName="flex flex-col gap-4">
        <FormField htmlFor="access-roles-name" label={t("name")} error={nameError} required>
          <FormFieldControl><Input id="access-roles-name" disabled={submitting} aria-invalid={!!nameError} value={name} onChange={e => { if (!submitting) { setName(e.target.value); setNameError(""); } }} placeholder={t("accessControl.roles.roleNamePlaceholder")} maxLength={100} /></FormFieldControl>
        </FormField>
        <FormField htmlFor="access-roles-description" label={t("description")}>
          <Textarea id="access-roles-description" disabled={submitting} value={description} onChange={e => { if (!submitting) setDescription(e.target.value); }} placeholder={t("configContexts.describe")} maxLength={500} />
        </FormField>
      </FormSection>
    </EntityEditorHost>
  );

  return (
    <AccessEntityList<Role>
      tableId="access.roles"
      config={ROLES_TABLE_CONFIG}
      queryKeyPrefix={["access", "roles"]}
      listFn={listRoles}
      getRowKey={r => r.id}
      rowLabel={r => r.name}
      searchPlaceholder={t("accessControl.roles.searchPlaceholder")}
      emptyState={<EmptyState icon={Key} title={t("accessControl.roles.noRoles")} description={t("accessControl.roles.noRolesDescription")} />}
      columns={(columnMeta) => [
        { key: "name", label: t("name"), ...columnMeta("name"), render: (r: Role) => <span className="font-medium">{r.name}</span> },
        { key: "description", label: t("description"), ...columnMeta("description") },
        { key: "policyCount", label: t("accessControl.roles.policies"), ...columnMeta("policyCount"), render: (r: Role) => {
          // Count is over the loaded policy set (the policies list is paginated);
          // the title makes that explicit. Clicking opens the authoritative rule list.
          const count = getPolicyCount(r.id);
          return (
            <button
              type="button"
              title={t("accessControl.roles.usedByLoaded", { count })}
              aria-label={`${t("accessControl.roles.usedByLoaded", { count })}, ${t("accessControl.roles.viewAllRules")}`}
              className="surface-chip rounded px-1.5 py-0.5 text-xs transition-colors hover:text-primary"
              onClick={(e) => { e.stopPropagation(); setSearchParams({ tab: "access" }); }}
            >
              {count}
            </button>
          );
        } },
      ]}
      canCreate={canCreate}
      canUpdate={canUpdate}
      canDelete={canDelete}
      createLabel={t("accessControl.roles.addRole")}
      onCreate={openCreate}
      onEdit={openEdit}
      editor={editor}
      deleteTitle={t("accessControl.roles.deleteTitle")}
      deleteDescription={(r) => (
        <Trans i18nKey="accessControl.roles.deleteDescription" values={{ name: r?.name ?? "" }} components={{ 1: <strong /> }} />
      )}
      deleteActionLabel={t("delete")}
      onDelete={deleteRole}
    />
  );
};

export default AccessRoles;
