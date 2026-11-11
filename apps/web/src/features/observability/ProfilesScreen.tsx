import { useState } from "react";
import { Link } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Plus } from "lucide-react";
import { usePermissions } from "@/shared/auth/permissionsContext";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import SearchInput from "@/shared/ui-shared/SearchInput";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import { Button } from "@/shared/ui-primitives/button";
import { listAllProfiles, type MonitoringProfile } from "./api";
import { PROFILES_KEY } from "./keys";

export const PROFILES_BASE = "/observability/profiles";

export default function ProfilesScreen() {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const [search, setSearch] = useState("");
  const [enabled, setEnabled] = useState("all");
  const [requestedPage, setPage] = useState(0);
  const pageSize = useTablePageSize();
  const query = useQuery({ queryKey: PROFILES_KEY, queryFn: () => listAllProfiles() });
  const term = search.trim().toLocaleLowerCase();
  const rows = (query.data ?? []).filter(profile => (enabled === "all" || String(profile.enabled) === enabled) && [profile.name, profile.slug, profile.description].some(value => value?.toLocaleLowerCase().includes(term)));
  const page = Math.min(requestedPage, Math.max(0, Math.ceil(rows.length / pageSize.value) - 1));
  const columns: Column<MonitoringProfile>[] = [
    { key: "name", label: t("name"), render: profile => <Link className="break-words text-primary hover:underline" to={`${PROFILES_BASE}/${profile.id}`}>{profile.name}<span className="block text-xs text-muted-foreground">{profile.slug}</span></Link> },
    { key: "item_count", label: t("observability.profiles.items"), render: profile => profile.item_count },
    { key: "assignment_count", label: t("observability.profiles.assignments"), render: profile => profile.assignment_count },
    { key: "enabled", label: t("enabled"), render: profile => t(profile.enabled ? "enabled" : "disabled") },
  ];
  return <div className="flex min-w-0 flex-col gap-4">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <SearchInput aria-label={t("search")} placeholder={t("search")} value={search} onValueChange={value => { setSearch(value); setPage(0); }} containerClassName="min-w-0 flex-1" />
      <FormSelect label={t("enabled")} value={enabled} onValueChange={value => { setEnabled(value); setPage(0); }} options={[{value:"all",label:t("observability.profiles.allStates")},{value:"true",label:t("enabled")},{value:"false",label:t("disabled")}]} />
      {can("CREATE", "MONITORING") && <Button asChild><Link to={`${PROFILES_BASE}/new`}><Plus className="size-4" />{t("observability.profiles.new")}</Link></Button>}
    </div>
    <DataTable tableId="observability.profile-catalog" columns={columns} data={rows.slice(page * pageSize.value, (page + 1) * pageSize.value)} getRowKey={profile => profile.id} loading={query.isPending} error={query.isError ? t("observability.entityLoadFailed") : undefined} pagination={{page,pageSize:pageSize.value,total:rows.length,onPageChange:setPage,onPageSizeChange:value=>{pageSize.setValue(value);setPage(0);}}} emptyState={<p className="p-4 text-sm text-muted-foreground">{t("observability.profiles.empty")}</p>} />
    {query.isError && <Button variant="outline" onClick={() => void query.refetch()}>{t("retry")}</Button>}
  </div>;
}
