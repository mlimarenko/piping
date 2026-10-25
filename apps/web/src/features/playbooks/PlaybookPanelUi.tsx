import { useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, Eye } from "lucide-react";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";

import { Badge } from "@/shared/ui-primitives/badge";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/shared/ui-primitives/collapsible";
import SelectorChips from "@/features/playbooks/SelectorChips";
import type {
  PlaybookRisk,
  PlaybookStatus,
  PlaybookTargetSetPreview,
} from "@/features/playbooks/api";

export function PlaybookTabPanel({
  active,
  children,
}: Readonly<{
  active: boolean;
  children: React.ReactNode;
}>) {
  if (!active) return null;
  return <div className="mt-0 min-h-0 flex-1">{children}</div>;
}

export function PreviewActions({ name, onOpen, children }: Readonly<{
  name: string;
  onOpen: (trigger: HTMLButtonElement) => void;
  children?: ReactNode;
}>) {
  const { t } = useTranslation();
  const trigger = useRef<HTMLButtonElement>(null);
  const pending = useRef(false);
  return <RowActions ref={trigger} label={`${t("actions")}: ${name}`} contentProps={{
    onCloseAutoFocus: event => {
      if (!pending.current) return;
      pending.current = false;
      event.preventDefault();
      if (trigger.current?.isConnected) onOpen(trigger.current);
    },
  }}>
    {children}
    <DropdownMenuItem onSelect={() => { pending.current = true; }}>
      <Eye className="size-4" />{t("playbooks.page.actions.details")}
    </DropdownMenuItem>
  </RowActions>;
}

export function RiskBadge({ risk }: Readonly<{ risk: PlaybookRisk }>) {
  const { t } = useTranslation();
  let variant: "status-active" | "status-staging" | "status-failed" = "status-failed";
  if (risk === "low") variant = "status-active";
  else if (risk === "medium") variant = "status-staging";
  return (
    <Badge variant={variant} className="rounded-md">
      {t(`playbooks.page.risk.${risk}`)}
    </Badge>
  );
}

export function StatusBadge({ status }: Readonly<{ status: PlaybookStatus }>) {
  const { t } = useTranslation();
  let variant: "status-active" | "status-neutral" | "status-offline" = "status-offline";
  if (status === "active") variant = "status-active";
  else if (status === "draft") variant = "status-neutral";
  return (
    <Badge variant={variant} className="rounded-md">
      {t(`playbooks.page.status.${status}`)}
    </Badge>
  );
}

export function TargetPreview({ preview }: Readonly<{ preview: PlaybookTargetSetPreview }>) {
  const { t } = useTranslation();
  return (
    <>
      <div className="grid grid-cols-3 gap-2">
        <MetricPill label={t("playbooks.page.preview.total")} value={String(preview.total)} />
        <MetricPill label={t("playbooks.page.preview.shown")} value={String(preview.items.length)} />
        <MetricPill label={t("playbooks.page.preview.truncated")} value={preview.truncated ? t("yes") : t("no")} />
      </div>
      <SelectorChips
        selector={preview.target_set.selector}
        maxPerFacet={4}
        className="block rounded-md border border-border bg-background p-2"
      />
      <Collapsible>
        <CollapsibleTrigger className="group flex min-h-9 w-full items-center gap-2 rounded-md px-2 py-2 text-left text-sm font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground">
          <ChevronDown className="h-3.5 w-3.5 shrink-0 transition-transform group-data-[state=closed]:-rotate-90" />
          {t("playbooks.page.preview.rawSelector")}
        </CollapsibleTrigger>
        <CollapsibleContent>
          <JsonPreview value={preview.target_set.selector} />
        </CollapsibleContent>
      </Collapsible>
      <div className="max-h-[24rem] space-y-2 overflow-auto pr-1">
        {preview.items.map((item) => (
          <div key={item.target_id} className="rounded-md border border-border bg-background p-2">
            <div className="flex items-center justify-between gap-2">
              <span className="truncate text-sm font-medium text-foreground">{item.name}</span>
              <span className="shrink-0 rounded-md bg-muted px-2 py-1 text-xs text-muted-foreground">
                {item.target_kind}
              </span>
            </div>
            <div className="mt-1 flex flex-wrap gap-2 text-xs text-muted-foreground">
              <span>{item.address || "—"}</span>
              <span>{item.site_name || "—"}</span>
              <span>{item.role_name || "—"}</span>
            </div>
          </div>
        ))}
      </div>
    </>
  );
}

export function JsonPreview({ value }: Readonly<{ value: unknown }>) {
  return (
    <pre className="max-h-40 overflow-auto rounded-md border border-border bg-background p-2 font-mono text-sm leading-relaxed text-muted-foreground">
      {JSON.stringify(value, null, 2)}
    </pre>
  );
}

export function MetricPill({ label, value }: Readonly<{ label: string; value: string }>) {
  return (
    <div className="rounded-md border border-border bg-background p-2">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className="mt-1 text-lg font-semibold text-foreground">{value}</div>
    </div>
  );
}
