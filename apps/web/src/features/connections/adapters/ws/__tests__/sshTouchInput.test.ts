import { describe, expect, it, vi } from "vitest";
import { attachSshTouchInput } from "../sshTouchInput";

const point = (x: number, y: number, identifier = 1) => ({ clientX: x, clientY: y, identifier }) as Touch;
function touch(target: HTMLElement, type: string, touches: Touch[]) {
  const event = new TouchEvent(type, { touches, bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
}

describe("SSH touch input", () => {
  it("keeps taps distinct from pan gestures and forwards wheel events from the original child", () => {
    const element = document.createElement("div");
    const child = element.appendChild(document.createElement("div"));
    const tap = vi.fn(); const wheel = vi.fn();
    child.addEventListener("wheel", wheel);
    const detach = attachSshTouchInput(element, tap);
    touch(child, "touchstart", [point(100, 200)]);
    touch(child, "touchend", []);
    expect(tap).toHaveBeenCalledOnce();
    touch(child, "touchstart", [point(100, 200)]);
    touch(child, "touchmove", [point(101, 202)]);
    expect(wheel).not.toHaveBeenCalled();
    touch(child, "touchmove", [point(101, 260)]);
    expect(wheel.mock.calls[0]?.[0]).toMatchObject({ deltaY: -60, deltaMode: 0 });
    expect(wheel.mock.calls[0]?.[0].wheelDeltaY).toBeUndefined();
    expect(wheel.mock.calls[0]?.[0].wheelDeltaX).toBeUndefined();
    touch(child, "touchmove", [point(101, 275)]);
    expect(wheel.mock.calls[1]?.[0]).toMatchObject({ deltaY: -15 });
    expect(touch(child, "touchend", []).defaultPrevented).toBe(true);
    expect(tap).toHaveBeenCalledOnce();
    detach();
  });

  it("does not hijack pinch, horizontal gestures, cancelled gestures or a different touch identifier", () => {
    const element = document.createElement("div"); const wheel = vi.fn(); const tap = vi.fn();
    element.addEventListener("wheel", wheel);
    const detach = attachSshTouchInput(element, tap);
    for (const movement of [[point(100, 260), point(150, 300, 2)], [point(200, 201)], [point(100, 260, 2)]]) {
      touch(element, "touchstart", [point(100, 200)]);
      expect(touch(element, "touchmove", movement).defaultPrevented).toBe(false);
      touch(element, "touchend", []);
    }
    touch(element, "touchstart", [point(100, 200)]);
    touch(element, "touchcancel", []);
    touch(element, "touchmove", [point(100, 260)]);
    expect(wheel).not.toHaveBeenCalled(); expect(tap).not.toHaveBeenCalled();
    detach();
  });

  it("restores touch policy and removes every listener on disposal", () => {
    const element = document.createElement("div"); element.style.touchAction = "pan-y";
    const tap = vi.fn(); const wheel = vi.fn(); element.addEventListener("wheel", wheel);
    const detach = attachSshTouchInput(element, tap);
    expect(element.style.touchAction).toBe("pinch-zoom");
    detach(); expect(element.style.touchAction).toBe("pan-y");
    touch(element, "touchstart", [point(100, 200)]);
    touch(element, "touchmove", [point(100, 260)]);
    touch(element, "touchend", []);
    expect(wheel).not.toHaveBeenCalled(); expect(tap).not.toHaveBeenCalled();
  });
});
