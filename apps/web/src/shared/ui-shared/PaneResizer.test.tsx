import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import PaneResizer from "./PaneResizer";

function capture(handle: HTMLElement) {
  const captured = new Set<number>();
  handle.setPointerCapture = vi.fn(id => { captured.add(id); });
  handle.hasPointerCapture = id => captured.has(id);
  handle.releasePointerCapture = vi.fn(id => { captured.delete(id); });
}

function pointer(target: HTMLElement | Window, type: string, coordinate: number, pointerType = "mouse", button = 0) {
  return fireEvent(target, Object.assign(new MouseEvent(type, {
    bubbles: true, cancelable: true, clientX: coordinate, clientY: coordinate, button,
    buttons: type === "pointerup" ? 0 : 1,
  }), { pointerId: 7, pointerType, isPrimary: true }));
}

describe("panel resizing", () => {
  it("keeps dragging over a remote canvas that consumes mouse events", () => {
    function Fixture() {
      const [width, setWidth] = useState(420);
      return <><PaneResizer orientation="horizontal" reverse size={width} min={280} max={480} label="Session width" onResize={setWidth} /><canvas data-testid="remote" /></>;
    }
    render(<Fixture />);
    const handle = screen.getByRole("slider", { name: "Session width" });
    const canvas = screen.getByTestId("remote");
    capture(handle);
    const remoteInput = vi.fn((event: Event) => event.stopPropagation());
    canvas.addEventListener("mousemove", remoteInput);
    canvas.addEventListener("mouseup", remoteInput);
    try {
      pointer(handle, "pointerdown", 100);
      pointer(canvas, "pointermove", 140);
      fireEvent.mouseMove(canvas, { clientX: 140, buttons: 1 });
      expect(handle).toHaveAttribute("aria-valuenow", "380");
      expect(remoteInput).not.toHaveBeenCalled();
      pointer(canvas, "pointerup", 140);
      expect(handle.releasePointerCapture).toHaveBeenCalledWith(7);
      expect(document.body.style.cursor).toBe("");
      fireEvent.mouseMove(canvas, { clientX: 150 });
      expect(remoteInput).toHaveBeenCalledOnce();
      expect(handle).toHaveAttribute("aria-valuenow", "380");
    } finally {
      pointer(window, "pointercancel", 140);
      canvas.removeEventListener("mousemove", remoteInput);
      canvas.removeEventListener("mouseup", remoteInput);
    }
  });

  it("changes the trailing inspector size with keyboard and pointer, inside its bounds", () => {
    function Fixture() {
      const [width, setWidth] = useState(320);
      return <><PaneResizer orientation="horizontal" reverse size={width} min={280} max={480} label="Details width" onResize={setWidth} /><output aria-label="Width">{width}</output></>;
    }
    render(<Fixture />);
    const handle = screen.getByRole("slider", { name: "Details width" });
    fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(handle).toHaveAttribute("aria-valuenow", "340");
    capture(handle);
    pointer(handle, "pointerdown", 100);
    pointer(window, "pointermove", 60);
    pointer(window, "pointerup", 60);
    expect(handle).toHaveAttribute("aria-valuenow", "380");
    fireEvent.keyDown(handle, { key: "Home" });
    fireEvent.keyDown(handle, { key: "ArrowRight" });
    expect(handle).toHaveAttribute("aria-valuenow", "280");
    fireEvent.keyDown(handle, { key: "End" });
    expect(handle).toHaveAttribute("aria-valuenow", "480");
  });

  it("supports vertical touch resizing and releases input on unmount", () => {
    const resize = vi.fn();
    const { unmount } = render(<PaneResizer orientation="vertical" size={300} min={200} max={400} onResize={resize} label="Session height" />);
    const handle = screen.getByRole("slider", { name: "Session height" });
    capture(handle);
    pointer(handle, "pointerdown", 100, "touch");
    pointer(window, "pointermove", 150, "touch");
    expect(resize).toHaveBeenLastCalledWith(350);
    pointer(window, "pointermove", 500, "touch");
    expect(resize).toHaveBeenLastCalledWith(400);
    unmount();
    pointer(window, "pointermove", 120, "touch");
    expect(resize).toHaveBeenCalledTimes(2);
    expect(handle.releasePointerCapture).toHaveBeenCalledWith(7);
    expect(document.body.style.cursor).toBe("");
    const remoteInput = vi.fn();
    window.addEventListener("mousemove", remoteInput, { once: true });
    fireEvent.mouseMove(window);
    expect(remoteInput).toHaveBeenCalledOnce();
  });

  it("does not start resizing on a secondary mouse button", () => {
    const resize = vi.fn();
    render(<PaneResizer orientation="horizontal" size={300} onResize={resize} label="Session width" />);
    const handle = screen.getByRole("slider", { name: "Session width" });
    capture(handle);
    pointer(handle, "pointerdown", 100, "mouse", 2);
    pointer(window, "pointermove", 150);
    expect(handle.setPointerCapture).not.toHaveBeenCalled();
    expect(resize).not.toHaveBeenCalled();
  });
});
