import { useEffect, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Sheet, SheetContent, SheetTitle } from "@/shared/ui-primitives/sheet";
import PaneResizer from "@/shared/ui-shared/PaneResizer";

export default function InspectorPane({ width, onResize, overlay, mobile = false, onClose, title, outsideFilter, children }: Readonly<{
  width: number;
  onResize: (width: number) => void;
  overlay: boolean;
  mobile?: boolean;
  onClose: () => void;
  title: string;
  outsideFilter?: boolean;
  children: ReactNode;
}>) {
  const { t } = useTranslation();
  const inlineRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!overlay && !mobile) inlineRef.current?.focus();
  }, [overlay, mobile, title]);
  const content = <>
    {outsideFilter && <output className="block shrink-0 border-b border-border bg-accent px-3 py-2 text-xs text-muted-foreground">{t("connections.workspace.outsideFilter")}</output>}
    {children}
  </>;
  if (overlay || mobile) {
    return <Sheet open onOpenChange={open => { if (!open) onClose(); }}>
      <SheetContent side={mobile ? "bottom" : "right"} hideClose
        aria-describedby={undefined}
        onCloseAutoFocus={event => event.preventDefault()}
        className={mobile ? "flex h-[85dvh] min-h-0 flex-col gap-0 overflow-hidden p-0" : "flex h-full w-[min(100vw,30rem)] min-h-0 max-w-none flex-col gap-0 overflow-hidden p-0 sm:max-w-none"}>
        <SheetTitle className="sr-only">{title}</SheetTitle>
        {content}
      </SheetContent>
    </Sheet>;
  }
  return <aside ref={inlineRef} tabIndex={-1} aria-label={title} style={{ width, order: 2 }}
    className="relative flex min-h-0 shrink-0"
    onKeyDown={event => {
      if (event.key === "Escape" && !event.defaultPrevented && event.currentTarget.contains(event.target as Node)) {
        event.stopPropagation();
        onClose();
      }
    }}>
    <PaneResizer orientation="horizontal" reverse size={width} min={280} max={480} onResize={onResize} label={t("connections.workspace.resizeDetails")} className="absolute inset-y-0 -left-2 w-2" />
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-md border border-border bg-card text-card-foreground">{content}</div>
  </aside>;
}
