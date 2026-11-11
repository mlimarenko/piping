import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import LiveRDP from "./LiveRDP";

const mocks = vi.hoisted(() => ({ attach: vi.fn(), resize: vi.fn(), dispose: vi.fn(), scale: vi.fn() }));
vi.mock("@/features/connections/adapters/ws/rdpAdapter", () => ({ attachRdpViewer: mocks.attach }));

describe("RDP pane input and geometry", () => {
  let resizeObserved: () => void;
  const originalResizeObserver = globalThis.ResizeObserver;
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.attach.mockReturnValue({
      resize: mocks.resize,
      dispose: mocks.dispose,
      setDisplayScale: mocks.scale,
      getDisplaySize: () => ({ width: 0, height: 0 }),
    });
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

  it("focuses and resizes on request without reattaching", () => {
    const props = { websocketUrl: "/api/v1/ws/rdp/session-1", token: "fixture", isActive: true };
    const { rerender } = render(<LiveRDP {...props} />);
    const viewer = screen.getByTestId("live-rdp");
    vi.spyOn(viewer, "getBoundingClientRect").mockReturnValue({
      x: 0, y: 0, top: 0, left: 0, right: 320, bottom: 240, width: 320, height: 240, toJSON: () => ({}),
    });
    const frame = vi.spyOn(window, "requestAnimationFrame").mockImplementation(callback => { callback(0); return 1; });
    mocks.resize.mockClear();
    rerender(<LiveRDP {...props} focusRequest={1} />);
    expect(document.activeElement).toBe(viewer);
    expect(mocks.resize).toHaveBeenLastCalledWith(320, 240);
    expect(mocks.attach).toHaveBeenCalledOnce();
    expect(mocks.dispose).not.toHaveBeenCalled();
    frame.mockRestore();
  });

  it("scales a fit view and keeps actual size at 1:1", () => {
    mocks.attach.mockReturnValue({
      resize: mocks.resize,
      dispose: mocks.dispose,
      setDisplayScale: mocks.scale,
      getDisplaySize: () => ({ width: 1000, height: 500 }),
    });
    const props = { websocketUrl: "/api/v1/ws/vnc/session-1", token: "fixture", isActive: true };
    const { rerender } = render(<LiveRDP {...props} />);
    const viewer = screen.getByTestId("live-rdp");
    vi.spyOn(viewer, "getBoundingClientRect").mockReturnValue({
      x: 0, y: 0, top: 0, left: 0, right: 500, bottom: 500, width: 500, height: 500, toJSON: () => ({}),
    });

    act(() => resizeObserved());
    expect(mocks.scale).toHaveBeenLastCalledWith(0.5);

    rerender(<LiveRDP {...props} displayMode="actual" />);
    expect(mocks.scale).toHaveBeenLastCalledWith(1);
  });

  it("caps the requested remote resolution for the data-saver preset", () => {
    const props = { websocketUrl: "/api/v1/ws/vnc/session-1", token: "fixture", isActive: true };
    render(<LiveRDP {...props} maxResolution={{ width: 1280, height: 720 }} />);
    const viewer = screen.getByTestId("live-rdp");
    vi.spyOn(viewer, "getBoundingClientRect").mockReturnValue({
      x: 0, y: 0, top: 0, left: 0, right: 1600, bottom: 900, width: 1600, height: 900, toJSON: () => ({}),
    });

    act(() => resizeObserved());
    expect(mocks.resize).toHaveBeenLastCalledWith(1280, 720);
  });

  it("ignores malicious adapter callbacks in read-only mode", () => {
    const clipboard = vi.fn(); const filesystem = vi.fn(); const text = vi.fn();
    const { unmount } = render(<LiveRDP websocketUrl="/api/v1/ws/observe/session-1" token="" isActive readOnly onClipboardControllerChange={clipboard} onFilesystem={filesystem} onRemoteClipboardText={text} />);
    const options = mocks.attach.mock.calls[0]![0];
    options.onClipboardControllerChange({ sendText: vi.fn() });
    options.onFilesystem({});
    options.onRemoteClipboardText("secret");
    expect(clipboard).not.toHaveBeenCalled();
    expect(filesystem).not.toHaveBeenCalled();
    expect(text).not.toHaveBeenCalled();
    expect(mocks.resize).not.toHaveBeenCalled();
    unmount();
    expect(mocks.dispose).toHaveBeenCalledOnce();
  });
});
