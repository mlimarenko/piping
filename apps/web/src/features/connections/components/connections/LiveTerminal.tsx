import { useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { useTranslation } from "react-i18next";
import {
  ArrowDown,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  Keyboard,
  KeyboardOff,
  Loader2,
  Minus,
  MoreHorizontal,
  Plus,
  Send,
} from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui-primitives/popover";
import type { ConnectionFailure } from "@/features/connections/connectionFailure";
import { connectionFailureFromWsClose } from "@/features/connections/connectionFailure";
import ConnectionFailureOverlay from "@/features/connections/components/connections/ConnectionFailureOverlay";
import {
  attachSshTerminal,
  SSH_FONT_SIZE_DEFAULT,
  SSH_FONT_SIZE_MAX,
  SSH_FONT_SIZE_MIN,
  type SshAdapter,
  type SshConnectionStatus,
  type SshTerminalKey,
} from "@/features/connections/adapters/ws/sshAdapter";
import { isSessionWsConnectable } from "@/features/connections/adapters/ws/urls";
import { useSessionTouchLayout } from "@/features/connections/hooks/useSessionViewport";

interface LiveTerminalProps {
  websocketUrl: string;
  token: string;
  isActive: boolean;
  isVisible?: boolean;
  /** Called when the WebSocket successfully opens. */
  onOpen?: () => void;
  /** Called when the websocket closes so the caller can reflect status. */
  onClose?: (code?: number, reason?: string) => void;
  onControllerChange?: (controller: SshTerminalController | null) => void;
  /**
   * Re-open this session. When provided, the disconnected overlay shows a
   * Reconnect button wired to the same `reconnectSession` flow used by the
   * panel's failed-state button — there is no second reconnect mechanism.
   */
  onReconnect?: () => void;
  /** Host label shown in the localized failure overlay (e.g. `host · SSH`). */
}

export interface SshTerminalController {
  focus: () => void;
  isLive: () => boolean;
  sendText: (data: string) => boolean;
}

/** How the mobile session sends input: a local command field or the live PTY. */
type TerminalInputMode = "command" | "direct";

/** Clamp a font size to the adapter's bounded range (single source of truth). */
function clampFontSize(size: number): number {
  return Math.min(SSH_FONT_SIZE_MAX, Math.max(SSH_FONT_SIZE_MIN, Math.round(size)));
}

/**
 * xterm.js-backed SSH viewer. Mounts a terminal into the first child `<div>`
 * and wires it to a reconnecting WebSocket via {@link attachSshTerminal}.
 *
 * The backend expects `wss://…/api/v1/ws/ssh/{session_id}?token=…` with
 * `Sec-WebSocket-Protocol: guacamole`; {@link attachSshTerminal} handles both.
 *
 * On a transient WS drop the backend keeps the SSH session alive and the
 * adapter reconnects in place. We reflect its state machine here:
 *   - `reconnecting-silent`  (0–10s): no UI; xterm keeps its frozen buffer.
 *   - `reconnecting-visible` (10–60s): non-modal "Reconnecting…" banner; the
 *     adapter blocks terminal input.
 *   - `failed`               (>60s): full-screen disconnected overlay + onClose.
 *
 */
const LiveTerminal = ({
  websocketUrl,
  token,
  isActive,
  isVisible = isActive,
  onOpen,
  onClose,
  onControllerChange,
  onReconnect,
}: LiveTerminalProps) => {
  const { t } = useTranslation();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const adapterRef = useRef<SshAdapter | null>(null);
  const commandInputRef = useRef<HTMLInputElement | null>(null);
  const [failure, setFailure] = useState<ConnectionFailure | null>(null);
  const [status, setStatus] = useState<SshConnectionStatus>("live");
  const [fontSize, setFontSize] = useState(SSH_FONT_SIZE_DEFAULT);
  const fontSizeRef = useRef(SSH_FONT_SIZE_DEFAULT);
  const [inputMode, setInputMode] = useState<TerminalInputMode>("command");
  const [moreOpen, setMoreOpen] = useState(false);
  // Reflected from real DOM focus (focusin/focusout) rather than a bare toggle,
  // so dismissing the OS keyboard by any means resets the show/hide button.
  const [keyboardActive, setKeyboardActive] = useState(false);
  const mobile = useSessionTouchLayout();

  // Latest mobile/mode values for effects that must not re-run (and therefore
  // reconnect) when they change. Declared before the attach effect so a fresh
  // attach already sees the right soft-keyboard policy.
  const policyRef = useRef({ mobile, inputMode });
  useEffect(() => {
    policyRef.current = { mobile, inputMode };
  });

  // Guard against degraded/stale session data (empty token or a host-only URL
  // with no relay path). Opening a socket for those yields a bogus
  // `wss://host/?token=` the browser can never connect, spamming the console.
  // Derived during render (not via setState-in-effect) so we both skip the
  // attach below and surface the disconnected overlay.
  const connectable = isSessionWsConnectable(websocketUrl, token);

  // Callback props are invoked by the reconnecting SSH adapter for the life of
  // the connection; keeping them in a ref (updated after every render) stops
  // the attach effect from disposing and reconnecting when their identity
  // changes, and avoids capturing a stale callback.
  const callbacksRef = useRef({ onOpen, onClose, onControllerChange });
  useEffect(() => {
    callbacksRef.current = { onOpen, onClose, onControllerChange };
  });

  useEffect(() => {
    if (!containerRef.current || !connectable) return;
    let adapter: SshAdapter | null = null;
    try {
      adapter = attachSshTerminal({
        container: containerRef.current,
        url: websocketUrl,
        token,
        onOpen: () => callbacksRef.current.onOpen?.(),
        onStatus: setStatus,
        onClose: (code, reason) => {
          // The adapter only calls onClose on a terminal close (clean 1000 or
          // reconnect give-up). Non-1000 → show the LOCALIZED disconnected
          // overlay. The shared classifier maps (code, reason) → i18n keys so
          // the user never sees the raw English backend reason string.
          if (code !== undefined && code !== 1000) {
            setFailure(connectionFailureFromWsClose(code, reason));
          }
          callbacksRef.current.onClose?.(code, reason);
        },
      });
      adapterRef.current = adapter;
      adapter.setFontSize(fontSizeRef.current);
      // Mobile command mode must not let the terminal summon the OS keyboard.
      adapter.setSoftKeyboardEnabled(!policyRef.current.mobile || policyRef.current.inputMode === "direct");
      callbacksRef.current.onControllerChange?.({
        focus: () => adapter?.focus(),
        isLive: () => adapter?.isLive() ?? false,
        sendText: (data) => adapter?.sendText(data) ?? false,
      });
    } catch (err) {
      console.error("[LiveTerminal] attach failed", err);
    }
    return () => {
      callbacksRef.current.onControllerChange?.(null);
      try {
        adapter?.dispose();
      } catch {
        // ignore — component unmount should not throw
      }
      adapterRef.current = null;
    };
  }, [connectable, token, websocketUrl]);

  // Apply input-policy changes (command ⇄ direct, desktop ⇄ mobile) to the live
  // adapter WITHOUT re-attaching: the socket, scrollback and scroll position all
  // survive. The adapter only flips the hidden textarea's inputMode.
  useEffect(() => {
    adapterRef.current?.setSoftKeyboardEnabled(!mobile || inputMode === "direct");
  }, [mobile, inputMode, connectable]);

  // When this tab/panel becomes active its container gets a real box again.
  // Double-RAF waits for the browser layout+paint pass after the React commit,
  // and a trailing timeout covers slow layouts; fit() now also propagates the
  // new cols/rows to the PTY so fullscreen TUIs (mc, htop) fill the area.
  // A mobile session is NOT focused here: that is what popped the OS keyboard
  // unprompted. The user opens it explicitly from the touch bar.
  useEffect(() => {
    if (!isActive) {
      commandInputRef.current?.blur();
      adapterRef.current?.blur();
      return;
    }
    let raf2 = 0;
    let timer = 0;
    const raf1 = window.requestAnimationFrame(() => {
      raf2 = window.requestAnimationFrame(() => {
        adapterRef.current?.fit();
        if (!policyRef.current.mobile) adapterRef.current?.focus();
      });
    });
    timer = window.setTimeout(() => adapterRef.current?.fit(), 120);
    return () => {
      window.cancelAnimationFrame(raf1);
      window.cancelAnimationFrame(raf2);
      window.clearTimeout(timer);
    };
  }, [isActive]);

  useEffect(() => {
    if (isVisible) adapterRef.current?.fit();
  }, [isVisible]);

  // A non-connectable session (stale/degraded url+token) never attaches, so
  // surface the disconnected overlay for it just like a real failure.
  const effectiveFailure = connectable ? failure : { kind: "configuration" as const };
  const hasError = effectiveFailure !== null;

  // Non-modal "Reconnecting…" banner during the visible reconnect phase. The
  // adapter has already blocked terminal input; we dim the terminal slightly so
  // the frozen-but-intact scrollback stays readable without looking live.
  const showReconnectBanner = status === "reconnecting-visible" && !hasError;

  const controlsDisabled = !connectable || hasError || status !== "live";
  const directInput = inputMode === "direct";

  const showKeyboard = () => {
    if (mobile && directInput) {
      adapterRef.current?.focus();
      return;
    }
    // iOS requires focus in this user gesture, not a deferred effect.
    flushSync(() => setKeyboardActive(true));
    commandInputRef.current?.focus();
  };

  const hideKeyboard = () => {
    // Blur BOTH possible owners so an open OS keyboard always retracts.
    commandInputRef.current?.blur();
    adapterRef.current?.blur();
    setKeyboardActive(false);
  };

  const applyFontSize = (next: number) => {
    const clamped = clampFontSize(next);
    const applied = adapterRef.current?.setFontSize(clamped) ?? clamped;
    fontSizeRef.current = applied;
    setFontSize(applied);
  };

  // One command + CR goes out through the existing sendText bridge. The local
  // field is not an echo: the remote's own output is the only confirmation.
  const sendCommand = () => {
    if (!isActive) return;
    const field = commandInputRef.current;
    const command = field?.value ?? "";
    if (!command) return;
    if (adapterRef.current?.sendText(`${command}\r`)) {
      if (field) field.value = "";
    }
  };

  const secondaryKeys: ReadonlyArray<readonly [SshTerminalKey, string, string]> = [
    ["escape", "Esc", "escape"],
    ["tab", "Tab", "tab"],
    ["left", "", "left"],
    ["down", "", "down"],
    ["up", "", "up"],
    ["right", "", "right"],
    ["pageUp", "PgUp", "pageUp"],
    ["pageDown", "PgDn", "pageDown"],
  ];

  const arrowIcon = (key: string) => {
    if (key === "left") return <ArrowLeft className="size-4" aria-hidden="true" />;
    if (key === "down") return <ArrowDown className="size-4" aria-hidden="true" />;
    if (key === "up") return <ArrowUp className="size-4" aria-hidden="true" />;
    return <ArrowRight className="size-4" aria-hidden="true" />;
  };

  return (
    <div
      className="relative flex h-full w-full min-h-0 min-w-0 flex-col overflow-hidden bg-[#0b0d10]"
      onFocusCapture={(event) => {
        if (!mobile) return;
        const target = event.target as Node | null;
        // Only the command field and the live terminal count as "keyboard
        // active" — toolbar buttons must not make the toggle believe the OS
        // keyboard is open.
        if (target && (target === commandInputRef.current || (directInput && containerRef.current?.contains(target)))) {
          setKeyboardActive(true);
        }
      }}
      onBlurCapture={(event) => {
        if (!mobile) return;
        const next = event.relatedTarget as Node | null;
        const staysInInput = next && (next === commandInputRef.current || (directInput && containerRef.current?.contains(next)));
        if (!staysInInput) setKeyboardActive(false);
      }}
    >
      <div
        ref={containerRef}
        className={`min-h-0 w-full min-w-0 flex-1 overflow-hidden bg-[#0b0d10] transition-opacity ${
          showReconnectBanner ? "opacity-60" : "opacity-100"
        }`}
        data-testid="live-terminal"
      />
      <form
        className={isActive && keyboardActive && !directInput ? "flex shrink-0 items-center gap-1.5 border-t border-border bg-background p-1.5 md:hidden pointer-coarse:flex" : "hidden"}
        onSubmit={(event) => { event.preventDefault(); sendCommand(); }}>
        <Input ref={commandInputRef} className="h-11 min-w-0 flex-1 text-[16px]"
          type="text" name="terminal-command" inputMode="text" enterKeyHint="send"
          autoCorrect="off" autoCapitalize="none" autoComplete="off" spellCheck={false}
          placeholder={t("connections.terminal.commandPlaceholder")}
          aria-label={t("connections.terminal.commandInput")} disabled={controlsDisabled}
          onKeyDown={(event) => {
            if (event.key === "Enter" && event.nativeEvent.isComposing) event.preventDefault();
          }} />
        <Button type="submit" variant="outline" size="icon" className="size-11 shrink-0"
          onPointerDown={(event) => event.preventDefault()}
          aria-label={t("connections.terminal.sendCommand")} disabled={controlsDisabled}>
          <Send className="size-4" aria-hidden="true" />
        </Button>
      </form>
      {/* One compact row. Hidden on fine-pointer desktops (md and up) where the
          physical keyboard types straight into xterm. */}
      <div
        role="toolbar"
        aria-label={t("connections.terminal.touchControls")}
        className={isActive ? "flex shrink-0 items-center gap-1.5 border-t border-border bg-background p-1.5 pb-[max(0.375rem,env(safe-area-inset-bottom))] md:hidden pointer-coarse:flex" : "hidden"}
      >
        <Button
          variant="outline"
          size="icon"
          className="size-11 shrink-0"
          aria-label={keyboardActive ? t("connections.terminal.keyboardHide") : t("connections.terminal.keyboardShow")}
          aria-pressed={keyboardActive}
          disabled={controlsDisabled}
          onPointerDown={(event) => event.preventDefault()}
          onClick={() => (keyboardActive ? hideKeyboard() : showKeyboard())}
        >
          {keyboardActive ? (
            <KeyboardOff className="size-4" aria-hidden="true" />
          ) : (
            <Keyboard className="size-4" aria-hidden="true" />
          )}
        </Button>

        {directInput && (
          <span
            className="min-h-11 min-w-0 flex-1 truncate rounded-md border border-border/60 px-2 text-xs leading-[2.75] text-muted-foreground"
            data-testid="live-terminal-direct-input"
          >
            {t("connections.terminal.directInputHint")}
          </span>
        )}

        <Button
          variant="outline"
          className="min-h-11 shrink-0 px-2 text-xs"
          aria-label={t("connections.terminal.interrupt")}
          disabled={controlsDisabled}
          onPointerDown={(event) => event.preventDefault()}
          onClick={() => {
            if (!directInput && commandInputRef.current) commandInputRef.current.value = "";
            adapterRef.current?.sendKey("interrupt");
          }}
        >
          Ctrl+C
        </Button>
        <Button
          variant="outline"
          size="icon"
          className="size-11 shrink-0"
          aria-label={t("connections.terminal.enter")}
          disabled={controlsDisabled}
          onPointerDown={(event) => event.preventDefault()}
          onClick={() => {
            if (!directInput && commandInputRef.current?.value) sendCommand();
            else adapterRef.current?.sendKey("enter");
          }}
        >
          ↵
        </Button>

        <Popover>
          <PopoverTrigger asChild>
            <Button
              variant="outline"
              size="icon"
              className="size-11 shrink-0"
              aria-label={t("connections.terminal.fontSize")}
              disabled={controlsDisabled}
              onPointerDown={(event) => event.preventDefault()}
            >
              <span className="text-xs font-medium" aria-hidden="true">Aa</span>
            </Button>
          </PopoverTrigger>
          <PopoverContent align="end" className="w-auto p-2">
            <fieldset className="flex items-center gap-1" aria-label={t("connections.terminal.fontSize")}>
              <Button
                variant="outline"
                size="icon"
                className="size-11"
                aria-label={t("connections.terminal.fontSizeDecrease")}
                disabled={controlsDisabled || fontSize <= SSH_FONT_SIZE_MIN}
                onClick={() => applyFontSize(fontSize - 1)}
              >
                <Minus className="size-4" aria-hidden="true" />
              </Button>
              <span className="min-w-10 text-center text-xs tabular-nums" data-testid="live-terminal-font-size">
                {fontSize}px
              </span>
              <Button
                variant="outline"
                size="icon"
                className="size-11"
                aria-label={t("connections.terminal.fontSizeIncrease")}
                disabled={controlsDisabled || fontSize >= SSH_FONT_SIZE_MAX}
                onClick={() => applyFontSize(fontSize + 1)}
              >
                <Plus className="size-4" aria-hidden="true" />
              </Button>
            </fieldset>
          </PopoverContent>
        </Popover>

        <Popover open={moreOpen} onOpenChange={setMoreOpen}>
          <PopoverTrigger asChild>
            <Button
              variant="outline"
              size="icon"
              className="size-11 shrink-0"
              aria-label={t("connections.terminal.moreControls")}
              disabled={controlsDisabled}
              onPointerDown={(event) => event.preventDefault()}
            >
              <MoreHorizontal className="size-4" aria-hidden="true" />
            </Button>
          </PopoverTrigger>
          <PopoverContent align="end" className="w-auto p-2">
            <fieldset className="grid grid-cols-4 gap-1.5" aria-label={t("connections.terminal.moreControls")}>
              {secondaryKeys.map(([key, label, translation]) => (
                <Button
                  key={key}
                  variant="outline"
                  className="min-h-11 px-2 text-xs"
                  aria-label={t(`connections.terminal.${translation}`)}
                  disabled={controlsDisabled}
                  onPointerDown={(event) => event.preventDefault()}
                  onClick={() => adapterRef.current?.sendKey(key)}
                >
                  {label || arrowIcon(key)}
                </Button>
              ))}
            </fieldset>
            <div className="mt-2 border-t border-border/60 pt-2">
              <Button
                variant={directInput ? "secondary" : "outline"}
                className="min-h-11 w-full justify-start px-2 text-xs"
                aria-pressed={directInput}
                disabled={controlsDisabled}
                onClick={() => {
                  setInputMode(directInput ? "command" : "direct");
                  setMoreOpen(false);
                }}
              >
                <Keyboard className="size-4" aria-hidden="true" />
                {t("connections.terminal.directInput")}
              </Button>
            </div>
          </PopoverContent>
        </Popover>
      </div>
      {showReconnectBanner && (
        <div
          className="absolute left-1/2 top-3 -translate-x-1/2 flex items-center gap-2 rounded-md bg-[#0b0d10]/90 px-3 py-1.5 shadow-md"
          data-testid="live-terminal-reconnecting"
        >
          <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          <span className="text-xs font-medium text-muted-foreground">
            {t("connections.sshReconnecting")}
          </span>
        </div>
      )}
      {effectiveFailure && (
        <ConnectionFailureOverlay failure={effectiveFailure} onRetry={onReconnect} />
      )}
    </div>
  );
};

export default LiveTerminal;
