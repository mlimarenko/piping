import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import LiveRDP from "./LiveRDP";

const mocks = vi.hoisted(() => ({ attach: vi.fn(), resize: vi.fn(), dispose: vi.fn() }));
vi.mock("@/features/connections/adapters/ws/rdpAdapter", () => ({ attachRdpViewer: mocks.attach }));

describe("RDP pane input and geometry", () => {
  let resizeObserved: () => void;
  const originalResizeObserver = globalThis.ResizeObserver;
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.attach.mockReturnValue({ resize: mocks.resize, dispose: mocks.dispose });
    globalThis.ResizeObserver = class {
      constructor(callback: ResizeObserverCallback) { resizeObserved = () => callback([], this); }
      observe() {} unobserve() {} disconnect() {}
    };
  });
  afterEach(() => { globalThis.ResizeObserver = originalResizeObserver; });

  it("resizes an unfocused visible client and relinquishes keyboard focus without disposing it", () => {
    const props = { websocketUrl: "/api/v1/ws/rdp/session-1", token: "fixture", isActive: true };
    const { rerender } = render(<LiveRDP {...props} />);
    const viewer = screen.getByTestId("live-rdp");
    viewer.focus();
    expect(document.activeElement).toBe(viewer);
    rerender(<LiveRDP {...props} isActive={false} />);
    expect(document.activeElement).not.toBe(viewer);
    vi.spyOn(viewer, "getBoundingClientRect").mockReturnValue({ x: 0, y: 0, top: 0, left: 0, right: 320, bottom: 240, width: 320, height: 240, toJSON: () => ({}) });
    act(() => resizeObserved());
    expect(mocks.resize).toHaveBeenLastCalledWith(320, 240);
    expect(mocks.attach).toHaveBeenCalledOnce();
    expect(mocks.dispose).not.toHaveBeenCalled();
    expect(screen.getByTestId("live-rdp")).toBe(viewer);
  });
});
