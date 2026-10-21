import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type * as ReactI18next from "react-i18next";
import type * as SshAdapterModule from "@/features/connections/adapters/ws/sshAdapter";
import type { SshAdapterOptions } from "@/features/connections/adapters/ws/sshAdapter";

const mocks = vi.hoisted(() => ({
  focus: vi.fn(), blur: vi.fn(), sendKey: vi.fn(), sendText: vi.fn(), fit: vi.fn(),
  dispose: vi.fn(), attach: vi.fn(), setFontSize: vi.fn(), getFontSize: vi.fn(() => 13),
  setSoftKeyboardEnabled: vi.fn(),
}));

vi.mock("@/features/connections/adapters/ws/sshAdapter", async (importOriginal) => {
  // Keep the real bounded-range constants so the component's clamp and the
  // adapter's own bound cannot drift apart in the test.
  const actual = await importOriginal<typeof SshAdapterModule>();
  return { ...actual, attachSshTerminal: mocks.attach };
});
vi.mock("react-i18next", async (importOriginal) => ({
  ...await importOriginal<typeof ReactI18next>(),
  useTranslation: () => ({ t: (key: string) => key }),
}));
import LiveTerminal from "./LiveTerminal";

/** Stub `matchMedia` so the soft-keyboard device decision is deterministic. */
function stubMobile(mobile: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: mobile && (query.includes("max-width") || query.includes("pointer: coarse")),
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  }));
}

