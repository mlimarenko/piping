import type { PointerEvent as ReactPointerEvent } from "react";
import { describe, expect, it, vi } from "vitest";
import { beginPanelPointerResize } from "./panelPointerResize";

function pointer(type: string, pointerId: number, x: number): Event {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperties(event, { pointerId: { value: pointerId }, clientX: { value: x }, clientY: { value: x } });
  return event;
}

describe("panel pointer resize lifecycle", () => {
  it.each(["pointerup", "pointercancel", "lostpointercapture", "blur", "cleanup"])("restores input and prior body styles after %s", (ending) => {
    const handle = document.createElement("div");
    document.body.append(handle);
    let captured = false;
    handle.setPointerCapture = vi.fn(() => { captured = true; });
    handle.hasPointerCapture = () => captured;
    handle.releasePointerCapture = vi.fn(() => { captured = false; });
    document.body.style.cursor = "crosshair";
    document.body.style.userSelect = "text";
    const delta = vi.fn(), finish = vi.fn(), remoteInput = vi.fn();
    handle.addEventListener("mousemove", remoteInput);
    const childCount = document.body.childElementCount;
    const cleanup = beginPanelPointerResize({ currentTarget: handle, pointerId: 7, clientX: 100, clientY: 100,
      preventDefault() {}, stopPropagation() {} } as unknown as ReactPointerEvent<HTMLElement>, "y", delta, finish);
    try {
      expect(document.body.childElementCount).toBe(childCount + 1);
      handle.dispatchEvent(new MouseEvent("mousemove", { bubbles: true }));
      expect(remoteInput).not.toHaveBeenCalled();
      window.dispatchEvent(pointer("pointerup", 8, 130));
      handle.dispatchEvent(pointer("lostpointercapture", 8, 130));
      expect(finish).not.toHaveBeenCalled();
      window.dispatchEvent(pointer("pointermove", 7, 130));
      expect(delta).toHaveBeenCalledExactlyOnceWith(30);
      if (ending === "cleanup") cleanup();
      else if (ending === "blur") window.dispatchEvent(new Event("blur"));
      else (ending === "lostpointercapture" ? handle : window).dispatchEvent(pointer(ending, 7, 130));
      cleanup();
      expect(finish).toHaveBeenCalledOnce();
      expect(captured).toBe(false);
      expect(document.body.childElementCount).toBe(childCount);
      expect(document.body.style.cursor).toBe("crosshair");
      expect(document.body.style.userSelect).toBe("text");
      handle.dispatchEvent(new MouseEvent("mousemove", { bubbles: true }));
      expect(remoteInput).toHaveBeenCalledOnce();
      window.dispatchEvent(pointer("pointermove", 7, 180));
      expect(delta).toHaveBeenCalledTimes(1);
    } finally {
      cleanup();
      handle.remove();
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    }
  });

  it("captures one touch pointer, isolates moves from remote input and cleans up once on cancel", () => {
    const handle = document.createElement("div");
    const release = vi.fn();
    handle.setPointerCapture = vi.fn(); handle.hasPointerCapture = () => true; handle.releasePointerCapture = release;
    const delta = vi.fn(), finish = vi.fn(), remoteInput = vi.fn();
    window.addEventListener("pointermove", remoteInput);
    const preventDefault = vi.fn(), stopPropagation = vi.fn();
    const cleanup = beginPanelPointerResize({ currentTarget: handle, pointerId: 7, clientX: 100, clientY: 100, preventDefault, stopPropagation } as unknown as ReactPointerEvent<HTMLDivElement>, "x", delta, finish);
    window.dispatchEvent(pointer("pointermove", 8, 140));
    expect(delta).not.toHaveBeenCalled();
    remoteInput.mockClear();
    window.dispatchEvent(pointer("pointermove", 7, 130));
    expect(delta).toHaveBeenCalledExactlyOnceWith(30);
    expect(remoteInput).not.toHaveBeenCalled();
    window.dispatchEvent(pointer("pointercancel", 7, 130));
    cleanup();
    expect(finish).toHaveBeenCalledOnce();
    expect(release).toHaveBeenCalledWith(7);
    window.dispatchEvent(pointer("pointermove", 7, 180));
    expect(delta).toHaveBeenCalledTimes(1);
    expect(document.body.style.userSelect).toBe("");
    window.removeEventListener("pointermove", remoteInput);
  });
});
