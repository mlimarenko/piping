import { useState } from "react";
import { Link } from "react-router";
import { AlertTriangle, Check, Copy, Loader2, Plus } from "lucide-react";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { Skeleton } from "@/shared/ui-primitives/skeleton";
import { writeBrowserClipboardText } from "@/shared/lib/clipboard";
import { cn } from "@/shared/lib/utils";
import InspectorSection from "./InspectorSection";

type Translate = (k: string, opts?: Record<string, unknown>) => string;

/* ── Row ── */

interface RelatedEntityRowProps {
  /** Lucide icon identifying the entity type. */
  icon: React.ElementType;
  /** Primary label — entity name. Always present. */
  label: string;
  /** Secondary field 1 — muted, truncated (role, type, status …). */
  secondary1?: string;
  /** Secondary field 2 — muted, truncated (IP, enabled/disabled …). */
  secondary2?: string;
  /** Inventory deep-link. When present (and no `onActivate`) the row/label is a
   *  `<Link>`; absent → plain. */
  href?: string | null;
  /** When provided, the clickable element calls this (opens the in-workspace edit
   *  form) instead of navigating to `href`. Takes precedence over `href`. */
  onActivate?: () => void;
  /** Link coverage. `row` keeps the legacy whole-row link; `label` leaves details interactive/selectable. */
  linkScope?: "row" | "label";
  /** Accessible label for the row (defaults to label). */
  ariaLabel?: string;
  /** Optional compact multiline details rendered under the primary row text. */
  details?: React.ReactNode;
}

/**
 * One related-entity row: a small type icon, a primary label, and up to two
 * muted secondary fields. By default, rows with `href` are a single navigation
 * target. `linkScope="label"` keeps only the primary label clickable so details
 * can contain selectable text or small inline controls.
 */
export const RelatedEntityRow = ({
  icon: Icon,
  label,
  secondary1,
  secondary2,
  href,
  onActivate,
  linkScope = "row",
  ariaLabel,
  details,
}: RelatedEntityRowProps) => {
  const secondary = [secondary1, secondary2].filter(Boolean).join(" · ");
  const labelLinkClass =
    "min-w-0 flex-1 truncate rounded-sm text-left text-xs font-medium text-foreground hover:text-primary hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
  const labelNode = linkScope === "label" && onActivate ? (
    <button type="button" onClick={onActivate} className={labelLinkClass} aria-label={ariaLabel ?? label}>
      {label}
    </button>
  ) : href && linkScope === "label" ? (
    <Link to={href} className={labelLinkClass} aria-label={ariaLabel ?? label}>
      {label}
    </Link>
  ) : (
    <span className="min-w-0 flex-1 truncate text-xs font-medium text-foreground">{label}</span>
  );
  const inner = (
    <div className={cn("flex min-w-0 gap-2 py-1.5", details ? "items-start" : "items-center")}>
      <Icon className={cn("h-3 w-3 shrink-0 text-muted-foreground", details && "mt-0.5 text-primary")} />
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-start gap-2">
          {labelNode}
          {secondary && (
            <span className="max-w-[42%] shrink-0 truncate text-right text-xs text-muted-foreground">
              {secondary}
            </span>
          )}
        </div>
        {details && <div className="mt-1.5 space-y-1">{details}</div>}
      </div>
    </div>
  );

  const baseClass =
    "w-full rounded-sm border-b border-border/10 last:border-0 transition-colors";
  const rowLinkClass = cn(
    "block",
    baseClass,
    "hover:bg-accent focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
  );

  if (linkScope === "row" && onActivate) {
    return (
      <button type="button" onClick={onActivate} className={cn(rowLinkClass, "text-left")} aria-label={ariaLabel ?? label}>
        {inner}
      </button>
    );
  }
  if (href && linkScope === "row") {
    return (
      <Link to={href} className={rowLinkClass} aria-label={ariaLabel ?? label}>
        {inner}
      </Link>
    );
  }
  return <div className={cn(baseClass, "cursor-default")}>{inner}</div>;
};

export interface CopyableDetailRowProps {
  icon: React.ElementType;
  value: string;
  copyLabel: string;
  copiedLabel: string;
  textClassName?: string;
  onCopyUnavailable?: () => void;
}

export const CopyableDetailRow = ({
  icon: Icon,
  value,
  copyLabel,
  copiedLabel,
  textClassName,
  onCopyUnavailable,
}: CopyableDetailRowProps) => {
  const [copied, setCopied] = useState(false);

  const copyValue = (event: React.MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
    void writeBrowserClipboardText(value).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      },
      () => onCopyUnavailable?.(),
    );
  };

  const ButtonIcon = copied ? Check : Copy;

  return (
    <div
      className="grid grid-cols-[1rem_minmax(0,1fr)_2.25rem] items-start gap-1.5 text-sm leading-5 text-muted-foreground"
      data-contact-detail-row
    >
      <Icon className="mt-0.5 size-4 text-muted-foreground/80" aria-hidden="true" />
      <span className={cn("min-w-0 cursor-text select-text break-words text-foreground/85", textClassName)}>
        {value}
      </span>
      <button
        type="button"
        className={cn(
          "inline-flex size-9 items-center justify-center rounded-sm text-muted-foreground transition-colors",
          "hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
          copied && "text-primary",
        )}
        aria-label={copied ? copiedLabel : copyLabel}
        title={copied ? copiedLabel : copyLabel}
        onClick={copyValue}
      >
        <ButtonIcon className="h-3 w-3" aria-hidden="true" />
      </button>
    </div>
  );
};

