import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import {
  AlertTriangle,
  Cable,
  CircleDot,
  ExternalLink,
  Eye,
  Loader2,
  RefreshCw,
  RotateCcw,
  Search,
} from "lucide-react";
import {
  listCablingEndpoints,
  traceCablingEndpoint,
  type CablingEndpointItem,
  type CablingEndpointKind,
  type CablingLinkState,
  type CablingTrace,
  type CablingTraceSegment,
  type CablingTraceState,
} from "@/features/inventory/api";
import {
  inventoryEntityParams,
  inventoryResourceEditPath,
  isInventoryEntityKey,
} from "@/features/inventory/resourceLinks";
import EmptyState from "@/shared/ui-shared/EmptyState";
import FormSection from "@/shared/ui-shared/FormSection";
import { PageActions } from "@/shared/ui-shared/PageChrome";
import StatusChip from "@/shared/ui-shared/StatusChip";
import SearchInput from "@/shared/ui-shared/SearchInput";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import { Button } from "@/shared/ui-primitives/button";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/shared/ui-primitives/sheet";
import { cn } from "@/shared/lib/utils";

const ALL = "all";
const ENDPOINT_KINDS: CablingEndpointKind[] = [
  "interface",
  "front-port",
  "rear-port",
  "console-port",
  "console-server-port",
  "power-port",
  "power-outlet",
  "power-feed",
  "circuit-termination",
];
const LINK_STATES: CablingLinkState[] = ["available", "cabled", "marked-connected", "unknown"];
const TRACE_STATES: CablingTraceState[] = [
  "complete",
  "incomplete",
  "split",
  "invalid",
  "position-conflict",
  "unsupported",
];

type MobilePane = "find" | "trace";

function stateTone(state?: CablingTraceState | CablingLinkState | null) {
  if (state === "complete" || state === "cabled") return "positive";
  if (state === "invalid" || state === "position-conflict") return "negative";
  if (state === "incomplete" || state === "split" || state === "marked-connected") return "warning";
  return "neutral";
}

function labelKey(prefix: string, value?: string | null) {
  return value ? `${prefix}.${value}` : `${prefix}.unknown`;
}

function endpointLocation(item?: CablingEndpointItem | null): string {
  if (!item) return "";
  return [item.location.site?.label, item.location.rack?.label, item.location.location?.label]
    .filter(Boolean)
    .join(" / ");
}

function endpointKey(item: CablingEndpointItem): string {
  return `${item.endpoint.kind}:${item.endpoint.id}`;
}

