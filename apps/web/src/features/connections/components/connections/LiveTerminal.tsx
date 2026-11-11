import { useEffect, useLayoutEffect, useRef, useState } from "react";
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
  Plus,
  RotateCcw,
  Send,
} from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
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
  type SshKeyModifiers,
  type SshTerminalKey,
} from "@/features/connections/adapters/ws/sshAdapter";
import { isSessionWsConnectable } from "@/features/connections/adapters/ws/urls";
import { useSessionTouchLayout } from "@/features/connections/hooks/useSessionViewport";

interface LiveTerminalProps {
  websocketUrl: string;
  token: string;
  readOnly?: boolean;
  isActive: boolean;
  focusRequest?: number;
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
type TerminalInputOwner = TerminalInputMode | "chord" | null;

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
  readOnly = false,
  isActive,
  focusRequest = 0,
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
  const chordInputRef = useRef<HTMLInputElement | null>(null);
  const keypadRef = useRef<HTMLDivElement | null>(null);
  const [failure, setFailure] = useState<ConnectionFailure | null>(null);
  const [status, setStatus] = useState<SshConnectionStatus>("live");
  const [fontSize, setFontSize] = useState(SSH_FONT_SIZE_DEFAULT);
  const fontSizeRef = useRef(SSH_FONT_SIZE_DEFAULT);
  const [inputMode, setInputMode] = useState<TerminalInputMode>("command");
  const [moreOpen, setMoreOpen] = useState(false);
  const [modifiers, setModifiers] = useState<SshKeyModifiers>({});
  const [chord, setChord] = useState("");
  const [keyNotSent, setKeyNotSent] = useState(false);
  const composingRef = useRef(false);
  const gateRef = useRef({ isActive, connectable: false, status });
  // A focused chord owns the native keyboard, but must not open the command row.
  const [keyboardOwner, setKeyboardOwner] = useState<TerminalInputOwner>(null);
  const keyboardActive = keyboardOwner !== null;
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
  const connectable = readOnly ? Boolean(websocketUrl) : isSessionWsConnectable(websocketUrl, token);
  useLayoutEffect(() => { gateRef.current = { isActive, connectable, status }; }, [isActive, connectable, status]);

  // Callback props are invoked by the reconnecting SSH adapter for the life of
  // the connection; keeping them in a ref (updated after every render) stops
  // the attach effect from disposing and reconnecting when their identity
  // changes, and avoids capturing a stale callback.
  const callbacksRef = useRef({ onOpen, onClose, onControllerChange });
  useEffect(() => {
    callbacksRef.current = { onOpen, onClose, onControllerChange };
  });

