import { fireEvent, render, screen } from "@testing-library/react";
import "@/shared/i18n";
import { vi } from "vitest";
import SessionToolsMenu from "./SessionToolsMenu";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";

const baseSession: LiveSession = {
  id: "session-1",
  hostId: "host-1",
  hostName: "pxm-gitlab01",
  protocol: "ssh",
  status: "connected",
  websocketUrl: "",
  token: "",
};

function openMenu(session: LiveSession = baseSession, overrides = {}) {
  const props = {
    session,
    onDetachSession: vi.fn(),
    onReconnectSession: vi.fn(),
    onCloseSession: vi.fn(),
    onCloseAllSessions: vi.fn(),
    onToggleFileBrowser: vi.fn(),
    ...overrides,
  };

  render(<SessionToolsMenu {...props} />);
  fireEvent.pointerDown(screen.getByRole("button", { name: "Session tools" }), {
    button: 0,
    ctrlKey: false,
  });

  return props;
}

describe("SessionToolsMenu", () => {
  it("shows useful SSH file browser action and not ID copy actions", () => {
    openMenu();

    expect(screen.getByText("File browser")).toBeInTheDocument();
    expect(screen.getByText("Open in new tab")).toBeInTheDocument();
    expect(screen.getByText("Reconnect session")).toBeInTheDocument();
    expect(screen.getByText("Close session")).toBeInTheDocument();
    expect(screen.getByText("Close all sessions")).toBeInTheDocument();
    expect(screen.queryByText("Copy target name")).not.toBeInTheDocument();
    expect(screen.queryByText("Copy target ID")).not.toBeInTheDocument();
    expect(screen.queryByText("Copy session ID")).not.toBeInTheDocument();
  });

  it("uses contrast-safe styling for secondary action descriptions", () => {
    openMenu();

    expect(screen.getByText("Browse, upload, and download files")).toHaveClass("menu-item-muted");
  });

  it("routes enabled session actions to existing panel handlers", () => {
    const props = openMenu();

    fireEvent.click(screen.getByRole("menuitem", { name: "Open in new tab" }));
    expect(props.onDetachSession).toHaveBeenCalledWith("session-1");
  });

  it("routes the SSH file browser action to the embedded panel toggle", () => {
    const props = openMenu();

    const fileBrowserAction = screen.getByText("File browser").closest('[role="menuitem"]');
    expect(fileBrowserAction).not.toBeNull();
    fireEvent.click(fileBrowserAction!);

    expect(props.onToggleFileBrowser).toHaveBeenCalledOnce();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows the hide label when the embedded file browser is already open", () => {
    openMenu(baseSession, { isFileBrowserOpen: true });

    expect(screen.getByText("Hide file browser")).toBeInTheDocument();
  });

  it("disables reconnect while a session is still connecting", () => {
    openMenu({ ...baseSession, status: "connecting" });

    const reconnect = screen.getByText("Reconnect session").closest('[role="menuitem"]');
    expect(reconnect).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("Available after the session finishes connecting")).toBeInTheDocument();
  });

  it("offers the RDP file browser once the session drive is ready", () => {
    openMenu({ ...baseSession, protocol: "rdp" }, { isRdpDriveReady: true });

    const fileBrowserAction = screen.getByText("File browser").closest('[role="menuitem"]');
    expect(fileBrowserAction).not.toBeNull();
    expect(fileBrowserAction).not.toHaveAttribute("aria-disabled", "true");
  });

  it("disables the RDP file browser until guacd exposes the drive", () => {
    openMenu({ ...baseSession, protocol: "rdp" });

    const fileBrowserAction = screen.getByText("File browser").closest('[role="menuitem"]');
    expect(fileBrowserAction).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("Available once the session drive is ready")).toBeInTheDocument();
  });

  it("routes ready RDP clipboard action to the provided handler", () => {
    const onSendClipboard = vi.fn();
    openMenu(
      { ...baseSession, protocol: "rdp" },
      { isClipboardReady: true, onSendClipboard },
    );

    const clipboardAction = screen
      .getByText("Send browser clipboard")
      .closest('[role="menuitem"]');
    expect(clipboardAction).not.toHaveAttribute("aria-disabled", "true");
    fireEvent.click(clipboardAction!);

    expect(onSendClipboard).toHaveBeenCalledOnce();
  });

  it("disables RDP clipboard action until the remote channel is ready", () => {
    openMenu({ ...baseSession, protocol: "rdp" }, { onSendClipboard: vi.fn() });

    const clipboardAction = screen
      .getByText("Send browser clipboard")
      .closest('[role="menuitem"]');
    expect(clipboardAction).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("Available once the remote clipboard channel is ready")).toBeInTheDocument();
  });

  it("hides file tools for VNC sessions", () => {
    openMenu({ ...baseSession, protocol: "vnc" });

    expect(screen.queryByText("File browser")).not.toBeInTheDocument();
    expect(screen.getByText("Open in new tab")).toBeInTheDocument();
  });

  it("offers VNC file tools only through an existing SSH session", () => {
    const props = openMenu(
      { ...baseSession, protocol: "vnc" },
      { isVncSshFileBrowserReady: true },
    );

    const fileBrowserAction = screen.getByText("Files (SSH)").closest('[role="menuitem"]');
    expect(fileBrowserAction).not.toBeNull();
    expect(screen.getByText("Use the connected SSH session for this host")).toBeInTheDocument();
    fireEvent.click(fileBrowserAction!);

    expect(props.onToggleFileBrowser).toHaveBeenCalledOnce();
  });

  it("does not show clipboard tools for SSH sessions", () => {
    openMenu();

    expect(screen.queryByText("Send browser clipboard")).not.toBeInTheDocument();
  });
});
