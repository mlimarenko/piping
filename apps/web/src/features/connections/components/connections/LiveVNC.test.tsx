import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ComponentProps } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import { ApiError, apiErrorFromResponse } from "@/shared/api/client";
import type { SessionClientCredentials } from "@/features/connections/api";
import LiveVNC from "./LiveVNC";
import type { VncAdapter, VncAdapterOptions } from "@/features/connections/adapters/ws/vncAdapter";

const mocks = vi.hoisted(() => ({
  getSessionClientCredentials: vi.fn(),
  attachVncViewer: vi.fn(),
  waitForViewerPaint: vi.fn(),
}));

vi.mock("@/features/connections/api", () => ({
  getSessionClientCredentials: mocks.getSessionClientCredentials,
}));
vi.mock("@/features/connections/adapters/ws/vncAdapter", () => ({
  attachVncViewer: mocks.attachVncViewer,
}));
vi.mock("@/features/connections/components/connections/viewerPaint", () => ({
  waitForViewerPaint: mocks.waitForViewerPaint,
}));

const credentials = {
  session_id: "session-1",
  protocol: "vnc",
  credentials: { password: "resolved-secret" },
};

function adapter() {
  return {
    dispose: vi.fn(),
    focus: vi.fn(),
    resize: vi.fn(),
    setInputEnabled: vi.fn(),
  } satisfies VncAdapter;
}

function renderViewer(overrides: Partial<ComponentProps<typeof LiveVNC>> = {}) {
  const props: ComponentProps<typeof LiveVNC> = {
    sessionId: "session-1",
    websocketUrl: "wss://example.test/vnc/session-1",
    token: "session-token",
    isActive: true,
    ...overrides,
  };
  return render(<LiveVNC {...props} />);
}

