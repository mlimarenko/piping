import { useMemo, useState } from "react";
import { useTranslation, Trans } from "react-i18next";
import SurfaceCard, { SurfaceCardHeader, SurfaceCardContent } from "@/shared/ui-shared/SurfaceCard";
import { type DataTableFilterOption } from "@/shared/ui-shared/DataTable";
import EmptyState from "@/shared/ui-shared/EmptyState";
import EntityRef from "@/shared/ui-shared/EntityRef";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FormField from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { Button } from "@/shared/ui-primitives/button";
import { Plus, Award, Key, Users, User, ArrowRight } from "lucide-react";
import { notify } from "@/shared/lib/notifications";
import { cn } from "@/shared/lib/utils";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { AccessEntityList } from "./AccessEntityList";
import { useAccessData, useAccessLabels, type Grant } from "./AccessDataContext";
import { listGrants } from "@/features/access-control/api";
import { GRANTS_TABLE_CONFIG } from "@/features/access-control/tableConfig";
import type { Uuid } from "@/shared/lib/ids";

const AccessGrants = () => {
  const { t } = useTranslation();
  const { createGrant, deleteGrant, roles, users, groups, grants: allGrants } = useAccessData();
  const labels = useAccessLabels();

  // View-but-not-edit: grants are gated on ROLE server-side, so hide the
  // add-grant and revoke affordances when the caller lacks the capability.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "ROLE");
  const canDelete = can("DELETE", "ROLE");

  const [role, setRole] = useState<Uuid | "">("");
  const [subjectType, setSubjectType] = useState<"user" | "group">("user");
  const [subject, setSubject] = useState<Uuid | "">("");
  const [submitting, setSubmitting] = useState(false);

  const userIds = useMemo(() => new Set<string>(users.map((u) => u.id)), [users]);
  const subjectOptionList = useMemo<DataTableFilterOption[]>(
    () => [
      ...users.map((u) => ({ value: u.id, label: u.username })),
      ...groups.map((g) => ({ value: g.id, label: g.name })),
    ],
    [users, groups],
  );

  const subjectOptions = subjectType === "user"
    ? users.map((u) => ({ id: u.id, label: u.username, icon: User }))
    : groups.map((g) => ({ id: g.id, label: g.name, icon: Users }));

  const handleAdd = async () => {
    if (!role || !subject) { notify("error", t("accessControl.grants.errors.subjectRequired")); return; }
    if (allGrants.some((g) => g.role === role && g.subject === subject && g.subjectType === subjectType)) {
      notify("error", t("accessControl.grants.errors.alreadyExists"));
      return;
    }
    setSubmitting(true);
    try {
      await createGrant({ role, subjectType, subject });
      setRole("");
      setSubject("");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <AccessEntityList<Grant>
      tableId="access.grants"
      config={GRANTS_TABLE_CONFIG}
      queryKeyPrefix={["access", "grants"]}
      listFn={listGrants}
      getRowKey={r => r.id}
      rowLabel={r => `${labels.role(r.role).name ?? r.role} → ${labels.subject(r.subject, r.subjectType).name ?? r.subject}`}
      searchPlaceholder={t("accessControl.grants.searchPlaceholder")}
      emptyState={<EmptyState icon={Award} title={t("accessControl.grants.noGrants")} description={t("accessControl.grants.noGrantsDescription")} />}
      fkOptions={{
        role: roles.map((r) => ({ value: r.id, label: r.name })),
        subject: subjectOptionList,
      }}
      filterContext={{ subjectUserIds: userIds }}
      columns={(columnMeta) => [
        { key: "role", label: t("accessControl.grants.role"), ...columnMeta("role"), render: (r: Grant) => (
          <div className="flex items-center gap-2">
            <div className="w-5 h-5 rounded bg-primary/10 flex items-center justify-center"><Key className="h-3 w-3 text-primary" /></div>
            <EntityRef {...labels.role(r.role)} className="font-medium text-foreground" />
          </div>
        )},
        { key: "arrow", label: "", excludeFromSettings: true, render: () => <ArrowRight className="h-3 w-3 text-muted-foreground" /> },
        { key: "subject", label: t("accessControl.grants.subject"), ...columnMeta("subject"), render: (r: Grant) => (
          <EntityRef
            {...labels.subject(r.subject, r.subjectType)}
            icon={r.subjectType === "user" ? User : Users}
          />
        )},
        { key: "type", label: t("type"), ...columnMeta("type"), render: (r: Grant) => (
          <span className={cn("surface-chip px-2 py-0.5 rounded text-xs font-medium", r.subjectType === "user" ? "text-status-info" : "text-status-positive")}>
            {r.subjectType === "user" ? t("accessControl.grants.subjectUser") : t("accessControl.grants.subjectGroup")}
          </span>
        )},
      ]}
      canCreate={canCreate}
      canUpdate={false}
      canDelete={canDelete}
      prefix={canCreate && (
        <SurfaceCard className="shrink-0">
          <SurfaceCardHeader title={<span className="flex items-center gap-2"><Award className="size-4 text-primary" />{t("accessControl.grants.createGrant")}</span>} />
          <SurfaceCardContent className="flex items-end gap-3 flex-wrap">
            <FormField
              htmlFor="access-grant-role"
              className="flex-1 min-w-[120px]"
              label={t("accessControl.grants.role")}
              labelClassName="text-xs text-muted-foreground"
              required
            >
              <EntitySelect
                id="access-grant-role"
                items={roles.map(r => ({ id: r.id, label: r.name, icon: Key }))}
                value={role}
                onValueChange={setRole}
                placeholder={t("accessControl.grants.selectRole")}
                searchPlaceholder={t("search")}
                triggerClassName="h-9 text-sm"
              />
            </FormField>
            <div className="flex items-center pb-0.5"><ArrowRight className="h-4 w-4 text-muted-foreground" /></div>
            <FormSelect
              className="w-28"
              label={t("type")}
              labelClassName="text-xs text-muted-foreground"
              value={subjectType}
              onValueChange={v => { setSubjectType(v); setSubject(""); }}
              options={[
                { value: "user", label: t("accessControl.grants.subjectUser") },
                { value: "group", label: t("accessControl.grants.subjectGroup") },
              ]}
            />
            <FormField
              htmlFor="access-grant-subject"
              className="flex-1 min-w-[120px]"
              label={t("accessControl.grants.subject")}
              labelClassName="text-xs text-muted-foreground"
              required
            >
              <EntitySelect
                id="access-grant-subject"
                items={subjectOptions}
                value={subject}
                onValueChange={setSubject}
                placeholder={t("accessControl.grants.selectSubject")}
                searchPlaceholder={t("search")}
                triggerClassName="h-9 text-sm"
              />
            </FormField>
            <Button
              className="gap-1 shrink-0"
              onClick={handleAdd}
              disabled={!role || !subject || submitting}
            >
              <Plus /> {t("accessControl.grants.addGrant")}
            </Button>
          </SurfaceCardContent>
        </SurfaceCard>
      )}
      deleteTitle={t("accessControl.grants.deleteTitle")}
      deleteDescription={(g) => (
        <Trans
          i18nKey="accessControl.grants.deleteDescription"
          values={{
            subject: g
              ? (labels.subject(g.subject, g.subjectType).name ?? g.subject)
              : "",
          }}
          components={{ 1: <strong /> }}
        />
      )}
      deleteActionLabel={t("accessControl.grants.revoke")}
      onDelete={deleteGrant}
    />
  );
};

export default AccessGrants;
