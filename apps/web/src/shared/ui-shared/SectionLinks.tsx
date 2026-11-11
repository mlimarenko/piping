import { useLayoutEffect, useRef, type ComponentType, type ReactNode } from "react";
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
  revealActive?: boolean;
}

/** URL sections use links; in-page panels use SectionTabs with TabsContent. */
export default function SectionLinks<TValue extends string>({
  items, activeValue, label, className, onNavigate, revealActive = true,
}: SectionLinksProps<TValue>) {
  const navRef = useRef<HTMLElement>(null);
  const following = useRef(true);
  const expectedScroll = useRef(0);
  const membership = JSON.stringify(items.map(item => item.value));
  useLayoutEffect(() => {
    if (!revealActive) return;
    const nav = navRef.current;
    const active = nav?.querySelector<HTMLElement>('[aria-current="page"]');
    if (!nav || !active) return;
    following.current = true;
    const reveal = () => {
      if (!following.current) return;
      const navRect = nav.getBoundingClientRect();
      const activeRect = active.getBoundingClientRect();
      if (activeRect.left < navRect.left) nav.scrollLeft += activeRect.left - navRect.left;
      else if (activeRect.right > navRect.right) nav.scrollLeft += activeRect.right - navRect.right;
      expectedScroll.current = nav.scrollLeft;
    };
    reveal();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(reveal);
    observer.observe(nav);
    nav.querySelectorAll<HTMLElement>('[data-slot="section-link"]').forEach(link => observer.observe(link));
    return () => observer.disconnect();
  }, [activeValue, revealActive, membership]);
  return (
    <nav ref={navRef} aria-label={label} data-slot="section-links"
      onScroll={(event) => {
        // Follow late layout changes until the user scrolls away; a new selection resumes following.
        if (Math.abs(event.currentTarget.scrollLeft - expectedScroll.current) > 1) following.current = false;
      }}
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
