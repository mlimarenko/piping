import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import ConnectionsPanel from "./ConnectionsPanel";

vi.mock("./LiveSessionViewer", () => ({
  default: () => <input aria-label="Preserved terminal" defaultValue="scrollback" />,
}));
vi.mock("./SessionHealthBar", () => ({ default: () => null }));

const props = () => ({
  mobile: true,
  sessions: [{ id: "ssh-1", hostId: "host-1", hostName: "mobile-host", protocol: "ssh" as const,
    status: "connected" as const, websocketUrl: "/api/v1/ws/ssh/ssh-1", token: "fixture" }],
  activeSessionId: "ssh-1", onSelectSession: vi.fn(), onCloseSession: vi.fn(),
  onCloseAllSessions: vi.fn(), onDetachSession: vi.fn(), onReconnectSession: vi.fn(),
  onSessionStatusChange: vi.fn(), expanded: true, onToggleExpand: vi.fn(),
  dockPosition: "bottom" as const, onChangeDock: vi.fn(), panelSize: 120, onPanelResize: vi.fn(),
  playbookTabs: [], activePlaybookTabId: null, onSelectPlaybookTab: vi.fn(), onClosePlaybookTab: vi.fn(),
});

describe("mobile session workspace", () => {
  beforeEach(() => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} };
  });

  it("uses the screen rather than the persisted desktop dock height", () => {
    const { container } = render(<ConnectionsPanel {...props()} />);
    expect(container.firstElementChild).toHaveClass("fixed");
    expect(container.firstElementChild).not.toHaveStyle({ height: "120px" });
  });

  it("ignores the persisted right dock on touch devices", () => {
    const { container } = render(<ConnectionsPanel {...props()} dockPosition="right" />);
    expect(container.firstElementChild).toHaveClass("flex-col");
    expect(container.firstElementChild).not.toHaveClass("flex-row");
  });

  it("keeps touch tab and close targets at 44px", () => {
    render(<ConnectionsPanel {...props()} />);
    expect(screen.getByRole("tab", { name: /mobile-host/i })).toHaveClass("min-h-11");
    expect(screen.getByRole("button", { name: "Close session" })).toHaveClass("size-11");
    expect(screen.getByRole("button", { name: "Back to connections" })).toHaveClass("size-11");
  });

  it("returns to targets without closing or remounting the terminal", () => {
    const callbacks = props();
    const { rerender } = render(<ConnectionsPanel {...callbacks} />);
    const terminal = screen.getByRole("textbox", { name: "Preserved terminal" });
    fireEvent.change(terminal, { target: { value: "kept buffer" } });
    fireEvent.click(screen.getByRole("button", { name: "Back to connections" }));
    expect(callbacks.onToggleExpand).toHaveBeenCalledOnce();
    expect(callbacks.onCloseSession).not.toHaveBeenCalled();
    rerender(<ConnectionsPanel {...callbacks} visible={false} />);
    rerender(<ConnectionsPanel {...callbacks} visible />);
    expect(screen.getByRole("textbox", { name: "Preserved terminal" })).toBe(terminal);
    expect(terminal).toHaveValue("kept buffer");
  });
});