const CablingWorkstation = () => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const searchRef = useRef<HTMLInputElement>(null);
  const inspectorTriggerRef = useRef<HTMLButtonElement>(null);
  const [search, setSearch] = useState("");
  const [endpointKind, setEndpointKind] = useState<CablingEndpointKind | "all">("all");
  const [linkState, setLinkState] = useState<CablingLinkState | "all">("all");
  const [traceState, setTraceState] = useState<CablingTraceState | "all">("all");
  const [issuesOnly, setIssuesOnly] = useState(false);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [inspectedKey, setInspectedKey] = useState<string | null>(null);
  const [mobilePane, setMobilePane] = useState<MobilePane>("find");
  // The mobile inspector is an overlay action, not a tab pane: it keeps its own
  // flag so cancelling the Sheet cannot change the find/trace selection.
  const [mobileInspectorOpen, setMobileInspectorOpen] = useState(false);

  const hasStructuredFilter =
    endpointKind !== "all" || linkState !== "all" || traceState !== "all" || issuesOnly;
  const trimmedSearch = search.trim();
  const searchTooShort = trimmedSearch.length > 0 && trimmedSearch.length < 2 && !hasStructuredFilter;

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.key === "/" && document.activeElement !== searchRef.current) {
        const target = event.target as HTMLElement | null;
        if (target?.tagName === "INPUT" || target?.tagName === "TEXTAREA") return;
        event.preventDefault();
        searchRef.current?.focus();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const endpointQuery = useQuery({
    queryKey: [
      "inventory",
      "cabling-workstation",
      "endpoints",
      trimmedSearch,
      endpointKind,
      linkState,
      traceState,
      issuesOnly,
    ],
    enabled: !searchTooShort && (trimmedSearch.length >= 2 || hasStructuredFilter),
    queryFn: () =>
      listCablingEndpoints({
        query: trimmedSearch || undefined,
        endpoint_kind: endpointKind === "all" ? undefined : [endpointKind],
        link_state: linkState === "all" ? undefined : [linkState],
        trace_state: traceState === "all" ? undefined : [traceState],
        issues_only: issuesOnly || undefined,
        limit: 50,
      }),
  });

  const items = useMemo(() => endpointQuery.data?.items ?? [], [endpointQuery.data?.items]);
  const effectiveSelectedKey = selectedKey ?? (items[0] ? endpointKey(items[0]) : null);
  const selected = items.find((item) => endpointKey(item) === effectiveSelectedKey) ?? items[0] ?? null;
  const inspected = items.find((item) => endpointKey(item) === inspectedKey) ?? selected;

  const traceQuery = useQuery({
    queryKey: ["inventory", "cabling-workstation", "trace", selected?.endpoint.kind, selected?.endpoint.id],
    enabled: Boolean(selected),
    queryFn: () => {
      if (!selected) throw new Error("missing selected endpoint");
      return traceCablingEndpoint(selected.endpoint);
    },
  });

  const trace = traceQuery.data ?? null;
  const badge = trace
    ? t(labelKey("inventory.cablingWorkstation.traceState", trace.state))
    : t("inventory.cablingWorkstation.resultCount", { count: endpointQuery.data?.total ?? 0 });

  const resetFilters = () => {
    setSearch("");
    setEndpointKind("all");
    setLinkState("all");
    setTraceState("all");
    setIssuesOnly(false);
    setSelectedKey(null);
  };

  const selectItem = (item: CablingEndpointItem) => {
    setSelectedKey(endpointKey(item));
    setInspectedKey(endpointKey(item));
    setMobilePane("trace");
  };

  const openResource = (entity: string, id?: string) => {
    if (!id || !isInventoryEntityKey(entity)) return;
    void navigate(inventoryResourceEditPath(entity, id));
  };

  return (
    <div className="@container flex min-h-0 flex-1 flex-col gap-3">
      <PageActions>
          <div className="flex items-center gap-2">
            <span className="text-xs text-muted-foreground">{badge}</span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-9 gap-1.5"
              onClick={() => endpointQuery.refetch()}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", endpointQuery.isFetching && "animate-spin")} />
              {t("refresh")}
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-9 gap-1.5"
              onClick={() => {
                const params = inventoryEntityParams("Cables");
                void navigate(`/inventory/resources?${params.toString()}`);
              }}
            >
              <ExternalLink className="h-3.5 w-3.5" />
              {t("inventory.cablingWorkstation.openCables")}
            </Button>
          </div>
      </PageActions>

      <div className="flex flex-col gap-2 @5xl:flex-row @5xl:items-center">
          <SearchInput
            ref={searchRef}
            value={search}
            onValueChange={setSearch}
            onKeyDown={(event) => {
              if (event.key === "Enter" && selected) {
                void traceQuery.refetch();
                setMobilePane("trace");
              }
            }}
            placeholder={t("inventory.cablingWorkstation.filters.search")}
            containerClassName="min-w-0 flex-1"
          />
        <FormSelect<CablingEndpointKind | "all">
          value={endpointKind} onValueChange={setEndpointKind}
          aria-label={t("inventory.cablingWorkstation.filters.allKinds")}
          className="@5xl:w-44"
          options={[{ value: ALL, label: t("inventory.cablingWorkstation.filters.allKinds") },
            ...ENDPOINT_KINDS.map((kind) => ({ value: kind, label: t(labelKey("inventory.cablingWorkstation.endpointKind", kind)) }))]}
        />
        <FormSelect<CablingLinkState | "all">
          value={linkState} onValueChange={setLinkState}
          aria-label={t("inventory.cablingWorkstation.filters.allLinkStates")}
          className="@5xl:w-44"
          options={[{ value: ALL, label: t("inventory.cablingWorkstation.filters.allLinkStates") },
            ...LINK_STATES.map((state) => ({ value: state, label: t(labelKey("inventory.cablingWorkstation.linkState", state)) }))]}
        />
        <FormSelect<CablingTraceState | "all">
          value={traceState} onValueChange={setTraceState}
          aria-label={t("inventory.cablingWorkstation.filters.allTraceStates")}
          className="@5xl:w-48"
          options={[{ value: ALL, label: t("inventory.cablingWorkstation.filters.allTraceStates") },
            ...TRACE_STATES.map((state) => ({ value: state, label: t(labelKey("inventory.cablingWorkstation.traceState", state)) }))]}
        />
        <label className="flex h-9 items-center gap-2 rounded-md border border-border px-2 text-sm">
          <Checkbox checked={issuesOnly} onCheckedChange={(value) => setIssuesOnly(Boolean(value))} />
          {t("inventory.cablingWorkstation.filters.issuesOnly")}
        </label>
        <Button type="button" variant="ghost" size="sm" className="h-9 gap-1.5" onClick={resetFilters}>
          <RotateCcw className="h-3.5 w-3.5" />
          {t("inventory.cablingWorkstation.filters.reset")}
        </Button>
      </div>

      {/* The mobile inspector is an ACTION, not a tab: the inspector the user
          sees on mobile is the Sheet below, so it must not be advertised as a
          tabpanel of the find/trace tablist; the desktop inspector column stays
          a plain grid child. `contents` keeps the tab wrappers box-neutral, so
          the responsive grid geometry is unchanged. An inactive wrapper is
          hidden below the same `@5xl` container threshold: a force-mounted
          tabpanel whose child is only CSS-hidden would still be exposed as an
          empty tabpanel in the accessibility tree. At `@5xl` both panes are
          visible together, so the wrappers return to `contents`. */}
      <Tabs value={mobilePane} onValueChange={(value) => setMobilePane(value as MobilePane)} className="contents">
        <div className="flex items-center gap-2 @5xl:hidden">
          <SectionTabs aria-label={t("inventory.cablingWorkstation.navLabel")} wrapperClassName="min-w-0 grow" className="w-full" triggerClassName="flex-1" items={(["find", "trace"] as const).map((pane) => ({
            value: pane, label: t(`inventory.cablingWorkstation.mobile.${pane}`),
          }))} />
          <Button
            ref={inspectorTriggerRef}
            type="button"
            variant="outline"
            size="sm"
            className="h-9 shrink-0 gap-1.5"
            aria-haspopup="dialog"
            aria-expanded={mobileInspectorOpen}
            onClick={() => setMobileInspectorOpen(true)}
          >
            <Eye className="h-3.5 w-3.5" />
            {t("inventory.cablingWorkstation.mobile.inspect")}
          </Button>
        </div>

      <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 @5xl:grid-cols-[minmax(18rem,22rem)_minmax(0,1fr)] @7xl:grid-cols-[minmax(19rem,22rem)_minmax(0,1fr)_minmax(19rem,22rem)]">
        <TabsContent value="find" forceMount tabIndex={-1} className={mobilePane === "find" ? "contents" : "hidden @5xl:contents"}>
        <FormSection title={t("inventory.cablingWorkstation.finder.title")} className={cn("min-w-0", mobilePane !== "find" && "hidden @5xl:block")}>
          <FinderPane
            items={items}
            selectedKey={selected ? endpointKey(selected) : null}
            isLoading={endpointQuery.isFetching}
            isError={endpointQuery.isError}
            searchTooShort={searchTooShort}
            canSearch={trimmedSearch.length >= 2 || hasStructuredFilter}
            onSelect={selectItem}
            onInspect={(item) => {
              setInspectedKey(endpointKey(item));
              setMobileInspectorOpen(true);
            }}
          />
        </FormSection>
        </TabsContent>

        <TabsContent value="trace" forceMount tabIndex={-1} className={mobilePane === "trace" ? "contents" : "hidden @5xl:contents"}>
        <FormSection title={t("inventory.cablingWorkstation.trace.title")} className={cn("min-w-0", mobilePane !== "trace" && "hidden @5xl:block")}>
          <TracePane
            trace={trace}
            selected={selected}
            isLoading={traceQuery.isFetching}
            isError={traceQuery.isError}
            onInspect={(item) => {
              setInspectedKey(endpointKey(item));
              setMobileInspectorOpen(true);
            }}
          />
        </FormSection>
        </TabsContent>

        <FormSection title={t("inventory.cablingWorkstation.inspector.title")} className="hidden min-w-0 @5xl:col-span-2 @5xl:block @7xl:col-span-1">
          <Inspector item={inspected} trace={trace} onOpen={openResource} />
        </FormSection>
      </div>
      </Tabs>

      <Sheet open={mobileInspectorOpen} onOpenChange={setMobileInspectorOpen}>
        <SheetContent
          side="right"
          className="flex w-full flex-col p-0 sm:max-w-md"
          onCloseAutoFocus={(event) => { event.preventDefault(); inspectorTriggerRef.current?.focus(); }}
        >
          <SheetHeader className="border-b border-border px-4 py-3">
            <SheetTitle>{t("inventory.cablingWorkstation.inspector.title")}</SheetTitle>
            <SheetDescription>{inspected?.endpoint.label ?? t("inventory.cablingWorkstation.inspector.emptyTitle")}</SheetDescription>
          </SheetHeader>
          <div className="min-h-0 flex-1 overflow-auto p-4">
            <Inspector item={inspected} trace={trace} onOpen={openResource} />
          </div>
          <SheetFooter className="border-t border-border p-4">
            <Button type="button" variant="outline" onClick={() => setMobileInspectorOpen(false)}>
              {t("close")}
            </Button>
          </SheetFooter>
        </SheetContent>
      </Sheet>
    </div>
  );
};