  useEffect(() => {
    // A replacement adapter starts in live status and may not emit onStatus on its first open.
    gateRef.current.status = "live";
    policyRef.current.inputMode = "command";
    // Synchronize retained UI state with the newly attached external session.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setStatus("live");
    setFailure(null);
    setInputMode("command");
    setMoreOpen(false);
    setModifiers({});
    setChord("");
    setKeyNotSent(false);
    setKeyboardOwner(null);
    composingRef.current = false;
    commandInputRef.current?.blur();
    chordInputRef.current?.blur();
    if (commandInputRef.current) commandInputRef.current.value = "";
    if (!containerRef.current || !connectable) return;
    let adapter: SshAdapter | null = null;
    try {
      adapter = attachSshTerminal({
        container: containerRef.current,
        url: websocketUrl,
        token,
        readOnly,
        onOpen: () => callbacksRef.current.onOpen?.(),
        onStatus: (next) => { gateRef.current.status = next; if (next !== "live") setModifiers({}); setStatus(next); },
        onClose: (code, reason) => {
          setModifiers({});
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
      adapter.setSoftKeyboardEnabled(!readOnly && !policyRef.current.mobile);
      if (!readOnly) callbacksRef.current.onControllerChange?.({
        focus: () => { if (gateRef.current.isActive && gateRef.current.status === "live" && adapter?.isLive()) adapter.focus(); },
        isLive: () => gateRef.current.isActive && gateRef.current.status === "live" && (adapter?.isLive() ?? false),
        sendText: (data) => gateRef.current.isActive && gateRef.current.status === "live" && !!adapter?.isLive() && adapter.sendText(data),
      });
    } catch (err) {
      console.error("[LiveTerminal] attach failed", err);
    }
    return () => {
      if (!readOnly) callbacksRef.current.onControllerChange?.(null);
      try {
        adapter?.dispose();
      } catch {
        // ignore — component unmount should not throw
      }
      adapterRef.current = null;
    };
  }, [connectable, token, websocketUrl, readOnly]);

  // Apply input-policy changes (command ⇄ direct, desktop ⇄ mobile) to the live
  // adapter WITHOUT re-attaching: the socket, scrollback and scroll position all
  // survive. The adapter only flips the hidden textarea's inputMode.
  useEffect(() => {
    adapterRef.current?.setSoftKeyboardEnabled(!readOnly && (!mobile || inputMode === "direct"));
  }, [mobile, inputMode, connectable, readOnly]);

  // When this tab/panel becomes active its container gets a real box again.
  // Double-RAF waits for the browser layout+paint pass after the React commit,
  // and a trailing timeout covers slow layouts; fit() now also propagates the
  // new cols/rows to the PTY so fullscreen TUIs (mc, htop) fill the area.
  // A mobile session is NOT focused here: that is what popped the OS keyboard
  // unprompted. The user opens it explicitly from the touch bar.
  useEffect(() => {
    if (!isActive) {
      // Reset transient input state as soon as this retained session loses focus.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setModifiers({});
      setKeyboardOwner(null);
      composingRef.current = false;
      commandInputRef.current?.blur();
      chordInputRef.current?.blur();
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
  }, [isActive, focusRequest]);

  useLayoutEffect(() => {
    const keypad = keypadRef.current;
    const input = chordInputRef.current;
    if (!isActive || !moreOpen || keyboardOwner !== "chord" || !keypad || !input) return;
    const revealInput = () => {
      if (document.activeElement !== input) return;
      const bounds = keypad.getBoundingClientRect();
      const field = input.getBoundingClientRect();
      const top = bounds.top + keypad.clientTop;
      const bottom = top + keypad.clientHeight;
      if (field.bottom > bottom) keypad.scrollTop += field.bottom - bottom;
      else if (field.top < top) keypad.scrollTop += field.top - top;
    };
    // Observe the actual scrollport after the shared viewport owner changes its
    // geometry. Scroll only this keypad, never the page or another session.
    revealInput();
    const observer = new ResizeObserver(revealInput);
    observer.observe(keypad);
    observer.observe(input);
    return () => observer.disconnect();
  }, [isActive, moreOpen, keyboardOwner]);

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

  const inputDisabled = !connectable || hasError || status !== "live";
  const controlsDisabled = !isActive || inputDisabled;
  const directInput = inputMode === "direct";
  const canSend = () => !readOnly && isActive && !controlsDisabled && !!adapterRef.current?.isLive();
  const inputOwner = (target: EventTarget | null): TerminalInputOwner => {
    if (!target) return null;
    if (target === commandInputRef.current) return "command";
    if (target === chordInputRef.current) return "chord";
    return directInput && target instanceof Node && containerRef.current?.contains(target) ? "direct" : null;
  };

  const sendKey = (key: SshTerminalKey, fixed = false) => {
    if (!canSend()) { setKeyNotSent(true); return false; }
    const sent = adapterRef.current?.sendKey(key, fixed ? {} : modifiers) ?? false;
    setKeyNotSent(!sent);
    setModifiers({});
    return sent;
  };

  const showKeyboard = () => {
    if (!canSend()) return;
    if (mobile && directInput) {
      adapterRef.current?.focus();
      return;
    }
    // iOS requires focus in this user gesture, not a deferred effect.
    flushSync(() => setKeyboardOwner("command"));
    commandInputRef.current?.focus();
  };

  const hideKeyboard = () => {
    // Blur every native input owner without discarding the command draft.
    commandInputRef.current?.blur();
    chordInputRef.current?.blur();
    adapterRef.current?.blur();
    composingRef.current = false;
    setKeyboardOwner(null);
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
    if (!canSend() || composingRef.current || keyboardOwner !== "command") return;
    const field = commandInputRef.current;
    const command = field?.value ?? "";
    if (!command) return;
    if (adapterRef.current?.sendText(`${command}\r`)) {
      if (field) field.value = "";
      setModifiers({});
    }
  };

  const secondaryKeys: ReadonlyArray<readonly [SshTerminalKey, string, string]> = [
    ["escape", "Esc", "escape"],
    ["tab", "Tab", "tab"],
    ["left", "", "left"],
    ["down", "", "down"],
    ["up", "", "up"],
    ["right", "", "right"],
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
        setKeyboardOwner(inputOwner(event.target));
      }}
      onBlurCapture={(event) => {
        if (!mobile) return;
        setKeyboardOwner(inputOwner(event.relatedTarget));
      }}
    >
      {/* Observation preserves the operator's grid, which may exceed this pane
          in both dimensions. Scroll it instead of fitting and resizing the PTY. */}
      <div
        ref={containerRef}
        className={`min-h-0 w-full min-w-0 flex-1 bg-[#0b0d10] transition-opacity ${
          readOnly ? "overflow-auto" : "overflow-hidden"
        } ${showReconnectBanner ? "opacity-60" : "opacity-100"}`}
        data-testid="live-terminal"
      />
      {!readOnly && <><form
        hidden={!isActive || keyboardOwner !== "command" || directInput}
        className={isActive && keyboardOwner === "command" && !directInput ? "flex shrink-0 items-center gap-1.5 border-t border-border bg-background p-1.5 md:hidden pointer-coarse:flex" : "hidden"}
        onSubmit={(event) => { event.preventDefault(); sendCommand(); }}>
        <Input ref={commandInputRef} className="h-11 min-w-0 flex-1 text-[16px]"
          type="text" name="terminal-command" inputMode="text" enterKeyHint="send"
          autoCorrect="off" autoCapitalize="none" autoComplete="off" spellCheck={false}
          placeholder={t("connections.terminal.commandPlaceholder")}
          aria-label={t("connections.terminal.commandInput")} disabled={inputDisabled}
          onCompositionStart={() => { composingRef.current = true; }}
          onCompositionEnd={() => { composingRef.current = false; }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && event.nativeEvent.isComposing) event.preventDefault();
          }} />
        <Button type="submit" variant="outline" size="icon" className="size-11 shrink-0"
          onPointerDown={(event) => event.preventDefault()}
          aria-label={t("connections.terminal.sendCommand")} disabled={controlsDisabled}>
          <Send className="size-4" aria-hidden="true" />
        </Button>
      </form>
      <div role="toolbar" aria-label={t("connections.terminal.touchControls")}
        className={isActive ? "flex max-h-[40%] shrink-0 min-h-0 flex-col border-t border-border bg-background pb-[max(0.375rem,env(safe-area-inset-bottom))] md:hidden pointer-coarse:flex" : "hidden"}>
        <div className="flex shrink-0 min-w-0 items-center gap-1 overflow-x-auto p-1">
          <Button variant="outline" size="icon" className="size-11 shrink-0"
            aria-label={keyboardActive ? t("connections.terminal.keyboardHide") : t("connections.terminal.keyboardShow")}
            aria-pressed={keyboardActive} disabled={controlsDisabled}
            onPointerDown={(event) => event.preventDefault()}
            onClick={() => (keyboardActive ? hideKeyboard() : showKeyboard())}>
            {keyboardActive ? <KeyboardOff className="size-4" /> : <Keyboard className="size-4" />}
          </Button>
          <Button variant="outline" className="min-h-11 shrink-0 px-2 text-xs"
            aria-label={t("connections.terminal.moreControls")} aria-expanded={moreOpen}
            disabled={controlsDisabled} onPointerDown={(event) => event.preventDefault()}
            onClick={() => {
              if (moreOpen) { setModifiers({}); chordInputRef.current?.blur(); composingRef.current = false; }
              setMoreOpen(!moreOpen);
            }}> {t("connections.terminal.keypad")} </Button>
          {secondaryKeys.map(([key, label, translation]) => (
            <Button key={translation} variant="outline" className="min-h-11 min-w-11 shrink-0 px-2 text-xs"
              aria-label={t(`connections.terminal.${translation}`)} disabled={controlsDisabled}
              onPointerDown={(event) => event.preventDefault()} onClick={() => sendKey(key)}>
              {label || arrowIcon(translation)}
            </Button>
          ))}
          <Button variant="outline" className="min-h-11 shrink-0 px-2 text-xs"
            aria-label={t("connections.terminal.interrupt")} disabled={controlsDisabled}
            onPointerDown={(event) => event.preventDefault()} onClick={() => {
              if (!canSend()) return;
              if (keyboardOwner === "command" && commandInputRef.current) commandInputRef.current.value = "";
              sendKey("interrupt", true);
            }}>Ctrl+C</Button>
          <Button variant="outline" className="min-h-11 min-w-11 shrink-0" aria-label={t("connections.terminal.enter")}
            disabled={controlsDisabled} onPointerDown={(event) => event.preventDefault()} onClick={() => {
              if (!canSend() || composingRef.current) return;
              if (keyboardOwner === "command" && commandInputRef.current?.value) sendCommand();
              else sendKey("enter");
            }}>↵</Button>
        </div>
        {moreOpen && <div ref={keypadRef} className="min-h-0 overflow-y-auto border-t border-border p-1" data-testid="terminal-keypad">
          <p className="text-xs text-muted-foreground">{t("connections.terminal.modifiersOneShot")}</p>
          <div className="flex items-center gap-1 pt-1">
            {(["ctrl", "alt", "shift"] as const).map((modifier) => <Button key={modifier} variant={modifiers[modifier] ? "secondary" : "outline"}
              className="size-11 shrink-0 px-1 text-xs" aria-pressed={!!modifiers[modifier]} disabled={controlsDisabled}
              onPointerDown={(event) => event.preventDefault()}
              onClick={() => { if (canSend()) setModifiers((current) => ({ ...current, [modifier]: !current[modifier] })); }}>
              {modifier === "ctrl" ? "Ctrl" : modifier === "alt" ? "Alt" : "Shift"}
            </Button>)}
            <Button variant="outline" size="icon" className="size-11 shrink-0" disabled={controlsDisabled}
              aria-label={t("connections.terminal.clearModifiers")} title={t("connections.terminal.clearModifiers")}
              onPointerDown={(event) => event.preventDefault()} onClick={() => setModifiers({})}>
              <RotateCcw className="size-4" aria-hidden="true" />
            </Button>
            <form className="flex shrink-0 items-center gap-1" onSubmit={(event) => {
              event.preventDefault();
              if (!canSend() || composingRef.current || !/^[^\p{C}\p{M}\p{Zl}\p{Zp}]$/u.test(chord)) return;
              if (sendKey({ character: chord })) setChord("");
            }}>
              <Input ref={chordInputRef} className="size-11 min-w-11 shrink-0 px-1 text-center text-[16px]" aria-label={t("connections.terminal.chordCharacter")}
                type="text" inputMode="text" autoCorrect="off" autoCapitalize="none" autoComplete="off" spellCheck={false}
                value={chord} onChange={(event) => setChord(event.target.value)} disabled={inputDisabled}
                onCompositionStart={() => { composingRef.current = true; }} onCompositionEnd={() => { composingRef.current = false; }}
                onKeyDown={(event) => { if (event.key === "Enter" && event.nativeEvent.isComposing) event.preventDefault(); }} />
              <Button type="submit" variant="outline" size="icon" className="size-11 shrink-0" disabled={controlsDisabled}
                aria-label={t("connections.terminal.sendChord")} title={t("connections.terminal.sendChord")}
                onPointerDown={(event) => event.preventDefault()}><Send className="size-4" aria-hidden="true" /></Button>
            </form>
          </div>
          <div className="flex flex-wrap gap-1 pt-1">
            {([...["home", "end", "pageUp", "pageDown", "insert", "delete", "backspace"], ...Array.from({ length: 12 }, (_, i) => `f${i + 1}`)] as Exclude<SshTerminalKey, object>[]).map((key) => {
              const name = key;
              const label = ({ pageUp: "PgUp", pageDown: "PgDn", insert: "Ins", delete: "Del", backspace: "⌫" } as Record<string, string>)[name] ?? name.toUpperCase();
              return <Button key={name} variant="outline" className="min-h-11 min-w-11 px-2 text-xs"
                aria-label={t(`connections.terminal.key.${name}`)} disabled={controlsDisabled}
                onPointerDown={(event) => event.preventDefault()} onClick={() => sendKey(key)}>{label}</Button>;
            })}
          </div>
          <div className="flex flex-wrap items-center gap-1 border-t border-border pt-1">
            <span className="text-xs text-muted-foreground" role="status">{t(directInput ? "connections.terminal.modeDirect" : "connections.terminal.modeCommand")}</span>
            <Button variant="outline" className="min-h-11 px-2 text-xs" disabled={controlsDisabled}
              onPointerDown={(event) => event.preventDefault()} onClick={() => {
                hideKeyboard(); composingRef.current = false; setModifiers({}); setInputMode(directInput ? "command" : "direct");
              }}>{t(directInput ? "connections.terminal.commandMode" : "connections.terminal.directInput")}</Button>
            <fieldset className="flex items-center gap-1" aria-label={t("connections.terminal.fontSize")}>
              <Button variant="outline" size="icon" className="size-11" aria-label={t("connections.terminal.fontSizeDecrease")}
                disabled={controlsDisabled || fontSize <= SSH_FONT_SIZE_MIN} onPointerDown={(event) => event.preventDefault()}
                onClick={() => applyFontSize(fontSize - 1)}><Minus className="size-4" /></Button>
              <span className="min-w-10 text-center text-xs tabular-nums" data-testid="live-terminal-font-size">{fontSize}px</span>
              <Button variant="outline" size="icon" className="size-11" aria-label={t("connections.terminal.fontSizeIncrease")}
                disabled={controlsDisabled || fontSize >= SSH_FONT_SIZE_MAX} onPointerDown={(event) => event.preventDefault()}
                onClick={() => applyFontSize(fontSize + 1)}><Plus className="size-4" /></Button>
            </fieldset>
          </div>
        </div>}
        {keyNotSent && <p role="alert" className="px-2 text-xs text-destructive">{t("connections.terminal.keyNotSent")}</p>}
      </div>
      </>}
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
        <ConnectionFailureOverlay failure={effectiveFailure} onRetry={readOnly ? undefined : onReconnect} />
      )}
    </div>
  );
};

export default LiveTerminal;
