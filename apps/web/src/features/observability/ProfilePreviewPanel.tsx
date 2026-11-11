import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { previewProfile } from "./api";
import { Button } from "@/shared/ui-primitives/button";
import { Checkbox } from "@/shared/ui-primitives/checkbox";

export default function ProfilePreviewPanel({ profileId, scopeType, scopeId, target, accepted, onAcceptedChange }: { profileId: string; scopeType: string; scopeId: string; target: string; accepted: boolean; onAcceptedChange: (value: boolean) => void }) {
  const { t } = useTranslation();
  const query = useQuery({ queryKey: ["observability", "profile-preview", profileId, scopeType, scopeId, target], queryFn: () => previewProfile(profileId, { scope_type: scopeType, scope_id: scopeType === "global" ? null : scopeId, target: target.trim() || null }), staleTime: 0 });
  return <section aria-label={t("observability.profiles.preview")} className="flex min-w-0 flex-col gap-2">
    <h2 className="text-sm font-semibold">{t("observability.profiles.preview")}</h2>
    {query.isPending ? <p role="status">{t("loading")}</p> : query.isError ? <div role="alert"><p>{t("observability.entityLoadFailed")}</p><Button variant="outline" onClick={() => void query.refetch()}>{t("retry")}</Button></div> : query.data && <>
      <p>{t("observability.profiles.previewCount", { hosts: query.data.hosts.length, targets: query.data.targets_count })}</p>
      {query.data.hosts.length === 0 && <p role="status" className="text-sm text-muted-foreground">{t("observability.profiles.noMatchingHosts")}</p>}
      {query.data.hosts.map(host => <div key={host.host_id} className="min-w-0 text-sm"><span className="font-medium">{host.host_name}</span><ul className="break-all font-mono text-xs">{host.targets.map((value, index) => <li key={`${value}-${index}`}>{value}</li>)}</ul></div>)}
      {query.data.conflicts.map((conflict, index) => <p role="alert" key={`${conflict.profile_id}-${index}`} className="break-words text-sm text-destructive">{conflict.message}</p>)}
      <label className="flex min-h-11 items-center gap-2 text-sm"><Checkbox checked={accepted} onCheckedChange={value => onAcceptedChange(value === true)} />{t("observability.profiles.confirmPreview")}</label>
    </>}
  </section>;
}