function FinderPane({
  items,
  selectedKey,
  isLoading,
  isError,
  searchTooShort,
  canSearch,
  onSelect,
  onInspect,
}: {
  items: CablingEndpointItem[];
  selectedKey: string | null;
  isLoading: boolean;
  isError: boolean;
  searchTooShort: boolean;
  canSearch: boolean;
  onSelect: (item: CablingEndpointItem) => void;
  onInspect: (item: CablingEndpointItem) => void;
}) {
  const { t } = useTranslation();
  if (searchTooShort) {
    return <EmptyState icon={Search} title={t("inventory.cablingWorkstation.searchTooShortTitle")} description={t("inventory.cablingWorkstation.searchTooShortDescription")} />;
  }
  if (!canSearch) {
    return <EmptyState icon={Search} title={t("inventory.cablingWorkstation.startTitle")} description={t("inventory.cablingWorkstation.startDescription")} />;
  }
  if (isError) {
    return <EmptyState icon={AlertTriangle} title={t("inventory.cablingWorkstation.errorTitle")} description={t("inventory.cablingWorkstation.errorDescription")} />;
  }
  if (isLoading && items.length === 0) {
    return <div className="flex h-52 items-center justify-center text-sm text-muted-foreground"><Loader2 className="mr-2 h-4 w-4 animate-spin" />{t("loading")}</div>;
  }
  if (items.length === 0) {
    return <EmptyState icon={Cable} title={t("inventory.cablingWorkstation.emptyTitle")} description={t("inventory.cablingWorkstation.emptyDescription")} />;
  }
  return (
    <div role="listbox" aria-label={t("inventory.cablingWorkstation.finder.title")} className="max-h-[62vh] overflow-auto p-2 @5xl:max-h-none">
      {items.map((item) => (
        <div
          key={endpointKey(item)}
          role="option"
          tabIndex={0}
          aria-selected={selectedKey === endpointKey(item)}
          onClick={() => onSelect(item)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              onSelect(item);
            }
          }}
          className={cn(
            "mb-2 flex w-full cursor-pointer flex-col gap-2 rounded-md border p-2 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
            selectedKey === endpointKey(item) ? "border-primary/50 bg-primary/10" : "border-input/30 hover:bg-accent/30",
          )}
        >
          <span className="flex items-center justify-between gap-2">
            <span className="min-w-0">
              <span className="block truncate text-sm font-medium">{item.endpoint.label}</span>
              <span className="block truncate text-xs text-muted-foreground">
                {item.parent?.label ?? t("inventory.cablingWorkstation.labels.noParent")}
              </span>
            </span>
            <StatusChip status={stateTone(item.link_state)} label={t(labelKey("inventory.cablingWorkstation.linkState", item.link_state))} />
          </span>
          <span className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
            <span className="truncate">{endpointLocation(item) || t("inventory.cablingWorkstation.labels.noLocation")}</span>
            <Button type="button" variant="ghost" size="sm" className="h-9 shrink-0 gap-1 px-2 text-sm" onClick={(event) => { event.stopPropagation(); onInspect(item); }}>
              <Eye className="h-3.5 w-3.5" />
              {t("inventory.cablingWorkstation.inspect")}
            </Button>
          </span>
        </div>
      ))}
    </div>
  );
}

