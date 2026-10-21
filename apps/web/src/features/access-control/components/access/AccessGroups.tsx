import { useState } from "react";
import { useTranslation, Trans } from "react-i18next";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import FormSection from "@/shared/ui-shared/FormSection";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import { UserCheck } from "lucide-react";
import { AccessEntityList } from "./AccessEntityList";
import { useAccessData, type Group } from "./AccessDataContext";
import { listGroups } from "@/features/access-control/api";
import { GROUPS_TABLE_CONFIG } from "@/features/access-control/tableConfig";
import { usePermissions } from "@/shared/auth/permissionsContext";

const AccessGroups = () => {
  const { t } = useTranslation();
  const { createGroup, updateGroup, deleteGroup, grants } = useAccessData();
  // View-but-not-edit: hide create/edit/delete affordances the caller can't use.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "GROUP");
  const canUpdate = can("UPDATE", "GROUP");
  const canDelete = can("DELETE", "GROUP");

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<Group | null>(null);
  const [name, setName] = useState("");
  const [externalId, setExternalId] = useState("");
  const [nameError, setNameError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [baseline, setBaseline] = useState({ name: "", externalId: "" });

  const openCreate = () => { setEditing(null); setName(""); setExternalId(""); setNameError(""); setBaseline({ name: "", externalId: "" }); setDialogOpen(true); };
  const openEdit = (g: Group) => { setEditing(g); setName(g.name); setExternalId(g.externalId); setNameError(""); setBaseline({ name: g.name, externalId: g.externalId }); setDialogOpen(true); };

  const handleSave = async () => {
    if (submitting) return;
    if (!name.trim()) { setNameError(t("accessControl.groups.errors.nameRequired")); return; }
    setSubmitting(true);
    try {
      if (editing) {
        await updateGroup(editing.id, { name: name.trim(), externalId: externalId.trim() });
      } else {
        await createGroup({ name: name.trim(), externalId: externalId.trim() });
      }
      setDialogOpen(false);
    } catch {
      // AccessDataProvider reports the error; retain the draft for correction.
    } finally {
      setSubmitting(false);
    }
  };

  // Count role assignments for this group (from context full list)
  const getAssignmentCount = (groupName: string) => grants.filter(g => g.subjectType === "group" && g.subject === groupName).length;

  const editor = (
    <EntityEditorHost open={dialogOpen} onOpenChange={next => { if (!submitting) setDialogOpen(next); }}
      title={editing ? t("accessControl.groups.editGroup") : t("accessControl.groups.createGroup")}
      description={editing ? t("accessControl.groups.updateDescription") : t("accessControl.groups.createDescription")}
      width="compact"
      dirty={name !== baseline.name || externalId !== baseline.externalId}
      saving={submitting}
      footer={<>
        <Button data-editor-cancel variant="outline" onClick={() => setDialogOpen(false)} disabled={submitting}>{t("cancel")}</Button>
        <Button onClick={handleSave} disabled={submitting}>{editing ? t("saveChanges") : t("accessControl.groups.createGroup")}</Button>
      </>}
    >
      <FormSection contentClassName="flex flex-col gap-4">
        <FormField htmlFor="access-groups-name" label={t("accessControl.groups.groupName")} error={nameError} required>
          <FormFieldControl><Input id="access-groups-name" disabled={submitting} aria-invalid={!!nameError} value={name} onChange={e => { if (!submitting) { setName(e.target.value); setNameError(""); } }} placeholder={t("accessControl.groups.groupNamePlaceholder")} maxLength={100} /></FormFieldControl>
        </FormField>
        <FormField htmlFor="access-groups-externalId" label={t("accessControl.users.externalId")}>
          <Input id="access-groups-externalId" disabled={submitting} value={externalId} onChange={e => { if (!submitting) setExternalId(e.target.value); }} placeholder={t("accessControl.users.externalIdPlaceholder")} maxLength={255} />
        </FormField>
      </FormSection>
    </EntityEditorHost>
  );

  return (
    <AccessEntityList<Group>
      tableId="access.groups"
      config={GROUPS_TABLE_CONFIG}
      queryKeyPrefix={["access", "groups"]}
      listFn={listGroups}
      getRowKey={r => r.id}
      rowLabel={r => r.name}
      searchPlaceholder={t("accessControl.groups.searchPlaceholder")}
      emptyState={<EmptyState icon={UserCheck} title={t("accessControl.groups.noGroups")} description={t("accessControl.groups.noGroupsDescription")} />}
      columns={(columnMeta) => [
        { key: "name", label: t("name"), ...columnMeta("name"), render: (r: Group) => <span className="font-medium">{r.name}</span> },
        { key: "external_id", label: t("accessControl.users.externalId"), ...columnMeta("external_id"), render: (r: Group) => <span className="font-mono text-xs text-muted-foreground">{r.externalId || "—"}</span> },
        { key: "assignments", label: t("accessControl.groups.members"), ...columnMeta("assignments"), render: (r: Group) => {
          const count = getAssignmentCount(r.name);
          return <span className="surface-chip px-1.5 py-0.5 rounded text-xs">{count}</span>;
        }},
      ]}
      canCreate={canCreate}
      canUpdate={canUpdate}
      canDelete={canDelete}
      createLabel={t("accessControl.groups.addGroup")}
      onCreate={openCreate}
      onEdit={openEdit}
      editor={editor}
      deleteTitle={t("accessControl.groups.deleteTitle")}
      deleteDescription={(g) => (
        <Trans i18nKey="accessControl.groups.deleteDescription" values={{ name: g?.name ?? "" }} components={{ 1: <strong /> }} />
      )}
      deleteActionLabel={t("delete")}
      onDelete={deleteGroup}
    />
  );
};

export default AccessGroups;
