import { useCallback, useLayoutEffect, useRef, type RefObject } from "react";

function eligible(element: HTMLElement | null): element is HTMLElement {
  if (!element?.isConnected || element.closest('[hidden], [inert], [aria-hidden="true"]') || element.matches(":disabled, [aria-disabled='true']")) return false;
  if (element.checkVisibility && !element.checkVisibility({ opacityProperty: true, visibilityProperty: true })) return false;
  for (let node: HTMLElement | null = element; node; node = node.parentElement) {
    const style = getComputedStyle(node);
    if (style.display === "none" || style.visibility === "hidden" || style.visibility === "collapse" || style.opacity === "0" || style.contentVisibility === "hidden") return false;
  }
  return true;
}

export function useCollectionPreviewFocus(anchor: RefObject<HTMLElement | null>, active = true) {
  const opener = useRef<HTMLElement | null>(null);
  const allowed = useRef(active);
  const suppressed = useRef(false);
  useLayoutEffect(() => {
    allowed.current = active;
    if (!active) opener.current = null;
    return () => { allowed.current = false; opener.current = null; };
  }, [active]);
  const capture = useCallback((element: HTMLElement) => {
    opener.current = element;
    suppressed.current = false;
  }, []);
  const suppress = useCallback(() => { suppressed.current = true; opener.current = null; }, []);
  const onCloseAutoFocus = useCallback((event: Event) => {
    event.preventDefault();
    const target = opener.current;
    opener.current = null;
    if (!allowed.current || suppressed.current) return;
    const focused = document.activeElement;
    if (focused instanceof HTMLElement && focused !== document.body && eligible(focused) &&
        !(event.target instanceof HTMLElement && event.target.contains(focused))) return;
    for (const candidate of [target, anchor.current]) {
      if (!eligible(candidate)) continue;
      candidate.focus({ preventScroll: true });
      if (document.activeElement === candidate) break;
    }
  }, [anchor]);
  return { capture, suppress, onCloseAutoFocus };
}
