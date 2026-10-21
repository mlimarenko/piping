/** Route touch panning through xterm's wheel handler, which already owns
 * scrollback, alternate-screen cursor keys and application mouse reporting. */
export function attachSshTouchInput(element: HTMLElement, onTap: () => void): () => void {
  let start: { id: number; x: number; y: number; target: EventTarget | null } | null = null;
  let lastY = 0;
  let panning = false;
  const previousTouchAction = element.style.touchAction;
  element.style.touchAction = "pinch-zoom";

  const reset = () => { start = null; panning = false; };
  const onStart = (event: TouchEvent) => {
    reset();
    if (event.touches.length !== 1) return;
    const touch = event.touches[0];
    if (!touch) return;
    start = { id: touch.identifier, x: touch.clientX, y: touch.clientY, target: event.target };
    lastY = touch.clientY;
  };
  const onMove = (event: TouchEvent) => {
    if (!start) return;
    const touch = event.touches[0];
    if (event.touches.length !== 1 || touch?.identifier !== start.id) { reset(); return; }
    if (!panning) {
      const dx = Math.abs(touch.clientX - start.x);
      const dy = Math.abs(touch.clientY - start.y);
      if (Math.max(dx, dy) < 6) return;
      if (dx > dy) { reset(); return; }
      panning = true;
    }
    event.preventDefault();
    event.stopPropagation();
    const deltaY = lastY - touch.clientY;
    lastY = touch.clientY;
    if (!deltaY) return;
    const target = start.target instanceof Element && element.contains(start.target) ? start.target : element;
    const wheel = new WheelEvent("wheel", {
      bubbles: true, cancelable: true, deltaY, deltaMode: WheelEvent.DOM_DELTA_PIXEL,
      clientX: touch.clientX, clientY: touch.clientY,
    });
    // Chromium synthesizes wheelDeltaY with the same sign as deltaY. Xterm's
    // scrollbar prefers that deprecated field, reversing the swipe. Supply
    // only the standard delta fields so every browser takes the same path.
    Object.defineProperties(wheel, { wheelDeltaY: { value: undefined }, wheelDeltaX: { value: undefined } });
    target.dispatchEvent(wheel);
  };
  const onEnd = (event: TouchEvent) => {
    if (panning) { event.preventDefault(); event.stopPropagation(); }
    else if (start && event.touches.length === 0) onTap();
    reset();
  };
  element.addEventListener("touchstart", onStart, { passive: true });
  element.addEventListener("touchmove", onMove, { passive: false, capture: true });
  element.addEventListener("touchend", onEnd, { passive: false, capture: true });
  element.addEventListener("touchcancel", reset);
  return () => {
    reset();
    element.style.touchAction = previousTouchAction;
    element.removeEventListener("touchstart", onStart);
    element.removeEventListener("touchmove", onMove, true);
    element.removeEventListener("touchend", onEnd, true);
    element.removeEventListener("touchcancel", reset);
  };
}