describe("LiveVNC", () => {
  beforeEach(() => {
    mocks.getSessionClientCredentials.mockReset();
    mocks.attachVncViewer.mockReset();
    mocks.waitForViewerPaint.mockReset();
    mocks.getSessionClientCredentials.mockResolvedValue(credentials);
    mocks.waitForViewerPaint.mockResolvedValue(undefined);
    vi.spyOn(window, "requestAnimationFrame").mockReturnValue(1);
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => undefined);
  });

  it("hands each concurrent session's endpoint password only to its own adapter", async () => {
    const passwords = [crypto.randomUUID().slice(0, 8), crypto.randomUUID().slice(0, 8)] as const;
    const pending = new Map<string, (value: SessionClientCredentials) => void>();
    mocks.getSessionClientCredentials.mockImplementation((id: string) => new Promise(resolve => {
      pending.set(id, resolve);
    }));
    const attached: VncAdapterOptions[] = [];
    mocks.attachVncViewer.mockImplementation((options: VncAdapterOptions) => {
      attached.push(options);
      return Promise.resolve(adapter());
    });
    renderViewer();
    renderViewer({ sessionId: "session-2", websocketUrl: "wss://example.test/vnc/session-2" });

    await act(async () => {
      pending.get("session-2")!({ session_id: "session-2", protocol: "vnc", credentials: { password: passwords[1] } });
      await Promise.resolve();
    });
    await waitFor(() => expect(attached).toHaveLength(1));
    await act(async () => {
      pending.get("session-1")!({ session_id: "session-1", protocol: "vnc", credentials: { password: passwords[0] } });
      await Promise.resolve();
    });
    await waitFor(() => expect(attached).toHaveLength(2));
    for (const [index, id] of ["session-1", "session-2"].entries()) {
      const options = attached.find(item => item.url.endsWith(id));
      expect(options !== undefined).toBe(true);
      expect(options?.password === passwords[index]).toBe(true);
      expect(options?.password === passwords[1 - index]).toBe(false);
      expect(mocks.getSessionClientCredentials).toHaveBeenCalledWith(id);
    }
  });

  it("accepts a passwordless endpoint response without reusing the previous mounted session's password", async () => {
    const attached: VncAdapterOptions[] = [];
    const client = adapter();
    mocks.attachVncViewer.mockImplementation((options: VncAdapterOptions) => {
      attached.push(options);
      return Promise.resolve(client);
    });
    const view = renderViewer();
    await waitFor(() => expect(attached).toHaveLength(1));
    view.unmount();
    expect(client.dispose).toHaveBeenCalledOnce();
    mocks.getSessionClientCredentials.mockResolvedValue({ session_id: "session-2", protocol: "vnc", credentials: {} });
    renderViewer({ sessionId: "session-2", websocketUrl: "wss://example.test/vnc/session-2" });
    await waitFor(() => expect(attached).toHaveLength(2));
    expect(attached[1]?.password === undefined).toBe(true);
  });

  it.each([403, 410])("fails closed on HTTP %i after a successful session without reusing its password", async status => {
    mocks.attachVncViewer.mockResolvedValue(adapter());
    const view = renderViewer();
    await waitFor(() => expect(mocks.attachVncViewer).toHaveBeenCalledOnce());
    mocks.getSessionClientCredentials.mockImplementation(async () => {
      throw await apiErrorFromResponse(Response.json({ message: "PRIVATE BACKEND DETAIL" }, { status }));
    });
    view.rerender(<LiveVNC sessionId="session-2" websocketUrl="wss://example.test/vnc/session-2" token="fixture" isActive />);
    const alert = await screen.findByRole("alert");
    expect(alert).not.toHaveTextContent("PRIVATE BACKEND DETAIL");
    expect(mocks.getSessionClientCredentials).toHaveBeenLastCalledWith("session-2");
    expect(mocks.attachVncViewer).toHaveBeenCalledOnce();
  });

  it("ignores a late credential response after the viewer changes session", async () => {
    let resolveFirst!: (value: SessionClientCredentials) => void;
    mocks.getSessionClientCredentials.mockReturnValueOnce(new Promise(resolve => { resolveFirst = resolve; }));
    mocks.getSessionClientCredentials.mockResolvedValue({ session_id: "session-2", protocol: "vnc", credentials: {} });
    const attached: VncAdapterOptions[] = [];
    mocks.attachVncViewer.mockImplementation((options: VncAdapterOptions) => {
      attached.push(options);
      return Promise.resolve(adapter());
    });
    const view = renderViewer();
    view.rerender(<LiveVNC sessionId="session-2" websocketUrl="wss://example.test/vnc/session-2" token="fixture" isActive />);
    await waitFor(() => expect(attached).toHaveLength(1));
    await act(async () => { resolveFirst(credentials); await Promise.resolve(); });
    expect(attached).toHaveLength(1);
    expect(attached[0]?.url).toBe("wss://example.test/vnc/session-2");
    expect(attached[0]?.password === undefined).toBe(true);
  });

  it("disables input on a live pane when it loses activity without reconnecting", async () => {
    const client = adapter();
    mocks.attachVncViewer.mockResolvedValue(client);
    const view = renderViewer({ token: "fixture" });
    await waitFor(() => expect(mocks.attachVncViewer).toHaveBeenCalledOnce());
    view.rerender(<LiveVNC sessionId="session-1" websocketUrl="wss://example.test/vnc/session-1" token="fixture" isActive={false} />);
    expect(client.setInputEnabled).toHaveBeenLastCalledWith(false);
    expect(client.dispose).not.toHaveBeenCalled();
  });

  it("applies the latest inactive state after asynchronous attachment", async () => {
    const client = adapter();
    let resolveAttach!: (value: VncAdapter) => void;
    mocks.attachVncViewer.mockReturnValue(new Promise(resolve => { resolveAttach = resolve; }));
    const view = renderViewer({ token: "fixture" });
    await waitFor(() => expect(mocks.attachVncViewer).toHaveBeenCalledOnce());
    view.rerender(<LiveVNC sessionId="session-1" websocketUrl="wss://example.test/vnc/session-1" token="fixture" isActive={false} />);
    await act(async () => { resolveAttach(client); await Promise.resolve(); });
    expect(client.setInputEnabled).toHaveBeenLastCalledWith(false);
    expect(mocks.attachVncViewer).toHaveBeenCalledOnce();
  });
  it("resizes an unfocused pane through its observer without recreating the RFB client", async () => {
    let resizeObserved!: () => void;
    const client = adapter();
    const originalResizeObserver = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class {
      constructor(callback: ResizeObserverCallback) { resizeObserved = () => callback([], this); }
      observe() {} unobserve() {} disconnect() {}
    };
    vi.spyOn(window, "requestAnimationFrame").mockImplementation(callback => { callback(0); return 1; });
    mocks.attachVncViewer.mockResolvedValue(client);
    try {
      const view = renderViewer({ isActive: false });
      await waitFor(() => expect(mocks.attachVncViewer).toHaveBeenCalledOnce());
      const viewer = screen.getByTestId("live-vnc");
      client.resize.mockClear();
      act(() => resizeObserved());
      expect(client.resize).toHaveBeenCalledOnce();
      expect(client.focus).not.toHaveBeenCalled();
      expect(client.dispose).not.toHaveBeenCalled();
      expect(screen.getByTestId("live-vnc")).toBe(viewer);
      view.unmount();
    } finally {
      globalThis.ResizeObserver = originalResizeObserver;
    }
  });
  it("maps a credential rejection to safe copy and offers retry without attaching", async () => {
    mocks.getSessionClientCredentials.mockRejectedValue(
      new ApiError({
        status: 401,
        reasonCode: "connection_authentication_failed",
        message: "PRIVATE BACKEND DETAIL",
      }),
    );
    const onRetry = vi.fn();
    renderViewer({ onRetry });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Authentication failed");
    expect(alert).not.toHaveTextContent("PRIVATE BACKEND DETAIL");
    expect(mocks.attachVncViewer).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("turns an unclean socket close into a network failure and forwards the close", async () => {
    let options!: VncAdapterOptions;
    mocks.attachVncViewer.mockImplementation((next: VncAdapterOptions) => {
      options = next;
      return Promise.resolve(adapter());
    });
    const onClose = vi.fn();
    renderViewer({ onClose });
    await waitFor(() => expect(mocks.attachVncViewer).toHaveBeenCalledTimes(1));

    act(() => options.onClose?.(false, "PRIVATE BACKEND DETAIL"));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Network error");
    expect(alert).not.toHaveTextContent("PRIVATE BACKEND DETAIL");
    expect(onClose).toHaveBeenCalledWith(false, "PRIVATE BACKEND DETAIL");
  });

  it("shows a retryable generic failure when viewer attachment rejects", async () => {
    mocks.attachVncViewer.mockRejectedValue(new Error("noVNC internals leaked"));
    const onRetry = vi.fn();
    renderViewer({ onRetry });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Network error");
    expect(alert).not.toHaveTextContent("noVNC internals leaked");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("disposes a viewer that finishes attaching after unmount", async () => {
    let resolveAttach!: (value: VncAdapter) => void;
    const lateAdapter = adapter();
    mocks.attachVncViewer.mockReturnValue(
      new Promise((resolve) => {
        resolveAttach = resolve;
      }),
    );
    const onClipboardControllerChange = vi.fn();
    const view = renderViewer({ onClipboardControllerChange });
    await waitFor(() => expect(mocks.attachVncViewer).toHaveBeenCalledTimes(1));

    view.unmount();
    await act(async () => {
      resolveAttach(lateAdapter);
      await Promise.resolve();
    });

    expect(lateAdapter.dispose).toHaveBeenCalledTimes(1);
    expect(onClipboardControllerChange).toHaveBeenCalledWith(null);
  });
});
