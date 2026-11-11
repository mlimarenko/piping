import { useTranslation } from "react-i18next";
import { useLayoutEffect, useRef, type RefObject } from "react";
import { Button } from "@/shared/ui-primitives/button";
import type { CustomMapPoint } from "./CustomMap";

/** Disambiguation for markers that cannot be separated by zooming. */
export function MapGroupList({ points, onSelect, onClose, fallbackFocusRef }: {
  points: readonly CustomMapPoint[];
  onSelect?: (id: string | null) => void;
  onClose: () => void;
  fallbackFocusRef: RefObject<HTMLElement | null>;
}) {
  const { t } = useTranslation();
  const sectionRef = useRef<HTMLElement>(null);
  const firstMemberRef = useRef<HTMLButtonElement>(null);
  useLayoutEffect(() => {
    const current = document.activeElement;
    const returnTo = current && current !== document.body && "focus" in current ? current as HTMLElement | SVGElement : fallbackFocusRef.current;
    const section = sectionRef.current;
    firstMemberRef.current?.focus();
    return () => {
      if (section?.contains(document.activeElement) && returnTo?.isConnected) returnTo.focus();
    };
  }, [fallbackFocusRef]);
  return (
    <section ref={sectionRef} onKeyDown={event => { if (event.key === "Escape") { event.stopPropagation(); onClose(); } }} aria-label={t("inventory.map.clusterMembers", { defaultValue: "Locations in this group" })} className="absolute left-3 top-3 z-10 max-h-[70%] max-w-[calc(100%-1.5rem)] overflow-auto rounded-md border border-border bg-popover p-2 text-popover-foreground shadow-md">
      <div className="flex items-center justify-between gap-4 px-2 py-1 text-sm font-medium">
        <span>{t("inventory.map.clusterMembers", { defaultValue: "Locations in this group" })}</span>
        <Button type="button" variant="ghost" size="lg" onClick={onClose}>{t("inventory.map.closeCluster", { defaultValue: "Close" })}</Button>
      </div>
      <ul>{points.map((point, index) => <li key={point.id}><Button ref={index === 0 ? firstMemberRef : undefined} type="button" variant="ghost" size="lg" className="h-auto min-h-11 w-full justify-start whitespace-normal break-words text-left [overflow-wrap:anywhere]" title={point.label ?? point.id} onClick={() => { onSelect?.(point.id); onClose(); }}>{point.label ?? point.id}</Button></li>)}</ul>
    </section>
  );
}
