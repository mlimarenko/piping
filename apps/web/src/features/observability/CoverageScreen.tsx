import { useMemo, useState } from "react";
import { Link } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Radio, RefreshCw } from "lucide-react";

import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import EmptyState from "@/shared/ui-shared/EmptyState";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import { cn } from "@/shared/lib/utils";
import { getMonitoringCoverage } from "./api";
import {
  coverageHostHref,
  stateBadgeClass,
  type CoverageHost,
  type CoverageResponse,
  type CoverageState,
  type CoverageTarget,
  type CoverageTotals,
} from "./coverage";
import { COVERAGE_KEY } from "./keys";

const STATE_FILTERS: CoverageState[] = ["covered", "no_scrape", "duplicate", "filtered"];

function filterCount(totals: CoverageTotals, state: CoverageState): number {
  return totals[state];
}

function targetSummary(target: CoverageTarget): string {
  const parts = [target.job, target.assignment_name ?? target.exporter_slug, target.target];
  return parts.filter((part): part is string => Boolean(part)).join(" · ");
}

function TotalsTile({
  label,
  value,
  active,
  onClick,
}: {
  label: string;
  value: number;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        "rounded-md border p-2 text-left transition-colors focus-visible:ring-2 focus-visible:ring-ring",
        active
          ? "border-primary/50 bg-primary/5"
          : "border-border bg-background hover:bg-accent",
      )}
    >
      <span className="block text-xs uppercase text-muted-foreground">{label}</span>
      <span className="mt-1 block text-lg font-semibold tabular-nums">{value}</span>
    </button>
  );
}

/**
 * Fleet view of exporter-assignment coverage.
 *
 * The backend computes it from the same discovery read model as
 * `/api/v1/monitoring/sd/preview`, so every target shown here also appears in
 * the discovery output with the same job and labels. State filtering happens in
 * the browser because the coverage states are derived from the resolved fleet,
 * not stored per host.
 */
export default function CoverageScreen() {
  const { t } = useTranslation();
  const pageSize = useTablePageSize();
  const [term, setTerm] = useState("");
  const [state, setState] = useState<CoverageState | null>(null);
  const [page, setPage] = useState(0);

  const query = useQuery({
    queryKey: [...COVERAGE_KEY, "fleet", term],
    queryFn: () => getMonitoringCoverage(term ? { search: term } : {}),
    placeholderData: (previous) => previous,
  });

  const data: CoverageResponse | undefined = query.data;
  const totals = data?.totals;
  const hosts = useMemo(() => {
    const rows = data?.hosts ?? [];
    return state ? rows.filter((host) => host.state === state) : rows;
  }, [data, state]);

  const currentPage = Math.min(page, Math.max(0, Math.ceil(hosts.length / pageSize.value) - 1));

  const columns: Column<CoverageHost>[] = [
    {
      key: "host",
      label: t("observability.coverage.host", { defaultValue: "Host" }),
      render: (host) => (
        <Link
          to={coverageHostHref(host)}
          className="flex min-w-0 flex-col underline-offset-2 hover:underline focus-visible:ring-2 focus-visible:ring-ring"
        >
          <span className="break-all text-sm font-medium text-foreground">{host.host_name}</span>
          <span className="text-xs text-muted-foreground">
            {t(`observability.coverage.kind.${host.kind}`, { defaultValue: host.kind })}
            {host.status ? ` · ${host.status}` : ""}
          </span>
        </Link>
      ),
    },
    {
      key: "state",
      label: t("observability.coverage.state", { defaultValue: "Coverage" }),
      render: (host) => (
        <Badge variant="outline" className={stateBadgeClass(host.state)}>
          {t(`observability.monitoring.state.${host.state}`, { defaultValue: host.state })}
        </Badge>
      ),
    },
    {
      key: "reason",
      label: t("observability.coverage.reason", { defaultValue: "Why not scraped" }),
      className: "hidden md:table-cell whitespace-normal",
      render: (host) =>
        host.reason
          ? t(`observability.monitoring.reason.${host.reason}`, { defaultValue: host.reason })
          : "—",
    },
    {
      key: "scrapes",
      label: t("observability.monitoring.scrapes", { defaultValue: "Scrapes" }),
      className: "hidden sm:table-cell text-right tabular-nums",
      render: (host) => host.scrapes,
    },
    {
      key: "targets",
      label: t("observability.monitoring.targets", { defaultValue: "Targets" }),
      className: "hidden lg:table-cell whitespace-normal",
      render: (host) =>
        host.targets.length === 0 ? (
          <span className="text-muted-foreground">
            {t("observability.coverage.noTargets", { defaultValue: "No targets" })}
          </span>
        ) : (
          <ul className="grid gap-0.5">
            {host.targets.map((target) => (
              <li key={`${target.assignment_id}-${target.target}`} className="break-all text-xs">
                {targetSummary(target)}
              </li>
            ))}
          </ul>
        ),
    },
  ];

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-4">
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-5">
        <TotalsTile
          label={t("observability.coverage.hostsTotal", { defaultValue: "Active hosts" })}
          value={totals?.hosts_total ?? 0}
          active={state === null}
          onClick={() => {
            setState(null);
            setPage(0);
          }}
        />
        {STATE_FILTERS.map((value) => (
          <TotalsTile
            key={value}
            label={t(`observability.monitoring.state.${value}`, { defaultValue: value })}
            value={totals ? filterCount(totals, value) : 0}
            active={state === value}
            onClick={() => {
              setState(state === value ? null : value);
              setPage(0);
            }}
          />
        ))}
      </div>

      <DataTable
        tableId="observability.coverage"
        columns={columns}
        data={hosts.slice(currentPage * pageSize.value, (currentPage + 1) * pageSize.value)}
        getRowKey={(host) => host.host_id}
        loading={query.isPending}
        error={
          query.isError
            ? t("observability.coverage.loadFailed", {
                defaultValue: "Could not load host coverage. Retry when the API is reachable.",
              })
            : undefined
        }
        onRetry={() => query.refetch()}
        emptyState={
          <EmptyState
            icon={Radio}
            title={t("observability.coverage.empty", { defaultValue: "No hosts in this view" })}
            description={t("observability.coverage.emptyHint", {
              defaultValue:
                "No active host matches the filter. Clear the filter or check the inventory status.",
            })}
          />
        }
        toolbar={
          <>
            <SearchInput
              placeholder={t("search")}
              value={term}
              onValueChange={(value) => {
                setTerm(value);
                setPage(0);
              }}
            />
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t("observability.coverage.refresh", { defaultValue: "Refresh coverage" })}
              disabled={query.isFetching}
              onClick={() => query.refetch()}
            >
              <RefreshCw className={query.isFetching ? "size-4 animate-spin" : "size-4"} />
            </Button>
          </>
        }
        pagination={
          data
            ? {
                page: currentPage,
                pageSize: pageSize.value,
                total: hosts.length,
                onPageChange: (next) => setPage(next),
                onPageSizeChange: (next) => {
                  pageSize.setValue(next);
                  setPage(0);
                },
              }
            : undefined
        }
      />
    </div>
  );
}