/* ── Body states ── */

const RelationSkeleton = () => (
  <div className="space-y-1.5 py-0.5">
    {[0, 1].map((i) => (
      <div key={i} className="flex items-center gap-2 py-1">
        <Skeleton className="h-3 w-3 shrink-0 rounded" />
        <Skeleton className="h-3 flex-1 rounded" />
        <Skeleton className="h-3 w-14 rounded" />
      </div>
    ))}
  </div>
);

const RelationError = ({ message }: { message: string }) => (
  <div className="flex items-center gap-1.5 py-1 text-xs text-status-negative">
    <AlertTriangle className="h-3 w-3 shrink-0" />
    <span>{message}</span>
  </div>
);

/**
 * Empty body for a relation section: plain muted text only. The add affordance is
 * the section header "+", so a second button here would be a duplicate control
 * with an identical accessible name — there is exactly one "+" per section.
 */
const RelationEmpty = ({ text }: { text: string }) => (
  <p className="py-1 text-xs text-muted-foreground">{text}</p>
);

/* ── Section ── */

export interface RelatedEntitySectionProps<T extends { id: string }> {
  /** Section title (already translated by the caller). */
  title: string;
  /** Lucide icon component for the section header. */
  icon: React.ElementType;
  /** Unique key for expand/collapse state (matches the InspectorSection pattern). */
  sectionKey: string;
  /** Shared expand/collapse state array + toggle from the parent inspector. */
  expandedSections: string[];
  onToggle: (key: string) => void;
  /** The data items for this relation. */
  items: T[];
  /** True while the relation query is in-flight. */
  loading: boolean;
  /** Non-null if the query errored. */
  error?: Error | null;
  /** Renders a single RelatedEntityRow from an item T. */
  renderRow: (item: T) => React.ReactNode;
  /**
   * When provided, a "+" button appears in the section header. Absent → no "+"
   * (e.g. entity UUID not yet resolved, or read-only relation).
   */
  onAdd?: () => void;
  /** i18n label for the "+" button aria-label/title. */
  addLabel?: string;
  /** i18n string shown when items is empty and not loading. */
  emptyText: string;
  /**
   * Count for the badge. Defaults to items.length. The eager-count flow passes the
   * resolved `total` here (or `undefined` until it resolves, paired with
   * `countLoading`) so the badge shows the real number without expanding the
   * section.
   */
  count?: number;
  /**
   * True while the eager count is still resolving. The badge then shows a subtle
   * spinner placeholder instead of a premature "0".
   */
  countLoading?: boolean;
  /** Translate fn for the shared error string. */
  t: Translate;
}

/**
 * Standardised inspector relation: a collapsible section (header = title + count
 * badge + a single "+" add button; body = related-entity rows) used uniformly
 * for every attached collection — contacts, interfaces, IP addresses, devices at
 * a site, etc. Loading shows a skeleton, empty shows muted text (+ optional "+"),
 * and a query error shows a muted inline error. There is no detach/delete affordance
 * anywhere in this component by design.
 */
const RelatedEntitySection = <T extends { id: string }>({
  title,
  icon,
  sectionKey,
  expandedSections,
  onToggle,
  items,
  loading,
  error,
  renderRow,
  onAdd,
  addLabel,
  emptyText,
  count,
  countLoading = false,
  t,
}: RelatedEntitySectionProps<T>) => (
  <InspectorSection
    title={
      <span className="flex items-center gap-2">
        {title}
        <Badge variant="secondary" className="h-4 px-1.5 text-xs">
          {/* Three states: count still loading → subtle spinner (never "0");
              resolved → the eager `count` (or items.length when no count given). */}
          {countLoading ? (
            <Loader2 className="h-2.5 w-2.5 animate-spin" aria-hidden="true" />
          ) : (
            count ?? items.length
          )}
        </Badge>
      </span>
    }
    icon={icon}
    sectionKey={sectionKey}
    expanded={expandedSections}
    onToggle={onToggle}
    action={
      onAdd ? (
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="h-9 w-6 text-muted-foreground hover:text-primary"
          onClick={onAdd}
          aria-label={addLabel}
          title={addLabel}
        >
          <Plus className="h-3.5 w-3.5" />
        </Button>
      ) : undefined
    }
  >
    {loading ? (
      <RelationSkeleton />
    ) : error ? (
      <RelationError message={t("connections.inspector.relations.error")} />
    ) : items.length === 0 ? (
      <RelationEmpty text={emptyText} />
    ) : (
      <div className="space-y-px">{items.map(renderRow)}</div>
    )}
  </InspectorSection>
);

export default RelatedEntitySection;