function TracePane({
  trace,
  selected,
  isLoading,
  isError,
}: {
  trace: CablingTrace | null;
  selected: CablingEndpointItem | null;
  isLoading: boolean;
  isError: boolean;
  onInspect: (item: CablingEndpointItem) => void;
}) {
  const { t } = useTranslation();
  if (!selected) {
    return <EmptyState icon={CircleDot} title={t("inventory.cablingWorkstation.trace.emptyTitle")} description={t("inventory.cablingWorkstation.trace.emptyDescription")} />;
  }
  if (isError) {
    return <EmptyState icon={AlertTriangle} title={t("inventory.cablingWorkstation.trace.errorTitle")} description={t("inventory.cablingWorkstation.trace.errorDescription")} />;
  }
  if (isLoading && !trace) {
    return <div className="flex h-52 items-center justify-center text-sm text-muted-foreground"><Loader2 className="mr-2 h-4 w-4 animate-spin" />{t("loading")}</div>;
  }
  if (!trace) return null;
  return (
    <div className="min-w-0 space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <StatusChip status={stateTone(trace.state)} label={t(labelKey("inventory.cablingWorkstation.traceState", trace.state))} />
        <span className="text-xs text-muted-foreground">
          {t("inventory.cablingWorkstation.trace.summary", {
            hops: trace.summary.hop_count,
            issues: trace.summary.issue_count,
          })}
        </span>
      </div>
      {trace.issues.length > 0 ? (
        <div className="space-y-2">
          {trace.issues.map((issue) => (
            <div key={`${issue.code}:${issue.endpoint?.id ?? issue.message}`} className="rounded-md border border-warning/30 bg-warning/10 p-2 text-xs">
              <span className="font-semibold">{t(labelKey("inventory.cablingWorkstation.issue", issue.code))}</span>
              <span className="ml-2 text-muted-foreground">{issue.message}</span>
            </div>
          ))}
        </div>
      ) : null}
      <ol className="space-y-2">
        {trace.segments.length === 0 ? (
          <li className="rounded-md border border-border p-3 text-sm text-muted-foreground">
            {t("inventory.cablingWorkstation.trace.noSegments")}
          </li>
        ) : (
          trace.segments.map((segment) => <TraceSegment key={`${segment.index}:${segment.from.id}`} segment={segment} />)
        )}
      </ol>
    </div>
  );
}

