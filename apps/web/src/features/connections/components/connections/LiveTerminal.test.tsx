import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type * as ReactI18next from "react-i18next";
import type * as SshAdapterModule from "@/features/connections/adapters/ws/sshAdapter";
import type { SshAdapterOptions } from "@/features/connections/adapters/ws/sshAdapter";

const mocks = vi.hoisted(() => ({
  focus: vi.fn(), blur: vi.fn(), sendKey: vi.fn(), sendText: vi.fn(), fit: vi.fn(),
  dispose: vi.fn(), attach: vi.fn(), setFontSize: vi.fn(), getFontSize: vi.fn(() => 13),
  setSoftKeyboardEnabled: vi.fn(), isLive: vi.fn(() => true),
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
  mocks.isLive.mockReturnValue(true);
  mocks.attach.mockReturnValue({
    focus: mocks.focus, blur: mocks.blur, sendKey: mocks.sendKey, sendText: mocks.sendText,
    fit: mocks.fit, dispose: mocks.dispose, isLive: mocks.isLive,
    setFontSize: mocks.setFontSize, getFontSize: mocks.getFontSize,
    setSoftKeyboardEnabled: mocks.setSoftKeyboardEnabled,
  });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

const props = { websocketUrl: "wss://host/api/v1/ws/ssh/session-1", token: "tok", isActive: true };

function commandInput(): HTMLInputElement {
  return screen.getByRole("textbox", { name: "connections.terminal.commandInput", hidden: true });
}
function keyboardButton(): HTMLElement {
  return screen.getByRole("button", { name: "connections.terminal.keyboardShow" });
}

describe("SSH mobile terminal input", () => {
  it("read-only observation exposes no input controller or touch controls", () => {
    const controller = vi.fn();
    const { unmount } = render(<LiveTerminal {...props} token="" readOnly onControllerChange={controller} />);
    expect(mocks.attach).toHaveBeenCalledWith(expect.objectContaining({readOnly: true}));
    expect(screen.queryByRole("toolbar")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(controller).not.toHaveBeenCalled();
    unmount();
    expect(controller).not.toHaveBeenCalled();
    expect(mocks.dispose).toHaveBeenCalledOnce();
  });

  it("scrolls an observed terminal without clipping the operator's rows or columns", () => {
    render(<LiveTerminal {...props} token="" readOnly />);
    const terminal = screen.getByTestId("live-terminal");
    // The backend pins this viewer to the operator's cols/rows and the adapter
    // never fits it, so the pane owns scrolling for the full grid.
    expect(terminal).toHaveClass("overflow-auto");
    expect(terminal).not.toHaveClass("overflow-hidden");
  });

  it("keeps an interactive terminal's fitted grid without an outer scrollbar", () => {
    render(<LiveTerminal {...props} />);
    const terminal = screen.getByTestId("live-terminal");
    expect(terminal).toHaveClass("overflow-hidden");
    expect(terminal).not.toHaveClass("overflow-auto");
  });

  it("keeps chord focus separate from explicit command keyboard entry", () => {
    render(<LiveTerminal {...props} />);
    const command = commandInput();
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    const chordInput = screen.getByRole("textbox", { name: "connections.terminal.chordCharacter" });
    act(() => chordInput.focus());
    expect(screen.queryByRole("textbox", { name: "connections.terminal.commandInput" })).not.toBeInTheDocument();
    const hide = screen.getByRole("button", { name: "connections.terminal.keyboardHide" });
    expect(hide).toHaveAttribute("aria-pressed", "true");
    fireEvent.change(chordInput, { target: { value: "c" } });
    fireEvent.click(screen.getByRole("button", { name: "Ctrl" }));
    fireEvent.submit(chordInput.closest("form")!);
    expect(mocks.sendKey).toHaveBeenCalledExactlyOnceWith({ character: "c" }, { ctrl: true });
    expect(mocks.sendText).not.toHaveBeenCalled();
    fireEvent.click(hide);
    expect(document.activeElement).not.toBe(chordInput);
    fireEvent.click(keyboardButton());
    expect(screen.getByRole("textbox", { name: "connections.terminal.commandInput" })).toBe(command);
    expect(document.activeElement).toBe(command);
    fireEvent.change(command, { target: { value: "retained command" } });
    act(() => chordInput.focus());
    expect(screen.queryByRole("textbox", { name: "connections.terminal.commandInput" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.enter" }));
    expect(mocks.sendKey).toHaveBeenLastCalledWith("enter", {});
    expect(mocks.sendText).not.toHaveBeenCalled();
    expect(command).toHaveValue("retained command");
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.keyboardHide" }));
    fireEvent.click(keyboardButton());
    fireEvent.submit(command.closest("form")!);
    expect(mocks.sendText).toHaveBeenCalledExactlyOnceWith("retained command\r");
    expect(mocks.attach).toHaveBeenCalledOnce();
    expect(mocks.dispose).not.toHaveBeenCalled();
  });

  it("keeps the focused chord inside its own scrollport when the keypad resizes", () => {
    const previousObserver = globalThis.ResizeObserver;
    let notifyResize = () => {};
    const observe = vi.fn();
    const disconnect = vi.fn();
    globalThis.ResizeObserver = class implements ResizeObserver {
      constructor(callback: ResizeObserverCallback) { notifyResize = () => callback([], this); }
      observe = observe;
      unobserve = vi.fn();
      disconnect = disconnect;
    };
    try {
      render(<LiveTerminal {...props} />);
      const terminal = screen.getByTestId("live-terminal");
      const toggle = screen.getByRole("button", { name: "connections.terminal.moreControls" });
      fireEvent.click(toggle);
      const keypad = screen.getByTestId("terminal-keypad");
      const input = screen.getByRole("textbox", { name: "connections.terminal.chordCharacter" });
      let height = 130;
      vi.spyOn(keypad, "getBoundingClientRect").mockImplementation(() => new DOMRect(0, 327, 390, height));
      Object.defineProperty(keypad, "clientHeight", { configurable: true, get: () => height });
      vi.spyOn(input, "getBoundingClientRect").mockImplementation(() => new DOMRect(4, 404 - keypad.scrollTop, 80, 44));
      act(() => input.focus());
      expect(observe).toHaveBeenCalledWith(keypad);
      expect(keypad.scrollTop).toBe(0);
      height = 86;
      act(() => notifyResize());
      expect(keypad.scrollTop).toBe(35);
      expect(input.getBoundingClientRect().bottom).toBeLessThanOrEqual(keypad.getBoundingClientRect().bottom);
      keypad.scrollTop = 100;
      act(() => notifyResize());
      expect(input.getBoundingClientRect().top).toBeGreaterThanOrEqual(keypad.getBoundingClientRect().top);
      fireEvent.click(toggle);
      expect(document.activeElement).not.toBe(input);
      expect(keyboardButton()).toHaveAttribute("aria-pressed", "false");
      expect(disconnect).toHaveBeenCalled();
      expect(screen.getByTestId("live-terminal")).toBe(terminal);
      expect(mocks.attach).toHaveBeenCalledOnce();
      expect(mocks.dispose).not.toHaveBeenCalled();
      expect(mocks.sendKey).not.toHaveBeenCalled();
      expect(mocks.sendText).not.toHaveBeenCalled();
    } finally {
      cleanup();
      globalThis.ResizeObserver = previousObserver;
    }
  });

  it("cannot focus an inactive or non-live retained terminal from its keyboard button", () => {
    const { rerender } = render(<LiveTerminal {...props} />);
    const button = keyboardButton();
    fireEvent.click(button);
    const input = commandInput();
    fireEvent.change(input, { target: { value: "retained" } });
    rerender(<LiveTerminal {...props} isActive={false} />);
    mocks.focus.mockClear();
    fireEvent.click(button);
    expect(button).toBeDisabled();
    expect(document.activeElement).not.toBe(input);
    expect(mocks.focus).not.toHaveBeenCalled();
    rerender(<LiveTerminal {...props} />);
    mocks.isLive.mockReturnValue(false);
    fireEvent.click(button);
    expect(document.activeElement).not.toBe(input);
    expect(mocks.focus).not.toHaveBeenCalled();
    mocks.isLive.mockReturnValue(true);
    fireEvent.click(button);
    expect(commandInput()).toHaveValue("retained");
    expect(mocks.attach).toHaveBeenCalledOnce();
    expect(mocks.dispose).not.toHaveBeenCalled();
  });

  it("prevents composing Enter in the chord input without sending", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    const input = screen.getByRole("textbox", { name: "connections.terminal.chordCharacter" });
    fireEvent.change(input, { target: { value: "a" } });
    fireEvent.compositionStart(input);
    const event = new KeyboardEvent("keydown", { key: "Enter", isComposing: true, bubbles: true, cancelable: true });
    expect(input.dispatchEvent(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    expect(mocks.sendKey).not.toHaveBeenCalled();
    expect(input).toHaveAttribute("autocapitalize", "none");
    const send = screen.getByRole("button", { name: "connections.terminal.sendChord" });
    expect(fireEvent.pointerDown(send)).toBe(false);
  });
  it("sends the fixed interrupt regardless of pending modifiers and consumes them", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    const shift = screen.getByRole("button", { name: "Shift" });
    fireEvent.click(shift);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.interrupt" }));
    expect(mocks.sendKey).toHaveBeenCalledExactlyOnceWith("interrupt", {});
    expect(shift).toHaveAttribute("aria-pressed", "false");
  });

  it("reports rejected keys and retains a rejected character", () => {
    mocks.sendKey.mockReturnValue(false);
    render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    fireEvent.click(screen.getByRole("button", { name: "Ctrl" }));
    fireEvent.click(screen.getByRole("button", { name: "Shift" }));
    const input = screen.getByRole("textbox", { name: "connections.terminal.chordCharacter" });
    fireEvent.change(input, { target: { value: "c" } });
    fireEvent.submit(input.closest("form")!);
    expect(mocks.sendKey).toHaveBeenCalledExactlyOnceWith({ character: "c" }, { ctrl: true, shift: true });
    expect(input).toHaveValue("c");
    expect(screen.getByRole("alert")).toHaveTextContent("connections.terminal.keyNotSent");
    fireEvent.click(screen.getByRole("button", { name: "Shift" }));
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.key.pageUp" }));
    expect(mocks.sendKey).toHaveBeenLastCalledWith("pageUp", { shift: true });
    expect(screen.getByRole("alert")).toBeInTheDocument();
    mocks.sendKey.mockReturnValue(true);
    fireEvent.submit(input.closest("form")!);
    expect(input).toHaveValue("");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("clears hidden modifiers on collapse", () => {
    render(<LiveTerminal {...props} />);
    const toggle = screen.getByRole("button", { name: "connections.terminal.moreControls" });
    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole("button", { name: "Alt" }));
    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.tab" }));
    expect(mocks.sendKey).toHaveBeenCalledExactlyOnceWith("tab", {});
  });

  it("blurs a focused composing chord on deactivation without losing the command draft", () => {
    const { rerender } = render(<LiveTerminal {...props} />);
    fireEvent.click(keyboardButton());
    const command = commandInput();
    fireEvent.change(command, { target: { value: "saved" } });
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    const chordInput = screen.getByRole("textbox", { name: "connections.terminal.chordCharacter" });
    act(() => chordInput.focus());
    fireEvent.compositionStart(chordInput);
    expect(document.activeElement).toBe(chordInput);
    rerender(<LiveTerminal {...props} isActive={false} />);
    expect(document.activeElement).not.toBe(chordInput);
    rerender(<LiveTerminal {...props} />);
    expect(document.activeElement).not.toBe(chordInput);
    fireEvent.click(keyboardButton());
    expect(commandInput()).toHaveValue("saved");
    fireEvent.submit(command.closest("form")!);
    expect(mocks.sendText).toHaveBeenCalledExactlyOnceWith("saved\r");
  });

  it("resets stale failure and input state when the adapter is replaced before first open", () => {
    const { rerender } = render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    const input = screen.getByRole("textbox", { name: "connections.terminal.chordCharacter" });
    fireEvent.change(input, { target: { value: "x" } });
    fireEvent.click(screen.getByRole("button", { name: "Ctrl" }));
    const old = mocks.attach.mock.calls[0]?.[0] as SshAdapterOptions;
    act(() => { old.onStatus?.("reconnecting-visible"); old.onClose?.(1006, "closed"); });
    rerender(<LiveTerminal {...props} websocketUrl="wss://host/api/v1/ws/ssh/session-2" token="other" />);
    expect(mocks.attach).toHaveBeenCalledTimes(2);
    expect(mocks.dispose).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "connections.terminal.moreControls" })).toBeEnabled();
    expect(screen.queryByTestId("live-terminal-reconnecting")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    expect(screen.getByRole("textbox", { name: "connections.terminal.chordCharacter" })).toHaveValue("");
    expect(screen.getByRole("button", { name: "Ctrl" })).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.tab" }));
    expect(mocks.sendKey).toHaveBeenCalledExactlyOnceWith("tab", {});
  });
  it("expands in the panel and sends one-shot modifiers without replacing the terminal", () => {
    render(<LiveTerminal {...props} />);
    const terminal = screen.getByTestId("live-terminal");
    const toggle = screen.getByRole("button", { name: "connections.terminal.moreControls" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByTestId("terminal-keypad")).not.toBeInTheDocument();
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(screen.getByRole("button", { name: "Shift" }));
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.tab" }));
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.key.f12" }));
    expect(mocks.sendKey.mock.calls).toEqual([["tab", { shift: true }], ["f12", {}]]);
    fireEvent.click(toggle);
    expect(screen.getByTestId("live-terminal")).toBe(terminal);
    expect(mocks.attach).toHaveBeenCalledOnce();
    expect(mocks.dispose).not.toHaveBeenCalled();
  });

  it("rejects non-live and inactive actions and resets modifiers after reconnect or switching", () => {
    const { rerender } = render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    const ctrl = screen.getByRole("button", { name: "Ctrl" });
    fireEvent.click(ctrl);
    mocks.isLive.mockReturnValue(false);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.tab" }));
    expect(mocks.sendKey).not.toHaveBeenCalled();
    const options = mocks.attach.mock.calls[0]?.[0] as SshAdapterOptions;
    act(() => options.onStatus?.("reconnecting-silent"));
    act(() => options.onStatus?.("live"));
    mocks.isLive.mockReturnValue(true);
    expect(ctrl).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(ctrl);
    rerender(<LiveTerminal {...props} isActive={false} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.tab", hidden: true }));
    expect(mocks.sendKey).not.toHaveBeenCalled();
    rerender(<LiveTerminal {...props} />);
    expect(ctrl).toHaveAttribute("aria-pressed", "false");
  });

  it("only sends an explicitly submitted single printable chord, not composition or text", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    fireEvent.click(screen.getByRole("button", { name: "Ctrl" }));
    const input = screen.getByRole("textbox", { name: "connections.terminal.chordCharacter" });
    fireEvent.change(input, { target: { value: "ab" } });
    fireEvent.submit(input.closest("form")!);
    fireEvent.change(input, { target: { value: "a" } });
    fireEvent.compositionStart(input);
    fireEvent.submit(input.closest("form")!);
    expect(mocks.sendKey).not.toHaveBeenCalled();
    fireEvent.compositionEnd(input);
    fireEvent.submit(input.closest("form")!);
    expect(mocks.sendKey).toHaveBeenCalledExactlyOnceWith({ character: "a" }, { ctrl: true });
  });

  it("does not submit an IME composition and keeps the command draft", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(keyboardButton());
    const input = commandInput();
    fireEvent.change(input, { target: { value: "日本語" } });
    fireEvent.compositionStart(input);
    fireEvent.submit(input.closest("form")!);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.enter" }));
    expect(mocks.sendText).not.toHaveBeenCalled();
    expect(mocks.sendKey).not.toHaveBeenCalled();
    expect(input).toHaveValue("日本語");
    fireEvent.compositionEnd(input);
    fireEvent.submit(input.closest("form")!);
    expect(mocks.sendText).toHaveBeenCalledExactlyOnceWith("日本語\r");
  });

  it("clears modifiers explicitly and on input mode change", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    const alt = screen.getByRole("button", { name: "Alt" });
    fireEvent.click(alt);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.clearModifiers" }));
    expect(alt).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(alt);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.directInput" }));
    expect(alt).toHaveAttribute("aria-pressed", "false");
  });

  it("does not intercept native terminal typing or paste in direct mode", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.directInput" }));
    const terminal = screen.getByTestId("live-terminal");
    const key = new KeyboardEvent("keydown", { key: "a", bubbles: true, cancelable: true });
    expect(terminal.dispatchEvent(key)).toBe(true);
    const paste = new Event("paste", { bubbles: true, cancelable: true });
    expect(terminal.dispatchEvent(paste)).toBe(true);
    expect(mocks.sendKey).not.toHaveBeenCalled();
    expect(mocks.sendText).not.toHaveBeenCalled();
  });
  it("refits and focuses on a desktop focus request without reconnecting", () => {
    stubMobile(false);
    vi.spyOn(window, "requestAnimationFrame").mockImplementation(callback => { callback(0); return 1; });
    const { rerender } = render(<LiveTerminal {...props} />);
    mocks.focus.mockClear();
    mocks.fit.mockClear();
    rerender(<LiveTerminal {...props} focusRequest={1} />);
    expect(mocks.focus).toHaveBeenCalled();
    expect(mocks.fit).toHaveBeenCalled();
    expect(mocks.attach).toHaveBeenCalledOnce();
    expect(mocks.dispose).not.toHaveBeenCalled();
    vi.restoreAllMocks();
  });
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

  it("does not open the mobile keyboard on a focus request", () => {
    vi.spyOn(window, "requestAnimationFrame").mockImplementation(callback => { callback(0); return 1; });
    const { rerender } = render(<LiveTerminal {...props} />);
    rerender(<LiveTerminal {...props} focusRequest={1} />);
    expect(mocks.focus).not.toHaveBeenCalled();
    expect(mocks.attach).toHaveBeenCalledOnce();
    expect(mocks.dispose).not.toHaveBeenCalled();
    vi.restoreAllMocks();
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
    expect(mocks.sendKey.mock.calls).toEqual([["interrupt", {}], ["enter", {}]]);
    expect(mocks.sendText).not.toHaveBeenCalled();
  });

  it("enables direct terminal input through the same adapter and never re-attaches", () => {
    render(<LiveTerminal {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.directInput" }));
    expect(mocks.setSoftKeyboardEnabled).toHaveBeenLastCalledWith(true);
    expect(screen.getByRole("status")).toHaveTextContent("connections.terminal.modeDirect");
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
    fireEvent.click(screen.getByRole("button", { name: "connections.terminal.moreControls" }));
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
