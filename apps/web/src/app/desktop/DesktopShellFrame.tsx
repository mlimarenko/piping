import { type MouseEvent, type ReactNode, useEffect, useMemo, useState } from "react";
import { Minus, MoreHorizontal, Square, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { changeAppLanguage } from "@/shared/i18n";
import {
  DESKTOP_LANGUAGE_STORAGE_KEY,
  DESKTOP_SHELL_READY_EVENT,
  DESKTOP_SHELL_STORAGE_KEY,
  invokeDesktopCommand,
  isPipingDesktopShell,
} from "@/app/desktop/runtime";
import PipingLogo from "@/shared/ui-shared/PipingLogo";

function hasNativeTitlebar(): boolean {
  if (typeof window === "undefined") return false;
  return (
    window.__PIPING_NATIVE_TITLEBAR__ === true ||
    document.documentElement.dataset.pipingNativeTitlebar === "true"
  );
}

function currentDesktopLanguage(language: string | undefined): "en" | "ru" {
  return language?.toLowerCase().startsWith("ru") ? "ru" : "en";
}

function invokeDesktop(command: string, args?: Record<string, unknown>): void {
  void invokeDesktopCommand(command, args).catch(() => undefined);
}

function startDesktopDrag(): void {
  const currentWindow = window.__TAURI__?.window?.getCurrentWindow?.();
  if (currentWindow?.startDragging) {
    void currentWindow.startDragging().catch(() => {
      invokeDesktop("start_window_drag");
    });
    return;
  }
  invokeDesktop("start_window_drag");
}

interface DesktopShellFrameProps {
  children: ReactNode;
}

const DesktopShellFrame = ({ children }: DesktopShellFrameProps) => {
  const [enabled, setEnabled] = useState(() => isPipingDesktopShell());
  const [nativeTitlebar, setNativeTitlebar] = useState(() => hasNativeTitlebar());
  const { i18n, t } = useTranslation();
  const language = useMemo(
    () => currentDesktopLanguage(i18n.resolvedLanguage ?? i18n.language),
    [i18n.language, i18n.resolvedLanguage],
  );

  useEffect(() => {
    let disposed = false;
    const probeTimers: number[] = [];

    const applyDesktopState = () => {
      if (disposed) return;
      const active = isPipingDesktopShell();
      setEnabled(active);
      setNativeTitlebar(hasNativeTitlebar());
      document.documentElement.dataset.pipingDesktopShell = active ? "true" : "false";

      if (!active) {
        delete window.__setDesktopLanguage;
        return;
      }

      try {
        window.localStorage.setItem(DESKTOP_SHELL_STORAGE_KEY, "1");
      } catch {
        // Local storage is optional in hardened browser settings.
      }

      window.__setDesktopLanguage = (nextLanguage: string) => {
        if (nextLanguage !== "en" && nextLanguage !== "ru") return;
        try {
          window.localStorage.setItem(DESKTOP_LANGUAGE_STORAGE_KEY, nextLanguage);
        } catch {
          // Local storage is optional in hardened browser settings.
        }
        void changeAppLanguage(nextLanguage);
      };
    };

    applyDesktopState();
    window.addEventListener(DESKTOP_SHELL_READY_EVENT, applyDesktopState);
    for (const delay of [0, 50, 250, 1000]) {
      probeTimers.push(window.setTimeout(applyDesktopState, delay));
    }

    return () => {
      disposed = true;
      window.removeEventListener(DESKTOP_SHELL_READY_EVENT, applyDesktopState);
      for (const timer of probeTimers) {
        window.clearTimeout(timer);
      }
      delete window.__setDesktopLanguage;
      delete document.documentElement.dataset.pipingDesktopShell;
    };
  }, [i18n]);

  if (!enabled || nativeTitlebar) {
    return <>{children}</>;
  }

  const startDrag = (event: MouseEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    startDesktopDrag();
  };

  const toggleMaximize = () => {
    invokeDesktop("toggle_maximize_window");
  };

  return (
    <div className="desktop-shell-frame">
      <header className="desktop-shell-titlebar">
        <button
          type="button"
          className="desktop-shell-brand"
          aria-label={t("desktopShell.menu", { defaultValue: "Piping menu" })}
          onClick={() => invokeDesktop("show_app_menu", { language })}
        >
          <PipingLogo size={16} />
          <span className="desktop-shell-brand-name">Piping</span>
          <span className="desktop-shell-brand-context">Desktop</span>
          <MoreHorizontal className="desktop-shell-menu-icon" aria-hidden="true" />
        </button>

        <div
          className="desktop-shell-drag-region"
          data-tauri-drag-region="deep"
          onMouseDown={startDrag}
          onDoubleClick={toggleMaximize}
        />

        <div className="desktop-shell-window-controls">
          <button
            type="button"
            className="desktop-shell-window-button"
            aria-label={t("desktopShell.minimize", { defaultValue: "Minimize" })}
            onClick={() => invokeDesktop("minimize_window")}
          >
            <Minus className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
          <button
            type="button"
            className="desktop-shell-window-button"
            aria-label={t("desktopShell.maximize", { defaultValue: "Maximize" })}
            onClick={toggleMaximize}
          >
            <Square className="h-3 w-3" aria-hidden="true" />
          </button>
          <button
            type="button"
            className="desktop-shell-window-button desktop-shell-window-button-danger"
            aria-label={t("desktopShell.close", { defaultValue: "Close" })}
            onClick={() => invokeDesktop("close_window")}
          >
            <X className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </div>
      </header>

      <div className="desktop-shell-content">{children}</div>
    </div>
  );
};

export default DesktopShellFrame;
