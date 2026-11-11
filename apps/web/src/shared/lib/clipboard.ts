const ASYNC_CLIPBOARD_WRITE_TIMEOUT_MS = 500;

async function awaitClipboardWriteAttempt(write: Promise<void>): Promise<void> {
  let timeoutId: number | undefined;
  write.catch(() => undefined);
  try {
    await Promise.race([
      write,
      new Promise<never>((_, reject) => {
        timeoutId = window.setTimeout(
          () => reject(new Error("Browser clipboard write timed out")),
          ASYNC_CLIPBOARD_WRITE_TIMEOUT_MS,
        );
      }),
    ]);
  } finally {
    if (timeoutId != null) window.clearTimeout(timeoutId);
  }
}

export async function writeBrowserClipboardText(text: string): Promise<void> {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    try {
      await awaitClipboardWriteAttempt(navigator.clipboard.writeText(text));
      return;
    } catch {
      // Fall through to the legacy path. Chromium can reject async clipboard
      // writes when the call is not considered a user gesture, and some
      // embedded browser contexts leave the promise pending instead of failing.
    }
  }

  if (typeof document === "undefined" || typeof document.execCommand !== "function") {
    throw new Error("Browser clipboard write is unavailable");
  }

  const activeElement = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "true");
  textarea.style.position = "fixed";
  textarea.style.left = "-9999px";
  textarea.style.top = "0";
  document.body.append(textarea);
  textarea.focus();
  textarea.select();

  try {
    if (!document.execCommand("copy")) {
      throw new Error("Browser clipboard write was rejected");
    }
  } finally {
    textarea.remove();
    activeElement?.focus();
  }
}
