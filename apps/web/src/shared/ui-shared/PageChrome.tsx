import { useId, useLayoutEffect, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Info, type LucideIcon } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui-primitives/popover";
import { cn } from "@/shared/lib/utils";
import { usePageChromeHosts, usePageChromeRegistration } from "./pageChromeContext";

interface PageChromeProps {
  title: ReactNode;
  description?: ReactNode;
  icon?: LucideIcon;
  actions?: ReactNode;
  badge?: string;
  className?: string;
  visible?: boolean;
}

/** A routed page's sole heading and actions live in the application header. */
export default function PageChrome({ title, description, icon: Icon, actions, badge, className, visible = true }: PageChromeProps) {
  const id = useId();
  const { claim } = usePageChromeRegistration();
  const { navigationToken, owner, titleHost, actionsHost } = usePageChromeHosts();
  useLayoutEffect(() => visible ? claim({ id, navigationToken }) : undefined, [claim, id, navigationToken, visible]);
  if (!visible || owner?.id !== id || owner.navigationToken !== navigationToken) return null;
  return <>
    {titleHost && createPortal(<div data-slot="page-chrome" className={cn("flex min-w-0 flex-wrap items-center gap-2", className)}>
      {Icon && <Icon className="size-4 shrink-0 text-primary" aria-hidden="true" />}
      <h1 data-slot="app-header-title" className="min-w-0 break-words text-base font-semibold tracking-tight text-foreground [overflow-wrap:anywhere]">{title}</h1>
      {badge && <span data-slot="page-chrome-badge" className="max-w-full break-words rounded-md border border-border bg-muted px-2 py-0.5 text-xs text-muted-foreground">{badge}</span>}
      {description && <Popover><PopoverTrigger asChild><Button variant="ghost" size="icon-sm" aria-label={typeof description === "string" ? description : "Page information"}><Info className="size-4" aria-hidden="true" /></Button></PopoverTrigger>
        <PopoverContent className="max-w-[calc(100vw-2rem)] text-sm" align="start">{description}</PopoverContent></Popover>}
    </div>, titleHost)}
    {actionsHost && actions && createPortal(<div data-slot="page-actions" className="flex min-w-0 max-w-full flex-wrap items-center gap-2 [&>*]:max-w-full [&>*]:flex-wrap">{actions}</div>, actionsHost)}
  </>;
}

/** Active child panels contribute actions without replacing their workspace title. */
export function PageActions({ children, visible = true }: { children: ReactNode; visible?: boolean }) {
  const { actionsHost } = usePageChromeHosts();
  return visible && actionsHost ? createPortal(<div data-slot="page-actions" className="flex min-w-0 max-w-full flex-wrap items-center gap-2 [&>*]:max-w-full [&>*]:flex-wrap">{children}</div>, actionsHost) : null;
}
