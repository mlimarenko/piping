import { useEffect, useId, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Check, Loader2, Search, X } from "lucide-react";

import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Button } from "@/shared/ui-primitives/button";
import { TablePagination } from "@/shared/ui-shared/Pagination";
import { TABLE_PAGE_SIZE_OPTIONS } from "@/shared/settings/registry";
import { useDebouncedValue } from "@/shared/hooks/useDebouncedValue";
import { cn } from "@/shared/lib/utils";
import type { ConnectionTarget } from "@/shared/api/types";
import { listConnectionTargetPage } from "@/features/connections/api";
import { toSelectedHost, type SelectedHost } from "@/features/playbooks/hostSelection";

interface HostMultiSelectPickerProps {
  selected: SelectedHost[];
  onChange: (next: SelectedHost[]) => void;
}

/** Page of the visible result list. Backend caps the page size, not the picker. */
const PICKER_PAGE_SIZE = 50;
/** Page of the "select all" sweep: fewer round-trips, still bounded. */
const SELECT_ALL_PAGE_SIZE = 250;
/** Per-token lookup page; exact-name/address matching only needs a small page. */
const PASTE_LOOKUP_LIMIT = 20;
const PASTE_CONCURRENCY = 4;
const MAX_PASTE_TOKENS = 500;
const REPORT_PREVIEW_LIMIT = 8;

type PickerReport = {
  unmatched: string[];
  ambiguous: string[];
  blocked: string[];
};

type PickerOperation =
  | { kind: "selectAll"; state: "running"; scanned: number; total: number }
  | { kind: "selectAll"; state: "done"; added: number; blocked: number; duplicates: number }
  | { kind: "paste"; state: "running"; done: number; total: number }
  | { kind: "paste"; state: "done"; added: number; duplicates: number; report: PickerReport }
  | { kind: "selectAll" | "paste"; state: "cancelled" }
  | { kind: "selectAll" | "paste"; state: "error" };

function emptyReport(): PickerReport {
  return { unmatched: [], ambiguous: [], blocked: [] };
}

/** Add the still-missing address-bound targets, keeping the current order. */
function mergeSelected(
  current: readonly SelectedHost[],
  additions: readonly ConnectionTarget[],
): SelectedHost[] {
  const seen = new Set(current.map((host) => host.id));
  const next = [...current];
  for (const target of additions) {
    if (target.addressBound === false || seen.has(target.id)) continue;
    seen.add(target.id);
    next.push(toSelectedHost(target));
  }
  return next;
}

/** One name or address per line (commas and semicolons also separate). */
function parsePasteTokens(text: string): string[] {
  const seen = new Set<string>();
  const tokens: string[] = [];
  for (const line of text.split(/[\n,;]+/)) {
    const token = line.trim();
    if (!token) continue;
    const key = token.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    tokens.push(token);
  }
  return tokens;
}

function exactMatches(targets: readonly ConnectionTarget[], token: string): ConnectionTarget[] {
  const needle = token.toLowerCase();
  const matched = targets.filter(
    (target) => target.name.toLowerCase() === needle || target.primaryIp === token,
  );
  return [...new Map(matched.map((target) => [target.id, target])).values()];
}

function ReportLine({ label, values }: Readonly<{ label: string; values: readonly string[] }>) {
  if (values.length === 0) return null;
  const shown = values.slice(0, REPORT_PREVIEW_LIMIT).join(", ");
  return (
    <li className="break-words">
      <span className="font-medium text-foreground">{label}</span> {shown}
      {values.length > REPORT_PREVIEW_LIMIT ? ` +${values.length - REPORT_PREVIEW_LIMIT}` : ""}
    </li>
  );
}

/**
 * Inline multi-select host picker for building a static target set.
 *
 * Results come from the paged, server-searched connection target read, so hosts
 * beyond the first screen are reachable. The selection is held by the caller and
 * rendered as its own list, independent of the current page and search term.
 * Only targets with a bound address can be selected: unaddressed rows stay
 * visible but disabled, and bulk actions report them instead of adding them.
 */
