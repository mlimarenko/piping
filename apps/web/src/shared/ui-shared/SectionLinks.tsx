import type { ComponentType, ReactNode } from "react";
import { Link, type To } from "react-router";
import { cn } from "@/shared/lib/utils";

export interface SectionLinkItem<TValue extends string = string> {
  value: TValue;
  label: string;
  to: To;
  icon?: ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  badge?: ReactNode;
}

interface SectionLinksProps<TValue extends string> {
  items: readonly SectionLinkItem<TValue>[];
  activeValue: TValue;
  label: string;
  className?: string;
  onNavigate?: (value: TValue) => void;
}

/** URL sections use links; in-page panels use SectionTabs with TabsContent. */
export default function SectionLinks<TValue extends string>({
  items, activeValue, label, className, onNavigate,
}: SectionLinksProps<TValue>) {
  return (
    <nav aria-label={label} data-slot="section-links"
      className={cn("section-tab-list", className)}>
      {items.map(({ value, label: title, to, icon: Icon, badge }) => (
        <Link
          key={value}
          to={to}
          aria-current={value === activeValue ? "page" : undefined}
          data-slot="section-link"
          data-state={value === activeValue ? "active" : "inactive"}
          className="section-tab focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
          onClick={onNavigate ? (event) => {
            if (!event.defaultPrevented && event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey) {
              onNavigate(value);
            }
          } : undefined}
        >
          {Icon ? <Icon className="size-4 shrink-0" aria-hidden /> : null}
          <span>{title}</span>
          {badge != null ? <>{" "}<span data-slot="section-link-badge" className="shrink-0 text-xs tabular-nums">{badge}</span></> : null}
        </Link>
      ))}
    </nav>
  );
}
