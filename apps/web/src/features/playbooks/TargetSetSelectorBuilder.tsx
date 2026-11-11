import { useId, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ChevronDown } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { Input } from "@/shared/ui-primitives/input";
import { Switch } from "@/shared/ui-primitives/switch";
import { Label } from "@/shared/ui-primitives/label";
import { useDebouncedValue } from "@/shared/hooks/useDebouncedValue";
import { TablePagination } from "@/shared/ui-shared/Pagination";
import { TABLE_PAGE_SIZE_OPTIONS } from "@/shared/settings/registry";
import { inventoryResourceStatusLabel } from "@/features/inventory/resourceStatus";
import { listTargetSetFacets, type TargetSetFacet, type TargetSetFacetPage } from "./api";
import { SELECTOR_FACETS, SELECTOR_FACET_KEYS, modelToSelector, type SelectorFacet, type SelectorModel } from "./selectorModel";
import HostMultiSelectPicker from "./HostMultiSelectPicker";
import { toSelectedHost, type SelectedHost } from "./hostSelection";
import { useConnectionTargetLabels } from "@/features/connections/useConnectionTargetLabels";

function TargetOverrides({ model, onChange, field }: Readonly<{
  model: SelectorModel;
  onChange: (model: SelectorModel) => void;
  field: "include_target_ids" | "exclude_target_ids";
}>) {
  const { t } = useTranslation();
  const [labels, setLabels] = useState(new Map<string, SelectedHost>());
  const [open, setOpen] = useState(false);
  const raw = model.passthrough[field];
  const ids = Array.isArray(raw) ? raw.filter((id): id is string => typeof id === "string") : [];
  const loaded = useConnectionTargetLabels(ids, open);
  const hydrated = new Map<string, SelectedHost>(loaded.data?.items.map(target => [target.id, toSelectedHost(target)]));
  const selected = ids.map(id => hydrated.get(id) ?? labels.get(id) ?? { id, name: id, subtitle: "", sourceKind: "device" as const, sourceId: null });
  return <details className="rounded-md border border-border p-3" open={open} onToggle={event => setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer text-sm font-medium">{t(`playbooks.targetOverrides.${field}`)}{ids.length ? ` (${ids.length})` : ""}</summary>
    <p className="my-2 text-xs text-muted-foreground">{t(`playbooks.targetOverrides.${field}Hint`)}</p>
    {(loaded.data?.unavailable ?? 0) > 0 && <p role="status" className="mb-2 text-xs text-muted-foreground">{t("playbooks.page.selectorBuilder.memberLabelsUnavailable")}</p>}
    {open && <HostMultiSelectPicker selected={selected} onChange={hosts => {
      setLabels(new Map(hosts.map(host => [host.id, host])));
      const passthrough = { ...model.passthrough };
      if (hosts.length) passthrough[field] = hosts.map(host => host.id);
      else delete passthrough[field];
      onChange({ ...model, passthrough });
    }} />}
  </details>;
}

function FacetSection({ facet, model, onChange, initiallyOpen = false }: Readonly<{
  facet: SelectorFacet;
  model: SelectorModel;
  onChange: (model: SelectorModel) => void;
  initiallyOpen?: boolean;
}>) {
  const { t } = useTranslation();
  const id = useId();
  const [open, setOpen] = useState(initiallyOpen);
  const [search, setSearch] = useState("");
  const debounced = useDebouncedValue(search);
  const [paging, setPaging] = useState({ context: "", page: 0, pageSize: 50 });
  const client = useQueryClient();
  const selected = model.facets[facet];
  const selector = modelToSelector(model);
  delete selector[SELECTOR_FACET_KEYS[facet]];
  const selectorKey = JSON.stringify(selector);
  const context = `${selectorKey}:${debounced}`;
  const page = paging.context === context ? paging.page : 0;
  const query = useQuery({
    queryKey: ["playbooks", "target-set-facets", facet, selectorKey, debounced, page, paging.pageSize],
    queryFn: () => listTargetSetFacets(facet, selector, debounced, page * paging.pageSize, paging.pageSize),
    enabled: open,
    throwOnError: false,
  });
  const options = query.data?.items ?? [];
  const known = new Map<string, TargetSetFacet>();
  for (const [, data] of client.getQueriesData<TargetSetFacetPage>({ queryKey: ["playbooks", "target-set-facets", facet] })) {
    for (const option of data?.items ?? []) known.set(option.id, option);
  }
  const visible = new Set(options.map(option => option.id));
  const rows = [
    ...selected.filter(value => !visible.has(value)).map(value => ({ id: value, name: known.get(value)?.name ?? value, count: null })),
    ...options,
  ];
  const label = t(`playbooks.page.selectorBuilder.facets.${facet}`);
  const setSelected = (values: string[]) => onChange({ ...model, facets: { ...model.facets, [facet]: values } });
  return <section className="rounded-md border border-border bg-card">
    <div className="flex items-center gap-2 px-2 py-1">
      <button type="button" className="flex min-h-10 min-w-0 flex-1 items-center gap-2 rounded-md text-left text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" aria-label={selected.length ? `${label} ${selected.length}` : label} aria-expanded={open} aria-controls={id} onClick={() => setOpen(value => !value)}>
        <ChevronDown aria-hidden="true" className={`size-4 shrink-0 transition-transform ${open ? "" : "-rotate-90"}`} />
        {label}{selected.length > 0 && <span className="text-xs text-muted-foreground">{selected.length}</span>}
      </button>
      {selected.length > 0 && <Button type="button" variant="ghost" size="sm" onClick={() => setSelected([])}>{t("playbooks.page.selectorBuilder.clear")}</Button>}
    </div>
    {open && <div id={id} className="flex flex-col gap-2 border-t border-border p-2">
      <Input value={search} onChange={event => setSearch(event.target.value)} aria-label={`${t("search")}: ${label}`} placeholder={t("search")} />
      {query.isPending && <p role="status" className="text-sm text-muted-foreground">{t("loading")}</p>}
      {query.isError && <div role="alert" className="flex items-center gap-2 text-sm"><span>{t("unknownError")}</span><Button type="button" variant="outline" size="sm" onClick={() => void query.refetch()}>{t("retry")}</Button></div>}
      <div className="flex max-h-60 flex-col gap-1 overflow-y-auto">
        {rows.map(option => <label key={option.id} className="flex min-h-10 cursor-pointer items-center gap-2 rounded-md px-2 hover:bg-muted">
          <Checkbox aria-label={`${facet === "status" ? inventoryResourceStatusLabel(t, option.name) : option.name}: ${option.count == null ? t("playbooks.targetSetPicker.unknownHosts") : t("playbooks.targetSetPicker.hosts", { count: option.count })}`} checked={selected.includes(option.id)} onCheckedChange={() => setSelected(selected.includes(option.id) ? selected.filter(value => value !== option.id) : [...selected, option.id])} />
          <span className="min-w-0 flex-1 break-words text-sm">{facet === "status" ? inventoryResourceStatusLabel(t, option.name) : option.name}</span>
          <span className="shrink-0 text-xs tabular-nums text-muted-foreground">{option.count ?? t("playbooks.targetSetPicker.unknownHosts")}</span>
        </label>)}
      </div>
      {!query.isPending && !query.isError && rows.length === 0 && <p className="text-sm text-muted-foreground">{t("playbooks.page.hostPicker.noMatches")}</p>}
      {((query.data?.total ?? 0) > paging.pageSize || page > 0) && <TablePagination compact page={page} pageSize={paging.pageSize} total={query.data?.total ?? 0}
        pageSizeOptions={TABLE_PAGE_SIZE_OPTIONS.filter(size => size <= 100)}
        onPageChange={page => setPaging({ ...paging, context, page })}
        onPageSizeChange={pageSize => setPaging({ context, page: 0, pageSize })} />}
    </div>}
  </section>;
}

export default function TargetSetSelectorBuilder({ model, onChange, focusFacet }: Readonly<{
  model: SelectorModel;
  onChange: (model: SelectorModel) => void;
  focusFacet?: SelectorFacet;
}>) {
  const { t } = useTranslation();
  const addressId = useId();
  return <div className="flex min-w-0 flex-col gap-2">
    {SELECTOR_FACETS.map(facet => <FacetSection key={`${focusFacet ?? ""}:${facet}`} facet={facet} model={model} onChange={onChange} initiallyOpen={facet === focusFacet} />)}
    <TargetOverrides model={model} onChange={onChange} field="include_target_ids" />
    <TargetOverrides model={model} onChange={onChange} field="exclude_target_ids" />
    <div className="flex items-center justify-between gap-2 rounded-md border border-border p-3">
      <div className="min-w-0"><Label htmlFor={addressId}>{t("playbooks.page.selectorBuilder.addressBound")}</Label><p className="text-xs text-muted-foreground">{t("playbooks.page.selectorBuilder.addressBoundHint")}</p></div>
      <Switch id={addressId} checked={model.addressBound} onCheckedChange={addressBound => onChange({ ...model, addressBound })} />
    </div>
    {Object.keys(model.passthrough).length > 0 && <details className="min-w-0 rounded-md border border-border p-3 text-sm">
      <summary className="cursor-pointer">{t("playbooks.page.selectorBuilder.additionalConditions")}</summary>
      <pre className="my-2 max-h-48 overflow-auto text-xs">{JSON.stringify(model.passthrough, null, 2)}</pre>
      <Button type="button" variant="outline" size="sm" onClick={() => onChange({ ...model, passthrough: {} })}>{t("playbooks.page.selectorBuilder.removeAdditionalConditions")}</Button>
    </details>}
  </div>;
}
