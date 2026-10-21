import type { PointerEvent as ReactPointerEvent } from "react";

/** One pointer-capture lifecycle for dock, file-browser and session-pane resizing. */
export function beginPanelPointerResize(
  event: ReactPointerEvent<HTMLElement>,
  axis: "x" | "y",
  onDelta: (delta: number) => void,
  onFinish: () => void,
): () => void {
  event.preventDefault();
  event.stopPropagation();
  const handle = event.currentTarget;
  const pointerId = event.pointerId;
  const start = axis === "x" ? event.clientX : event.clientY;
  handle.setPointerCapture(pointerId);
  let finished = false;
  const move = (next: PointerEvent) => {
    if (next.pointerId !== pointerId) return;
    next.preventDefault();
    next.stopImmediatePropagation();
    onDelta((axis === "x" ? next.clientX : next.clientY) - start);
  };
  const end = (next: PointerEvent) => { if (next.pointerId === pointerId) finish(); };
  const finish = () => {
    if (finished) return;
    finished = true;
    window.removeEventListener("pointermove", move, true);
    window.removeEventListener("pointerup", end, true);
    window.removeEventListener("pointercancel", end, true);
    window.removeEventListener("blur", finish);
    handle.removeEventListener("lostpointercapture", finish);
    if (handle.hasPointerCapture(pointerId)) handle.releasePointerCapture(pointerId);
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
    onFinish();
  };
  document.body.style.cursor = axis === "x" ? "ew-resize" : "ns-resize";
  document.body.style.userSelect = "none";
  window.addEventListener("pointermove", move, true);
  window.addEventListener("pointerup", end, true);
  window.addEventListener("pointercancel", end, true);
  window.addEventListener("blur", finish);
  handle.addEventListener("lostpointercapture", finish);
  return finish;
}
