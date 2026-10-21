import { describe, expect, it, vi } from "vitest";

import { attachRemoteClipboardPaste, writeBrowserClipboardText } from "../remoteClipboard";

function pasteEvent(text: string, target?: HTMLElement): ClipboardEvent {
  const event = new Event("paste", { bubbles: true, cancelable: true }) as ClipboardEvent;
  Object.defineProperty(event, "clipboardData", {
    value: { getData: vi.fn(() => text) },
  });
  target?.dispatchEvent(event);
  return event;
}

describe("attachRemoteClipboardPaste", () => {
  it("sends text pasted into the remote viewer element", () => {
    const element = document.createElement("div");
    const controller = { sendText: vi.fn() };

    const subscription = attachRemoteClipboardPaste(element, controller);
    const event = pasteEvent("hello\r\nremote", element);

    expect(controller.sendText).toHaveBeenCalledWith("hello\nremote");
    expect(event.defaultPrevented).toBe(true);

    subscription.dispose();
  });

  it("does not hijack paste into editable UI inside the viewer container", () => {
    const element = document.createElement("div");
    const input = document.createElement("input");
    element.append(input);
    const controller = { sendText: vi.fn() };

    const subscription = attachRemoteClipboardPaste(element, controller);
    const event = pasteEvent("local input", input);

    expect(controller.sendText).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);

    subscription.dispose();
  });

  it("reads browser clipboard on Ctrl+V and triggers the remote paste shortcut", async () => {
    const element = document.createElement("div");
    element.tabIndex = 0;
    document.body.append(element);
    element.focus();
    const controller = { sendText: vi.fn() };
    const readClipboardText = vi.fn().mockResolvedValue("local\r\nclipboard");
    const onPasteShortcut = vi.fn();

    const subscription = attachRemoteClipboardPaste(element, controller, {
      readClipboardText,
      onPasteShortcut,
    });
    const event = new KeyboardEvent("keydown", {
      key: "v",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    element.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(false);
    await vi.waitFor(() => {
      expect(controller.sendText).toHaveBeenCalledWith("local\nclipboard");
      expect(onPasteShortcut).toHaveBeenCalledOnce();
    });

    subscription.dispose();
    element.remove();
  });

  it("recognizes Ctrl+V by physical key code on non-Latin layouts", async () => {
    const element = document.createElement("div");
    element.tabIndex = 0;
    document.body.append(element);
    element.focus();
    const controller = { sendText: vi.fn() };
    const readClipboardText = vi.fn().mockResolvedValue("localized shortcut");
    const onPasteShortcut = vi.fn();

    const subscription = attachRemoteClipboardPaste(element, controller, {
      readClipboardText,
      onPasteShortcut,
    });
    const event = new KeyboardEvent("keydown", {
      key: "м",
      code: "KeyV",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    element.dispatchEvent(event);

    await vi.waitFor(() => {
      expect(controller.sendText).toHaveBeenCalledWith("localized shortcut");
      expect(onPasteShortcut).toHaveBeenCalledOnce();
    });

    subscription.dispose();
    element.remove();
  });

  it("captures native paste data for Ctrl+V when async clipboard read is unavailable", async () => {
    const element = document.createElement("div");
    element.tabIndex = 0;
    document.body.append(element);
    element.focus();
    const controller = { sendText: vi.fn() };
    const onPasteShortcut = vi.fn();

    const subscription = attachRemoteClipboardPaste(element, controller, {
      onPasteShortcut,
    });
    const keydown = new KeyboardEvent("keydown", {
      key: "v",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    element.dispatchEvent(keydown);

    const capture = document.querySelector("textarea[aria-hidden='true']");
    expect(capture).toBeInstanceOf(HTMLTextAreaElement);
    const paste = pasteEvent("native\r\nclipboard", capture as HTMLTextAreaElement);

    expect(keydown.defaultPrevented).toBe(false);
    expect(paste.defaultPrevented).toBe(true);
    await vi.waitFor(() => {
      expect(controller.sendText).toHaveBeenCalledWith("native\nclipboard");
      expect(onPasteShortcut).toHaveBeenCalledOnce();
    });

    subscription.dispose();
    element.remove();
  });

  it("suppresses paste shortcut keyup after focus moves to the native paste capture", () => {
    const element = document.createElement("div");
    element.tabIndex = 0;
    document.body.append(element);
    element.focus();
    const controller = { sendText: vi.fn() };
    const onPasteShortcut = vi.fn();

    const subscription = attachRemoteClipboardPaste(element, controller, {
      onPasteShortcut,
    });
    const keydown = new KeyboardEvent("keydown", {
      key: "v",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    element.dispatchEvent(keydown);

    const capture = document.querySelector("textarea[aria-hidden='true']");
    expect(capture).toBeInstanceOf(HTMLTextAreaElement);

    const keyup = new KeyboardEvent("keyup", {
      key: "v",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    capture?.dispatchEvent(keyup);

    expect(keyup.defaultPrevented).toBe(true);

    subscription.dispose();
    element.remove();
  });

  it("does not hijack Ctrl+V into editable UI inside the viewer container", () => {
    const element = document.createElement("div");
    const input = document.createElement("input");
    element.append(input);
    document.body.append(element);
    input.focus();
    const controller = { sendText: vi.fn() };
    const readClipboardText = vi.fn().mockResolvedValue("local input");
    const onPasteShortcut = vi.fn();

    const subscription = attachRemoteClipboardPaste(element, controller, {
      readClipboardText,
      onPasteShortcut,
    });
    const event = new KeyboardEvent("keydown", {
      key: "v",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    input.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(false);
    expect(readClipboardText).not.toHaveBeenCalled();
    expect(controller.sendText).not.toHaveBeenCalled();
    expect(onPasteShortcut).not.toHaveBeenCalled();

    subscription.dispose();
    element.remove();
  });
});

describe("writeBrowserClipboardText", () => {
  it("falls back to execCommand copy when async clipboard write is rejected", async () => {
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const originalExecCommand = Object.getOwnPropertyDescriptor(document, "execCommand");
    const writeText = vi.fn().mockRejectedValue(new Error("denied"));
    const execCommand = vi.fn(() => true);

    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    Object.defineProperty(document, "execCommand", {
      configurable: true,
      value: execCommand,
    });

    try {
      await writeBrowserClipboardText("remote clipboard");
    } finally {
      if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
      else Reflect.deleteProperty(navigator, "clipboard");
      if (originalExecCommand) Object.defineProperty(document, "execCommand", originalExecCommand);
      else Reflect.deleteProperty(document, "execCommand");
    }

    expect(writeText).toHaveBeenCalledWith("remote clipboard");
    expect(execCommand).toHaveBeenCalledWith("copy");
  });

  it("falls back to execCommand copy when async clipboard write never settles", async () => {
    vi.useFakeTimers();
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const originalExecCommand = Object.getOwnPropertyDescriptor(document, "execCommand");
    const writeText = vi.fn(() => new Promise<void>(() => {}));
    const execCommand = vi.fn(() => true);

    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    Object.defineProperty(document, "execCommand", {
      configurable: true,
      value: execCommand,
    });

    try {
      const copy = writeBrowserClipboardText("remote clipboard");
      await vi.advanceTimersByTimeAsync(600);
      await copy;
    } finally {
      vi.useRealTimers();
      if (originalClipboard) Object.defineProperty(navigator, "clipboard", originalClipboard);
      else Reflect.deleteProperty(navigator, "clipboard");
      if (originalExecCommand) Object.defineProperty(document, "execCommand", originalExecCommand);
      else Reflect.deleteProperty(document, "execCommand");
    }

    expect(writeText).toHaveBeenCalledWith("remote clipboard");
    expect(execCommand).toHaveBeenCalledWith("copy");
  });
});