beforeEach(() => {
  vi.clearAllMocks();
  stubMobile(true);
  mocks.sendText.mockReturnValue(true);
  mocks.sendKey.mockReturnValue(true);
  mocks.attach.mockReturnValue({
    focus: mocks.focus, blur: mocks.blur, sendKey: mocks.sendKey, sendText: mocks.sendText,
    fit: mocks.fit, dispose: mocks.dispose, isLive: () => true,
    setFontSize: mocks.setFontSize, getFontSize: mocks.getFontSize,
    setSoftKeyboardEnabled: mocks.setSoftKeyboardEnabled,
  });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

const props = { websocketUrl: "wss://host/api/v1/ws/ssh/session-1", token: "tok", isActive: true };

function commandInput(): HTMLInputElement {
  return screen.getByRole("textbox", { name: "connections.terminal.commandInput" });
}
function keyboardButton(): HTMLElement {
  return screen.getByRole("button", { name: "connections.terminal.keyboardShow" });
}

describe("SSH mobile terminal input", () => {
  it("keeps an unfocused visible pane's draft and output but hides its composer and touch toolbar", () => {
    const { rerender } = render(<LiveTerminal {...props} isVisible />);
    fireEvent.click(keyboardButton());
    const input = commandInput();
    fireEvent.change(input, { target: { value: "saved draft" } });
    rerender(<LiveTerminal {...props} isActive={false} isVisible />);
    expect(screen.getByTestId("live-terminal")).toBeInTheDocument();
    expect(input.closest("form")).toHaveClass("hidden");
    expect(screen.getByRole("toolbar", { hidden: true })).toHaveClass("hidden");
    fireEvent.submit(input.closest("form")!);
    expect(mocks.sendText).not.toHaveBeenCalled();
    rerender(<LiveTerminal {...props} isVisible />);
    fireEvent.click(keyboardButton());
    expect(commandInput()).toBe(input);
    expect(input).toHaveValue("saved draft");
    expect(mocks.attach).toHaveBeenCalledOnce();
    expect(mocks.dispose).not.toHaveBeenCalled();
  });
  it("does NOT focus the terminal (or open the OS keyboard) on session activation", async () => {
    render(<LiveTerminal {...props} />);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 150)); });
    expect(mocks.focus).not.toHaveBeenCalled();
    expect(mocks.attach).toHaveBeenCalledTimes(1);
  });

  it("still focuses the terminal on activation for a fine-pointer desktop", async () => {
    stubMobile(false);
    render(<LiveTerminal {...props} />);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 150)); });
    expect(mocks.focus).toHaveBeenCalled();
  });

  it("keeps the terminal from hijacking the keyboard: soft input disabled in command mode", () => {
    render(<LiveTerminal {...props} />);
    expect(mocks.setSoftKeyboardEnabled).toHaveBeenCalledWith(false);
  });

  it("shows the command field on an explicit keyboard request and blurs BOTH owners on hide, never reconnecting", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(keyboardButton());
    expect(document.activeElement).toBe(commandInput());
    expect(mocks.focus).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.keyboardHide" }));
    expect(mocks.blur).toHaveBeenCalledOnce();
    expect(document.activeElement).not.toBe(commandInput());
    // Controls must never tear down / reconnect the session.
    expect(mocks.attach).toHaveBeenCalledTimes(1);
    expect(mocks.dispose).not.toHaveBeenCalled();
  });

  it("sends one complete Unicode command plus CR on submit and does not fake echo", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(keyboardButton());
    const input = commandInput();
    fireEvent.change(input, { target: { value: "echo привет мир" } });
    // Composing locally must not push per-keystroke traffic.
    expect(mocks.sendText).not.toHaveBeenCalled();
    fireEvent.submit(input.closest("form")!);
    expect(mocks.sendText).toHaveBeenCalledTimes(1);
    expect(mocks.sendText).toHaveBeenCalledWith("echo привет мир\r");
    // Cleared locally; the remote's own output is the only echo.
    expect(input.value).toBe("");
  });

  it("submits composed text from the Enter touch key and keeps the local field focused", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(keyboardButton());
    const input = commandInput();
    fireEvent.change(input, { target: { value: "echo composed" } });
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.enter" }));
    expect(mocks.sendText).toHaveBeenCalledExactlyOnceWith("echo composed\r");
    expect(input).toHaveValue("");
    expect(document.activeElement).toBe(input);
  });

  it("sends Enter and Ctrl+C through the touch keys while live without taking focus", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.interrupt" }));
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.enter" }));
    expect(mocks.sendKey.mock.calls).toEqual([["interrupt"], ["enter"]]);
    expect(mocks.sendText).not.toHaveBeenCalled();
  });

  it("enables direct terminal input through the same adapter and never re-attaches", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.directInput" }));
    expect(mocks.setSoftKeyboardEnabled).toHaveBeenLastCalledWith(true);
    expect(screen.getByTestId("live-terminal-direct-input")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "connections.terminal.directInput" })).not.toBeInTheDocument();

    // In direct mode the keyboard toggle hands focus to the live PTY.
    mocks.focus.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.keyboardShow" }));
    expect(mocks.focus).toHaveBeenCalledOnce();
    expect(mocks.attach).toHaveBeenCalledTimes(1);
    expect(mocks.dispose).not.toHaveBeenCalled();
  });

  it("clamps font size to the bounded range via the adapter setter", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.fontSize" }));
    const decrease = screen.getByRole("button", { name: "connections.terminal.fontSizeDecrease" });
    const increase = screen.getByRole("button", { name: "connections.terminal.fontSizeIncrease" });
    mocks.setFontSize.mockImplementation((size: number) => size);

    for (let i = 0; i < 20; i += 1) fireEvent.click(increase);
    expect(screen.getByTestId("live-terminal-font-size")).toHaveTextContent("24px");
    expect(mocks.setFontSize).toHaveBeenLastCalledWith(24);
    expect(increase).toBeDisabled();

    for (let i = 0; i < 30; i += 1) fireEvent.click(decrease);
    expect(screen.getByTestId("live-terminal-font-size")).toHaveTextContent("10px");
    expect(mocks.setFontSize).toHaveBeenLastCalledWith(10);
    expect(decrease).toBeDisabled();
  });

  it("disables every mobile control and the command field on failure", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(keyboardButton());
    const options = mocks.attach.mock.calls[0]?.[0] as SshAdapterOptions;
    act(() => options.onStatus?.("reconnecting-visible"));

    for (const button of screen.getAllByRole("button")) expect(button).toBeDisabled();
    expect(commandInput()).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.interrupt" }));
    expect(mocks.sendKey).not.toHaveBeenCalled();
    fireEvent.submit(commandInput().closest("form")!);
    expect(mocks.sendText).not.toHaveBeenCalled();
  });

  it("blurs both input owners when returning to targets without reconnecting", () => {
    const { rerender } = render(<LiveTerminal {...props} />);
    fireEvent.click(keyboardButton());
    expect(document.activeElement).toBe(commandInput());
    rerender(<LiveTerminal {...props} isActive={false} />);
    expect(document.activeElement).not.toBe(commandInput());
    expect(mocks.blur).toHaveBeenCalledOnce();
    expect(mocks.attach).toHaveBeenCalledOnce();
    expect(mocks.dispose).not.toHaveBeenCalled();
  });
});