const HostMultiSelectPicker = ({ selected, onChange }: HostMultiSelectPickerProps) => {
  const { t } = useTranslation();
  const pasteId = useId();
  const [search, setSearch] = useState("");
  const debouncedSearch = useDebouncedValue(search);
  const [paging, setPaging] = useState({
    search: "",
    page: 0,
    pageSize: PICKER_PAGE_SIZE,
  });
  // A new search starts at the first page without an effect: the stored page
  // only applies while it belongs to the term the query is showing.
  const page = paging.search === debouncedSearch ? paging.page : 0;
  const pageSize = paging.pageSize;
  const [pasteOpen, setPasteOpen] = useState(false);
  const [pasteText, setPasteText] = useState("");
  const [operation, setOperation] = useState<PickerOperation | null>(null);

  const selectedRef = useRef(selected);
  useEffect(() => {
    selectedRef.current = selected;
  }, [selected]);

  // Bumped by cancel or by a newer bulk action; a superseded run never applies.
  const runToken = useRef(0);
  const running = operation?.state === "running";

  const pageQuery = useQuery({
    queryKey: ["connections", "targets", "picker", debouncedSearch, page, pageSize],
    queryFn: () =>
      listConnectionTargetPage({
        limit: pageSize,
        offset: page * pageSize,
        search: debouncedSearch || undefined,
      }),
    placeholderData: (previous) => previous,
  });

  const targets = pageQuery.data?.items ?? [];
  const total = pageQuery.data?.total ?? 0;
  const selectedIds = useMemo(() => new Set(selected.map((host) => host.id)), [selected]);
  const pageBlocked = targets.filter((target) => target.addressBound === false).length;

  const toggle = (target: ConnectionTarget) => {
    if (target.addressBound === false) return;
    if (selectedIds.has(target.id)) {
      onChange(selected.filter((host) => host.id !== target.id));
    } else {
      onChange([...selected, toSelectedHost(target)]);
    }
  };

  const remove = (id: string) => onChange(selected.filter((host) => host.id !== id));
  const clearAll = () => onChange([]);

  const cancelOperation = () => {
    runToken.current += 1;
    setOperation((prev) =>
      prev?.kind === "selectAll"
        ? { kind: "selectAll", state: "cancelled" }
        : prev?.kind === "paste"
          ? { kind: "paste", state: "cancelled" }
          : prev,
    );
  };

  const selectAllFound = async () => {
    const token = ++runToken.current;
    setOperation({ kind: "selectAll", state: "running", scanned: 0, total });
    const collected: ConnectionTarget[] = [];
    try {
      let offset = 0;
      let serverTotal = 0;
      for (;;) {
        const next = await listConnectionTargetPage({
          limit: SELECT_ALL_PAGE_SIZE,
          offset,
          search: debouncedSearch || undefined,
        });
        if (token !== runToken.current) return;
        serverTotal = next.total;
        collected.push(...next.items);
        offset += next.items.length;
        setOperation({
          kind: "selectAll",
          state: "running",
          scanned: collected.length,
          total: serverTotal,
        });
        if (offset >= serverTotal) break;
        // The server says more rows exist but returned none: fail instead of
        // presenting a partial sweep as the complete match set.
        if (next.items.length === 0) throw new Error("Empty connection target page before total");
      }
      const current = selectedRef.current;
      const next = mergeSelected(current, collected);
      onChange(next);
      const bound = collected.filter((target) => target.addressBound !== false).length;
      setOperation({
        kind: "selectAll",
        state: "done",
        added: next.length - current.length,
        blocked: collected.length - bound,
        duplicates: bound - (next.length - current.length),
      });
    } catch {
      if (token !== runToken.current) return;
      setOperation({ kind: "selectAll", state: "error" });
    }
  };

  const applyPaste = async () => {
    const tokens = parsePasteTokens(pasteText);
    if (tokens.length === 0 || tokens.length > MAX_PASTE_TOKENS) return;
    const token = ++runToken.current;
    setOperation({ kind: "paste", state: "running", done: 0, total: tokens.length });
    const matched: ConnectionTarget[] = [];
    const report = emptyReport();
    let cursor = 0;
    let done = 0;
    try {
      const worker = async () => {
        for (;;) {
          const tokenIndex = cursor++;
          const candidate = tokens[tokenIndex];
          if (candidate === undefined) return;
          const exact: ConnectionTarget[] = [];
          let offset = 0;
          for (;;) {
            const result = await listConnectionTargetPage({
              limit: PASTE_LOOKUP_LIMIT,
              offset,
              search: candidate,
            });
            if (token !== runToken.current) return;
            const serverTotal = result.total;
            for (const match of exactMatches(result.items, candidate)) {
              if (!exact.some((known) => known.id === match.id)) exact.push(match);
            }
            offset += result.items.length;
            // Two distinct exact ids are already ambiguous. Otherwise the exact
            // host can sit on any page, so walk the server search to its total.
            if (exact.length > 1 || offset >= serverTotal) break;
            if (result.items.length === 0) throw new Error("Empty connection target page before total");
          }
          if (exact.length === 0) report.unmatched.push(candidate);
          else if (exact.length > 1) report.ambiguous.push(candidate);
          else {
            const match = exact[0];
            if (match) {
              if (match.addressBound === false) report.blocked.push(candidate);
              else matched.push(match);
            }
          }
          done += 1;
          setOperation({ kind: "paste", state: "running", done, total: tokens.length });
        }
      };
      await Promise.all(
        Array.from({ length: Math.min(PASTE_CONCURRENCY, tokens.length) }, () => worker()),
      );
      if (token !== runToken.current) return;
      const current = selectedRef.current;
      const next = mergeSelected(current, matched);
      const added = next.length - current.length;
      onChange(next);
      setOperation({
        kind: "paste",
        state: "done",
        added,
        duplicates: matched.length - added,
        report,
      });
      setPasteText("");
    } catch {
      if (token !== runToken.current) return;
      // Stop the sibling lookups so no late page can be mistaken for this run.
      runToken.current += 1;
      setOperation({ kind: "paste", state: "error" });
    }
  };

  const pasteTokens = parsePasteTokens(pasteText);
  const pasteTooMany = pasteTokens.length > MAX_PASTE_TOKENS;
  const retryOperation = operation?.kind === "selectAll"
    ? () => void selectAllFound()
    : operation?.kind === "paste"
      ? () => void applyPaste()
      : undefined;

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        <span>{t("playbooks.page.hostPicker.selectedSummary", { count: selected.length })}</span>
        {selected.length > 0 && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="px-2 text-muted-foreground hover:text-foreground"
            onClick={clearAll}
          >
            {t("playbooks.page.hostPicker.clearAll")}
          </Button>
        )}
      </div>

      <div className="relative">
        <Search className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder={t("playbooks.page.hostPicker.searchPlaceholder")}
          aria-label={t("playbooks.page.hostPicker.searchPlaceholder")}
          className="pl-8 pr-10"
        />
        {search && (
          <button
            type="button"
            onClick={() => setSearch("")}
            aria-label={t("playbooks.page.hostPicker.clearSearch")}
            className="absolute right-1 top-1/2 flex size-8 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        )}
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        <span role="status" aria-live="polite">
          {pageQuery.isFetching
            ? t("playbooks.page.hostPicker.searching")
            : t("playbooks.page.hostPicker.foundSummary", { count: total })}
        </span>
        {pageBlocked > 0 && (
          <span>{t("playbooks.page.hostPicker.pageWithoutAddress", { count: pageBlocked })}</span>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => void selectAllFound()}
          disabled={running || total === 0}
        >
          {operation?.kind === "selectAll" && operation.state === "running" ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : null}
          {t("playbooks.page.hostPicker.selectAllFound", { count: total })}
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          aria-expanded={pasteOpen}
          onClick={() => setPasteOpen((prev) => !prev)}
        >
          {t("playbooks.page.hostPicker.pasteToggle")}
        </Button>
      </div>

      {pasteOpen && (
        <div className="space-y-2 rounded-md border border-border bg-background p-2">
          <label htmlFor={pasteId} className="block text-xs font-medium text-foreground">
            {t("playbooks.page.hostPicker.pasteLabel")}
          </label>
          <Textarea
            id={pasteId}
            value={pasteText}
            onChange={(e) => setPasteText(e.target.value)}
            placeholder={t("playbooks.page.hostPicker.pastePlaceholder")}
            disabled={running}
            className="min-h-24 font-mono text-xs"
          />
          <p className="text-xs text-muted-foreground">
            {t("playbooks.page.hostPicker.pasteHint", { max: MAX_PASTE_TOKENS })}
          </p>
          {pasteTooMany && (
            <p role="alert" className="text-xs text-destructive">
              {t("playbooks.page.hostPicker.pasteTooMany", {
                count: pasteTokens.length,
                max: MAX_PASTE_TOKENS,
              })}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              size="sm"
              onClick={() => void applyPaste()}
              disabled={running || pasteTokens.length === 0 || pasteTooMany}
            >
              {t("playbooks.page.hostPicker.pasteApply")}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => {
                setPasteOpen(false);
                setPasteText("");
              }}
            >
              {t("cancel")}
            </Button>
          </div>
        </div>
      )}

      {operation && (
        <div className="rounded-md border border-border bg-card px-2 py-1.5 text-xs text-muted-foreground">
          {operation.state === "running" ? (
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span role="status" aria-live="polite">
                {operation.kind === "selectAll"
                  ? t("playbooks.page.hostPicker.selectAllProgress", {
                      scanned: operation.scanned,
                      total: operation.total,
                    })
                  : t("playbooks.page.hostPicker.pasteProgress", {
                      done: operation.done,
                      total: operation.total,
                    })}
              </span>
              <Button type="button" variant="link" size="sm" className="px-0" onClick={cancelOperation}>
                {t("cancel")}
              </Button>
            </div>
          ) : operation.state === "done" && operation.kind === "selectAll" ? (
            <p role="status" aria-live="polite">
              {t("playbooks.page.hostPicker.selectAllDone", {
                added: operation.added,
                blocked: operation.blocked,
                duplicates: operation.duplicates,
              })}
            </p>
          ) : operation.state === "done" ? (
            <div role="status" aria-live="polite" className="space-y-1">
              <p>
                {t("playbooks.page.hostPicker.pasteDone", {
                  added: operation.added,
                  duplicates: operation.duplicates,
                })}
              </p>
              <ul className="space-y-0.5">
                <ReportLine
                  label={t("playbooks.page.hostPicker.reportUnmatched", {
                    count: operation.report.unmatched.length,
                  })}
                  values={operation.report.unmatched}
                />
                <ReportLine
                  label={t("playbooks.page.hostPicker.reportAmbiguous", {
                    count: operation.report.ambiguous.length,
                  })}
                  values={operation.report.ambiguous}
                />
                <ReportLine
                  label={t("playbooks.page.hostPicker.reportBlocked", {
                    count: operation.report.blocked.length,
                  })}
                  values={operation.report.blocked}
                />
              </ul>
            </div>
          ) : operation.state === "cancelled" ? (
            <p role="status" aria-live="polite">
              {t("playbooks.page.hostPicker.operationCancelled")}
            </p>
          ) : (
            <div role="alert" className="flex flex-wrap items-center justify-between gap-2">
              <span>{t("playbooks.page.hostPicker.operationError")}</span>
              {retryOperation && (
                <Button type="button" variant="link" size="sm" className="px-0" onClick={retryOperation}>
                  {t("retry")}
                </Button>
              )}
            </div>
          )}
        </div>
      )}

      <div className="max-h-[240px] space-y-1 overflow-y-auto overscroll-contain rounded-md border border-border bg-background p-2">
        {pageQuery.isPending ? (
          <p role="status" className="px-1 py-6 text-center text-sm text-muted-foreground">
            {t("loading")}
          </p>
        ) : pageQuery.isError ? (
          <div role="alert" className="px-3 py-6 text-center">
            <p className="text-sm font-medium text-foreground">{t("unknownError")}</p>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="mt-2"
              onClick={() => void pageQuery.refetch()}
            >
              {t("retry")}
            </Button>
          </div>
        ) : targets.length === 0 ? (
          <p className="px-1 py-6 text-center text-sm text-muted-foreground">
            {t("playbooks.page.hostPicker.noMatches")}
          </p>
        ) : (
          targets.map((target) => {
            const checked = selectedIds.has(target.id);
            const disabled = target.addressBound === false;
            return (
              <button
                key={target.id}
                type="button"
                aria-pressed={checked}
                disabled={disabled}
                onClick={() => toggle(target)}
                className={cn(
                  "flex min-h-9 w-full items-center gap-2 rounded-md px-2 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                  disabled
                    ? "cursor-not-allowed opacity-45"
                    : checked
                      ? "bg-accent"
                      : "hover:bg-accent",
                )}
              >
                <span
                  className={cn(
                    "flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-[4px] border",
                    checked
                      ? "border-primary bg-primary text-primary-foreground"
                      : "border-input/30",
                  )}
                >
                  {checked && <Check className="h-2.5 w-2.5" />}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium text-foreground">
                    {target.name}
                  </span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {disabled
                      ? t("playbooks.page.hostPicker.noAddress")
                      : [target.role, target.site, target.primaryIp].filter(Boolean).join(" · ") || "—"}
                  </span>
                </span>
              </button>
            );
          })
        )}
      </div>

      {total > 0 && (
        <TablePagination
          compact
          page={page}
          pageSize={pageSize}
          total={total}
          pageSizeOptions={TABLE_PAGE_SIZE_OPTIONS}
          onPageChange={(next) =>
            setPaging({ search: debouncedSearch, page: next, pageSize })
          }
          onPageSizeChange={(size) =>
            setPaging({ search: debouncedSearch, page: 0, pageSize: size })
          }
        />
      )}

      {selected.length > 0 && (
        <div className="max-h-40 overflow-y-auto overscroll-contain rounded-md border border-border bg-background p-1">
          <div className="flex flex-wrap gap-1">
            {selected.map((host) => (
              <span
                key={host.id}
                className="inline-flex min-h-9 max-w-[12rem] items-center gap-2 rounded-md border border-border bg-card px-2 py-1 text-xs text-foreground"
              >
                <span className="min-w-0 truncate" title={host.subtitle || host.name}>
                  {host.name}
                </span>
                <button
                  type="button"
                  onClick={() => remove(host.id)}
                  className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  aria-label={t("playbooks.page.hostPicker.removeNamed", { name: host.name })}
                >
                  <X className="h-3 w-3" />
                </button>
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
};

export default HostMultiSelectPicker;
