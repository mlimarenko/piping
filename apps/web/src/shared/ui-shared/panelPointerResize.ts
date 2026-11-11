import type { PointerEvent as ReactPointerEvent } from "react";

/** Keep remote viewers from consuming drag input, including legacy mouse events. */
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
  const previousCursor = document.body.style.cursor;
  const previousUserSelect = document.body.style.userSelect;
  const cursor = axis === "x" ? "ew-resize" : "ns-resize";
  const shield = document.createElement("div");
  shield.setAttribute("aria-hidden", "true");
  Object.assign(shield.style, {
    position: "fixed", inset: "0", zIndex: "2147483647", cursor, touchAction: "none",
  });
  document.body.append(shield);
  let finished = false;
  const blockInput = (next: Event) => {
    next.preventDefault();
    next.stopImmediatePropagation();
  };
  const move = (next: PointerEvent) => {
    if (next.pointerId !== pointerId) return;
    blockInput(next);
    onDelta((axis === "x" ? next.clientX : next.clientY) - start);
  };
  const end = (next: PointerEvent) => {
    if (next.pointerId !== pointerId) return;
    blockInput(next);
    finish();
  };
  const lostCapture = (next: PointerEvent) => { if (next.pointerId === pointerId) finish(); };
  const finish = () => {
    if (finished) return;
    finished = true;
    window.removeEventListener("pointermove", move, true);
    window.removeEventListener("pointerup", end, true);
    window.removeEventListener("pointercancel", end, true);
    window.removeEventListener("mousemove", blockInput, true);
    window.removeEventListener("mouseup", blockInput, true);
    window.removeEventListener("blur", finish);
    handle.removeEventListener("lostpointercapture", lostCapture);
    if (handle.hasPointerCapture(pointerId)) handle.releasePointerCapture(pointerId);
    shield.remove();
    document.body.style.cursor = previousCursor;
    document.body.style.userSelect = previousUserSelect;
    onFinish();
  };
  document.body.style.cursor = cursor;
  document.body.style.userSelect = "none";
  window.addEventListener("pointermove", move, true);
  window.addEventListener("pointerup", end, true);
  window.addEventListener("pointercancel", end, true);
  window.addEventListener("mousemove", blockInput, true);
  window.addEventListener("mouseup", blockInput, true);
  window.addEventListener("blur", finish);
  handle.addEventListener("lostpointercapture", lostCapture);
  return finish;
}
