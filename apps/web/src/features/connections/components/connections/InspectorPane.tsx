import type { ReactNode } from "react";
import { Pin, PinOff } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import PaneResizer from "@/shared/ui-shared/PaneResizer";
import { cn } from "@/shared/lib/utils";

export default function InspectorPane({ width, onResize, pinned, onPin, narrow, outsideFilter, children }: Readonly<{
  width: number; onResize: (width: number) => void; pinned: boolean; onPin: () => void; narrow: boolean; children: ReactNode;
  outsideFilter?: boolean;
}>) {
  const { t } = useTranslation();
  const floating = !pinned || narrow;
  let pinLabelKey: string;
  if (narrow) pinLabelKey = "connections.workspace.floatingDetails";
  else if (pinned) pinLabelKey = "connections.workspace.unpinDetails";
  else pinLabelKey = "connections.workspace.pinDetails";
  const pinLabel = t(pinLabelKey);
  return <aside aria-label={t("connections.workspace.details")} style={{ width: width + 6, order: 2 }}
    className={cn("flex min-h-0 shrink-0", floating && "absolute inset-y-0 right-0 z-50 rounded-lg bg-background shadow-xl")}>
    <PaneResizer orientation="horizontal" reverse size={width} min={280} max={480} onResize={onResize} label={t("connections.workspace.resizeDetails")} />
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex min-h-11 shrink-0 items-center justify-between border-b border-border px-3">
        <span className="text-sm font-medium text-muted-foreground">{t("connections.workspace.details")}</span>
        <Button variant="ghost" size="icon-sm" className="size-9" onClick={onPin} disabled={narrow} aria-label={pinLabel} title={pinLabel}>
          {pinned ? <PinOff className="size-3.5" aria-hidden="true" /> : <Pin className="size-3.5" aria-hidden="true" />}
        </Button>
      </div>
      {outsideFilter && <output className="block shrink-0 border-b border-border bg-accent px-3 py-2 text-xs text-muted-foreground">{t("connections.workspace.outsideFilter")}</output>}
      {children}
    </div>
  </aside>;
}