function TraceSegment({ segment }: { segment: CablingTraceSegment }) {
  const { t } = useTranslation();
  return (
    <li className="rounded-md border border-border p-3">
      <div className="flex items-center justify-between gap-3">
        <span className="text-xs font-semibold uppercase text-muted-foreground">
          {segment.index + 1}. {t(labelKey("inventory.cablingWorkstation.traversal", segment.traversal))}
        </span>
        {segment.cable ? <StatusChip status="info" label={segment.cable.label} /> : null}
      </div>
      <div className="mt-2 grid gap-2 text-sm sm:grid-cols-[1fr_auto_1fr] sm:items-center">
        <span className="truncate">{segment.from.label}</span>
        <span className="text-muted-foreground">→</span>
        <span className="truncate font-medium">{segment.to?.label ?? "—"}</span>
      </div>
      {segment.note ? <p className="mt-2 text-xs text-muted-foreground">{segment.note}</p> : null}
    </li>
  );
}

function Inspector({
  item,
  trace,
  onOpen,
}: {
  item: CablingEndpointItem | null;
  trace: CablingTrace | null;
  onOpen: (entity: string, id?: string) => void;
}) {
  const { t } = useTranslation();
  if (!item) {
    return <EmptyState icon={Eye} title={t("inventory.cablingWorkstation.inspector.emptyTitle")} description={t("inventory.cablingWorkstation.inspector.emptyDescription")} />;
  }
  return (
    <div className="space-y-4 p-3">
      <div>
        <p className="text-sm font-semibold">{item.endpoint.label}</p>
        <p className="text-xs text-muted-foreground">{t(labelKey("inventory.cablingWorkstation.endpointKind", item.endpoint.kind))}</p>
      </div>
      <dl className="grid grid-cols-1 gap-2 text-sm">
        <Field label={t("inventory.cablingWorkstation.inspector.parent")} value={item.parent?.label} />
        <Field label={t("inventory.cablingWorkstation.inspector.location")} value={endpointLocation(item)} />
        <Field label={t("inventory.cablingWorkstation.inspector.cable")} value={item.cable?.label} />
        <Field label={t("inventory.cablingWorkstation.inspector.traceState")} value={trace ? t(labelKey("inventory.cablingWorkstation.traceState", trace.state)) : item.trace_state ? t(labelKey("inventory.cablingWorkstation.traceState", item.trace_state)) : undefined} />
      </dl>
      <div className="space-y-2">
        <h3 className="text-xs font-semibold uppercase text-muted-foreground">{t("inventory.cablingWorkstation.inspector.related")}</h3>
        {item.related_links.length === 0 ? (
          <p className="text-xs text-muted-foreground">{t("inventory.cablingWorkstation.inspector.noRelated")}</p>
        ) : (
          item.related_links.map((link) => (
            <Button key={`${link.entity}:${link.id}`} type="button" variant="outline" size="sm" className="mr-2 h-9 gap-1.5" onClick={() => onOpen(link.entity, link.id)}>
              <ExternalLink className="h-3.5 w-3.5" />
              {link.label}
            </Button>
          ))
        )}
      </div>
    </div>
  );
}

function Field({ label, value }: { label: string; value?: string | null }) {
  return (
    <div className="border-b border-border py-2">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="mt-1 truncate font-medium">{value || "—"}</dd>
    </div>
  );
}

export default CablingWorkstation;
