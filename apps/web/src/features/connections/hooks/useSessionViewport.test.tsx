import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useSessionViewport } from "./useSessionViewport";

function Frame({ enabled = true }: { enabled?: boolean }) {
  return <div ref={useSessionViewport(enabled)} data-testid="frame" />;
}

describe("session visual viewport", () => {
  it("tracks keyboard, orientation and viewport panning without counteracting zoom", () => {
    const original = Object.getOwnPropertyDescriptor(window, "visualViewport");
    const viewport = Object.assign(new EventTarget(), { height: 720, offsetTop: 0, scale: 1 });
    Object.defineProperty(window, "visualViewport", { configurable: true, value: viewport });
    try {
      const { rerender, unmount } = render(<Frame />);
      const frame = screen.getByTestId("frame");
      expect(frame.style.getPropertyValue("--session-viewport-height")).toBe("720px");
      viewport.height = 340;
      viewport.offsetTop = 12;
      viewport.dispatchEvent(new Event("resize"));
      expect(frame.style.getPropertyValue("--session-viewport-height")).toBe("340px");
      expect(frame.style.getPropertyValue("--session-viewport-top")).toBe("12px");
      viewport.scale = 2;
      viewport.dispatchEvent(new Event("scroll"));
      expect(frame.style.getPropertyValue("--session-viewport-height")).toBe("100dvh");
      rerender(<Frame enabled={false} />);
      expect(frame.style.getPropertyValue("--session-viewport-height")).toBe("");
      viewport.height = 500;
      viewport.dispatchEvent(new Event("resize"));
      expect(frame.style.getPropertyValue("--session-viewport-height")).toBe("");
      unmount();
    } finally {
      if (original) Object.defineProperty(window, "visualViewport", original);
      else Reflect.deleteProperty(window, "visualViewport");
    }
  });
});
