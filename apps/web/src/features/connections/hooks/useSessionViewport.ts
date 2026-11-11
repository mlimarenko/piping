import { useCallback, useEffect, useState, type RefCallback } from "react";

/** Keep touch sessions fullscreen in landscape, not just below the CSS breakpoint. */
export function useSessionTouchLayout(): boolean {
  const detect = () => typeof window !== "undefined" && typeof window.matchMedia === "function" &&
    (window.matchMedia("(max-width: 767px)").matches || window.matchMedia("(pointer: coarse)").matches);
  const [touch, setTouch] = useState(detect);
  useEffect(() => {
    const narrow = window.matchMedia("(max-width: 767px)");
    const coarse = window.matchMedia("(pointer: coarse)");
    const update = () => setTouch(narrow.matches || coarse.matches);
    narrow.addEventListener("change", update);
    coarse.addEventListener("change", update);
    return () => {
      narrow.removeEventListener("change", update);
      coarse.removeEventListener("change", update);
    };
  }, []);
  return touch;
}

/** One visible-screen frame for docked and detached mobile sessions. */
export function useSessionViewport(enabled: boolean): RefCallback<HTMLDivElement> {
  return useCallback((node: HTMLDivElement | null) => {
    if (!node || !enabled) return;
    const viewport = window.visualViewport;
    const update = () => {
      // Do not counteract browser pinch zoom; font sizing is a separate control.
      const unzoomed = !viewport || viewport.scale === 1;
      node.style.setProperty("--session-viewport-height", unzoomed
        ? `${viewport?.height ?? window.innerHeight}px` : "100dvh");
      node.style.setProperty("--session-viewport-top", unzoomed
        ? `${viewport?.offsetTop ?? 0}px` : "0px");
    };
    update();
    viewport?.addEventListener("resize", update);
    viewport?.addEventListener("scroll", update);
    window.addEventListener("resize", update);
    return () => {
      viewport?.removeEventListener("resize", update);
      viewport?.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
      node.style.removeProperty("--session-viewport-height");
      node.style.removeProperty("--session-viewport-top");
    };
  }, [enabled]);
}
