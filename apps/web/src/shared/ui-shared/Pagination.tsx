import { useTranslation } from "react-i18next";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { TABLE_PAGE_SIZE_OPTIONS } from "@/shared/settings/registry";
import { Button } from "@/shared/ui-primitives/button";

export interface TablePaginationProps {
  page: number; // 0-based
  pageSize: number;
  total: number;
  pageSizeOptions?: readonly number[];
  onPageChange: (page: number) => void;
  onPageSizeChange: (size: number) => void;
}

/**
 * Server-driven pagination footer for DataTable.
 * `page` is 0-based. Renders "X–Y of Z", Prev/Next, numbered pages with
 * ellipsis, and a page-size selector.
 */
export function TablePagination({
  page,
  pageSize,
  total,
  pageSizeOptions = TABLE_PAGE_SIZE_OPTIONS,
  onPageChange,
  onPageSizeChange,
}: TablePaginationProps) {
  const { t } = useTranslation();

  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const rangeStart = total === 0 ? 0 : page * pageSize + 1;
  const rangeEnd = Math.min((page + 1) * pageSize, total);

  // Build page number list with ellipsis.
  // Always show: first, last, current, ±1 of current. Ellipsis where gaps exist.
  const pageNumbers = (() => {
    if (totalPages <= 7) {
      return Array.from({ length: totalPages }, (_, i) => i);
    }
    const pages: (number | "…")[] = [];
    const around = new Set([0, totalPages - 1, page - 1, page, page + 1].filter(p => p >= 0 && p < totalPages));
    let prev: number | null = null;
    for (const p of Array.from(around).sort((a, b) => a - b)) {
      if (prev !== null && p - prev > 1) pages.push("…");
      pages.push(p);
      prev = p;
    }
    return pages;
  })();

  return (
    <div className="flex w-full flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
      {/* Range text */}
      <span className="tabular-nums">
        {t("pagination.range", {
          start: rangeStart,
          end: rangeEnd,
          total,
          defaultValue: `{{start}}–{{end}} of {{total}}`,
        })}
      </span>

      {/* Page controls */}
      <div className="flex items-center gap-1">
        <Button
          variant="outline"
          size="sm"
          className="size-8 p-0"
          disabled={page === 0}
          onClick={() => onPageChange(page - 1)}
          aria-label={t("pagination.prev", { defaultValue: "Previous page" })}
        >
          <ChevronLeft className="h-3.5 w-3.5" />
        </Button>

        {pageNumbers.map((p, i) =>
          p === "…" ? (
            <span key={`ellipsis-${i}`} className="px-1 text-muted-foreground/50 select-none">
              …
            </span>
          ) : (
            <Button
              key={p}
              variant="outline"
              size="sm"
              className={cn(
                "h-8 min-w-8 px-2 tabular-nums",
                p === page && "pointer-events-none border-primary/30 bg-primary/10 text-primary",
              )}
              onClick={() => onPageChange(p)}
              aria-label={t("pagination.goToPage", { page: p + 1, defaultValue: `Go to page {{page}}` })}
              aria-current={p === page ? "page" : undefined}
            >
              {p + 1}
            </Button>
          ),
        )}

        <Button
          variant="outline"
          size="sm"
          className="size-8 p-0"
          disabled={page >= totalPages - 1}
          onClick={() => onPageChange(page + 1)}
          aria-label={t("pagination.next", { defaultValue: "Next page" })}
        >
          <ChevronRight className="h-3.5 w-3.5" />
        </Button>
      </div>

      {/* Page-size selector */}
      <div className="flex items-center gap-1.5">
        <span>{t("pagination.perPage", { defaultValue: "Per page:" })}</span>
        <select
          value={pageSize}
          onChange={(e) => onPageSizeChange(Number(e.target.value))}
          className="h-8 rounded-md border border-input bg-card px-2 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          aria-label={t("pagination.perPageLabel", { defaultValue: "Rows per page" })}
        >
          {pageSizeOptions.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
}
