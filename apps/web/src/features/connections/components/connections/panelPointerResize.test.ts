import type { PointerEvent as ReactPointerEvent } from "react";
import { describe, expect, it, vi } from "vitest";
import { beginPanelPointerResize } from "./panelPointerResize";

function pointer(type: string, pointerId: number, x: number): Event {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperties(event, { pointerId: { value: pointerId }, clientX: { value: x }, clientY: { value: x } });
  return event;
}

describe("panel pointer resize lifecycle", () => {
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
