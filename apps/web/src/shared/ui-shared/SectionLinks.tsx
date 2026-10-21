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
  variant?: "line" | "secondary";
  wrap?: boolean;
  onNavigate?: (value: TValue) => void;
}

/** URL sections use links; in-page panels use SectionTabs with TabsContent. */
export default function SectionLinks<TValue extends string>({
  items, activeValue, label, className, variant = "line", wrap = false, onNavigate,
}: SectionLinksProps<TValue>) {
  return (
    <nav aria-label={label} data-slot="section-links" data-variant={variant}
      className={cn("section-tab-list", wrap && "flex-wrap overflow-visible", className)}>
      {items.map(({ value, label: title, to, icon: Icon, badge }) => (
        <Link
          key={value}
          to={to}
          aria-current={value === activeValue ? "page" : undefined}
          data-slot="section-link"
          data-state={value === activeValue ? "active" : "inactive"}
          className={cn("section-tab focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
            wrap && "h-auto max-w-full whitespace-normal py-2 text-left",
            wrap && (variant === "secondary" ? "min-h-9" : "min-h-10"))}
          onClick={onNavigate ? (event) => {
            if (!event.defaultPrevented && event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey) {
              onNavigate(value);
            }
          } : undefined}
        >
          {Icon ? <Icon className="size-4 shrink-0" aria-hidden /> : null}
          <span className="min-w-0 break-words">{title}</span>
          {badge != null ? <>{" "}<span data-slot="section-link-badge" className="shrink-0 text-xs tabular-nums">{badge}</span></> : null}
        </Link>
      ))}
    </nav>
  );
}
