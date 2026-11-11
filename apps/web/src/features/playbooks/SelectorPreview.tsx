import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import type { PlaybookTargetSelectorPreview } from "./api";
import { SELECTOR_FACETS, type SelectorModel } from "./selectorModel";
import { inventoryResourceStatusLabel } from "@/features/inventory/resourceStatus";

export function SelectorPreview({ data, pending, failed, retry, model }: Readonly<{
  data?: PlaybookTargetSelectorPreview;
  pending: boolean;
  failed: boolean;
  retry: () => void;
  model: SelectorModel;
}>) {
  const { t } = useTranslation();
  if (pending) return <p role="status">{t("playbooks.selectorPreview.pending")}</p>;
  if (failed || !data) return <div role="alert" className="flex flex-col gap-2"><p>{t("playbooks.selectorPreview.failed")}</p><Button type="button" variant="outline" onClick={retry}>{t("retry")}</Button></div>;
  return <div className="flex min-w-0 flex-col gap-3">
    <p role="status" className="text-sm font-medium">{t("playbooks.targetSetPicker.hosts", { count: data.total })}</p>
    {data.total === 0 && <p className="text-sm text-muted-foreground">{t("playbooks.selectorPreview.empty")}</p>}
    {data.truncated && <p className="text-xs text-muted-foreground">{t("playbooks.selectorPreview.truncated", { count: data.items.length })}</p>}
    <ul className="flex min-w-0 flex-col divide-y divide-border">
      {data.items.map(host => <li key={`${host.source_kind}:${host.source_id}`} className="flex min-w-0 flex-col gap-1 py-2">
        <span className="break-words text-sm font-medium">{host.name}</span>
        {(Array.isArray(model.passthrough.include_target_ids) && model.passthrough.include_target_ids.includes(host.target_id) || Array.isArray(model.passthrough.include_source_ids) && model.passthrough.include_source_ids.includes(host.source_id)) && <span className="text-xs text-muted-foreground">{t("playbooks.targetOverrides.pinned")}</span>}
        <span className="break-all text-xs text-muted-foreground">{host.address ?? t("playbooks.page.preview.noAddress")}</span>
        <dl className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
          {SELECTOR_FACETS.filter(facet => model.facets[facet].length > 0 && facet !== "tag").map(facet => {
            const value = facet === "status" ? host.status : host[`${facet}_name` as keyof typeof host];
            return typeof value === "string" ? <div key={facet}><dt className="inline">{t(`playbooks.page.selectorBuilder.facets.${facet}`)}: </dt><dd className="inline">{facet === "status" ? inventoryResourceStatusLabel(t, value) : value}</dd></div> : null;
          })}
        </dl>
      </li>)}
    </ul>
  </div>;
}
