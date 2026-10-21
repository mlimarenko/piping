(() => {
  const HEIGHT = 28;
  const BAR_ID = "piping-desktop-native-titlebar";
  const RESIZE_HANDLES_ID = "piping-desktop-native-resize-handles";
  const STYLE_ID = "piping-desktop-native-titlebar-style";
  const READY_EVENT = "piping:desktop-shell-ready";
  const LANGUAGE_STORAGE_KEY = "piping.desktop.language";
  const SHELL_STORAGE_KEY = "piping.desktop.shell";

  const labels = {
    en: {
      menu: "Piping menu",
      desktop: "Desktop",
      minimize: "Minimize",
      maximize: "Maximize",
      close: "Close",
    },
    ru: {
      menu: "Меню Piping",
      desktop: "Desktop",
      minimize: "Свернуть",
      maximize: "Развернуть",
      close: "Закрыть",
    },
  };

  const logoSvg = `
    <svg class="piping-desktop-native-titlebar-logo" width="17" height="17" viewBox="0 0 32 32" fill="none" aria-hidden="true">
      <defs>
        <linearGradient id="piping-desktop-native-titlebar-logo-gradient" x1="0" y1="32" x2="32" y2="0" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stop-color="#ffc94d" />
          <stop offset="50%" stop-color="#f9b310" stop-opacity="0.95" />
          <stop offset="100%" stop-color="#e08e00" stop-opacity="0.9" />
        </linearGradient>
        <mask id="piping-desktop-native-titlebar-mask-large">
          <rect width="32" height="32" fill="black" />
          <circle cx="12" cy="20" r="12" fill="white" />
          <circle cx="20" cy="12" r="9" fill="black" />
        </mask>
        <mask id="piping-desktop-native-titlebar-mask-small">
          <rect width="32" height="32" fill="black" />
          <circle cx="20" cy="12" r="9" fill="white" />
          <circle cx="12" cy="20" r="12" fill="black" />
        </mask>
      </defs>
      <circle cx="12" cy="20" r="12" fill="url(#piping-desktop-native-titlebar-logo-gradient)" mask="url(#piping-desktop-native-titlebar-mask-large)" />
      <circle cx="20" cy="12" r="9" fill="url(#piping-desktop-native-titlebar-logo-gradient)" mask="url(#piping-desktop-native-titlebar-mask-small)" />
    </svg>`;

  function invoke(command, args) {
    return window.__TAURI__?.core?.invoke?.(command, args);
  }

  function startWindowDrag() {
    const currentWindow = window.__TAURI__?.window?.getCurrentWindow?.();
    if (currentWindow?.startDragging) {
      return currentWindow.startDragging().catch(() => invoke("start_window_drag")?.catch(() => undefined));
    }
    return invoke("start_window_drag")?.catch(() => undefined);
  }

  function startWindowResize(direction) {
    return window.__TAURI__?.window?.getCurrentWindow?.()?.startResizeDragging?.(direction)?.catch(() => undefined);
  }

  function normalizeLanguage(value) {
    const normalized = String(value || "").toLowerCase().replace("_", "-");
    if (normalized.startsWith("ru")) return "ru";
    if (normalized.startsWith("en")) return "en";
    return null;
  }

  function currentLanguage() {
    try {
      const saved = normalizeLanguage(window.localStorage.getItem(LANGUAGE_STORAGE_KEY));
      if (saved) return saved;
    } catch (_) {}

    const htmlLanguage = normalizeLanguage(document.documentElement.lang);
    if (htmlLanguage) return htmlLanguage;

    const candidates = [...(window.navigator.languages || []), window.navigator.language];
    return candidates.map(normalizeLanguage).find(Boolean) || "en";
  }

  function text(key) {
    const language = currentLanguage();
    return labels[language]?.[key] || labels.en[key] || key;
  }

  function markDesktopShell() {
    try {
      if (window.__PIPING_DESKTOP__ !== true) {
        Object.defineProperty(window, "__PIPING_DESKTOP__", {
          value: true,
          configurable: false,
          enumerable: false,
          writable: false,
        });
      }
    } catch (_) {}

    try {
      if (window.__PIPING_NATIVE_TITLEBAR__ !== true) {
        Object.defineProperty(window, "__PIPING_NATIVE_TITLEBAR__", {
          value: true,
          configurable: false,
          enumerable: false,
          writable: false,
        });
      }
    } catch (_) {}

    try {
      window.localStorage.setItem(SHELL_STORAGE_KEY, "1");
    } catch (_) {}

    if (!window.__setDesktopLanguage || window.__setDesktopLanguage.__pipingNativeTitlebar === true) {
      const setDesktopLanguage = (language) => {
        if (language !== "en" && language !== "ru") return;
        try {
          window.localStorage.setItem(LANGUAGE_STORAGE_KEY, language);
        } catch (_) {}
        document.documentElement.lang = language;
        mountTitlebar();
      };
      setDesktopLanguage.__pipingNativeTitlebar = true;
      window.__setDesktopLanguage = setDesktopLanguage;
    }

    if (!document.documentElement) return;

    document.documentElement.dataset.pipingDesktopShell = "true";
    document.documentElement.dataset.pipingNativeTitlebar = "true";
    window.dispatchEvent(new Event(READY_EVENT));
  }

  function installStyle() {
    if (document.getElementById(STYLE_ID)) return;
    if (!document.head && !document.documentElement) return;

    const style = document.createElement("style");
    style.id = STYLE_ID;
    style.textContent = `
      :root[data-piping-native-titlebar="true"] {
        --piping-desktop-native-titlebar-height: ${HEIGHT}px;
      }

      :root[data-piping-native-titlebar-mode="fixed"] {
        --app-vh: calc(100dvh - var(--piping-desktop-native-titlebar-height)) !important;
      }

      :root[data-piping-native-titlebar-mode="fixed"] body {
        height: 100dvh !important;
        min-height: 100dvh !important;
        margin: 0 !important;
        padding-top: var(--piping-desktop-native-titlebar-height) !important;
        box-sizing: border-box !important;
        overflow: hidden !important;
      }

      :root[data-piping-native-titlebar-mode="fixed"] #root {
        min-height: var(--app-vh);
      }

      :root[data-piping-native-titlebar-mode="embedded"] {
        --app-vh: calc(100dvh - var(--piping-desktop-native-titlebar-height)) !important;
      }

      :root[data-piping-native-titlebar-mode="embedded"] .desktop-shell-frame {
        --desktop-shell-titlebar-height: var(--piping-desktop-native-titlebar-height) !important;
        --app-vh: calc(100dvh - var(--piping-desktop-native-titlebar-height)) !important;
      }

      :root[data-piping-native-titlebar-mode="embedded"] .desktop-shell-titlebar {
        height: var(--piping-desktop-native-titlebar-height) !important;
        min-height: var(--piping-desktop-native-titlebar-height) !important;
        padding: 0 !important;
        overflow: visible !important;
        background: transparent !important;
      }

      :root[data-piping-native-titlebar-mode="embedded"] .desktop-shell-content {
        height: var(--app-vh) !important;
      }

      #${BAR_ID},
      #${BAR_ID} * {
        box-sizing: border-box;
      }

      #${BAR_ID} {
        height: var(--piping-desktop-native-titlebar-height);
        min-height: var(--piping-desktop-native-titlebar-height);
        width: 100%;
        display: grid;
        grid-template-columns: auto minmax(0, 1fr) auto;
        align-items: center;
        overflow: hidden;
        border-bottom: 1px solid rgba(148, 163, 184, 0.22);
        background: linear-gradient(180deg, #1b2027 0%, #151920 100%);
        color: #e5e7eb;
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        line-height: 1;
        user-select: none;
        contain: layout style;
      }

      :root[data-piping-native-titlebar-mode="fixed"] #${BAR_ID} {
        position: fixed;
        top: 0;
        left: 0;
        right: 0;
        z-index: 2147483647;
      }

      :root[data-piping-native-titlebar-mode="embedded"] #${BAR_ID} {
        position: relative;
        z-index: 1;
      }

      #${BAR_ID} .piping-desktop-native-titlebar-brand {
        height: 100%;
        min-width: 0;
        display: inline-flex;
        align-items: center;
      }

      #${BAR_ID} .piping-desktop-native-titlebar-menu {
        width: 30px;
        height: 100%;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        border: 0;
        border-right: 1px solid rgba(148, 163, 184, 0.12);
        border-radius: 0;
        background: transparent;
        color: inherit;
        cursor: pointer;
      }

      #${BAR_ID} .piping-desktop-native-titlebar-menu:hover {
        background: rgba(148, 163, 184, 0.12);
      }

      #${BAR_ID} .piping-desktop-native-titlebar-title {
        height: 100%;
        display: inline-flex;
        align-items: center;
        gap: 7px;
        padding: 0 10px;
        color: #f8fafc;
        font-size: 12px;
        font-weight: 700;
        letter-spacing: 0;
        white-space: nowrap;
      }

      #${BAR_ID} .piping-desktop-native-titlebar-context {
        color: #97a3b5;
        font-size: 11px;
        font-weight: 500;
      }

      #${BAR_ID} .piping-desktop-native-titlebar-drag {
        min-width: 0;
        height: 100%;
        cursor: default;
      }

      #${BAR_ID} .piping-desktop-native-titlebar-controls {
        height: 100%;
        display: flex;
        align-items: center;
        border-left: 1px solid rgba(148, 163, 184, 0.12);
      }

      #${BAR_ID} .piping-desktop-native-titlebar-control {
        width: 36px;
        height: 100%;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        border: 0;
        border-radius: 0;
        background: transparent;
        color: #dbe5f3;
        font-size: 13px;
        line-height: 1;
        cursor: pointer;
      }

      #${BAR_ID} .piping-desktop-native-titlebar-control:hover {
        background: rgba(148, 163, 184, 0.12);
        color: #ffffff;
      }

      #${BAR_ID} .piping-desktop-native-titlebar-control-close:hover {
        background: #ef4444;
        color: #ffffff;
      }

      #${RESIZE_HANDLES_ID} {
        position: static;
        pointer-events: none;
      }

      #${RESIZE_HANDLES_ID} [data-window-resize] {
        position: fixed;
        z-index: 2147483647;
        pointer-events: auto;
        background: transparent;
      }

      #${RESIZE_HANDLES_ID} [data-window-resize="North"] {
        top: 0;
        left: 12px;
        right: 12px;
        height: 6px;
        cursor: ns-resize;
      }

      #${RESIZE_HANDLES_ID} [data-window-resize="South"] {
        bottom: 0;
        left: 12px;
        right: 12px;
        height: 6px;
        cursor: ns-resize;
      }

      #${RESIZE_HANDLES_ID} [data-window-resize="West"] {
        top: 12px;
        bottom: 12px;
        left: 0;
        width: 6px;
        cursor: ew-resize;
      }

      #${RESIZE_HANDLES_ID} [data-window-resize="East"] {
        top: 12px;
        right: 0;
        bottom: 12px;
        width: 6px;
        cursor: ew-resize;
      }

      #${RESIZE_HANDLES_ID} [data-window-resize="NorthWest"],
      #${RESIZE_HANDLES_ID} [data-window-resize="NorthEast"],
      #${RESIZE_HANDLES_ID} [data-window-resize="SouthWest"],
      #${RESIZE_HANDLES_ID} [data-window-resize="SouthEast"] {
        width: 12px;
        height: 12px;
      }

      #${RESIZE_HANDLES_ID} [data-window-resize="NorthWest"] {
        top: 0;
        left: 0;
        cursor: nwse-resize;
      }

      #${RESIZE_HANDLES_ID} [data-window-resize="NorthEast"] {
        top: 0;
        right: 0;
        cursor: nesw-resize;
      }

      #${RESIZE_HANDLES_ID} [data-window-resize="SouthWest"] {
        bottom: 0;
        left: 0;
        cursor: nesw-resize;
      }

      #${RESIZE_HANDLES_ID} [data-window-resize="SouthEast"] {
        right: 0;
        bottom: 0;
        cursor: nwse-resize;
      }
    `;

    (document.head || document.documentElement).append(style);
  }

  function createTitlebar() {
    const bar = document.createElement("div");
    bar.id = BAR_ID;
    bar.innerHTML = `
      <div class="piping-desktop-native-titlebar-brand">
        <button class="piping-desktop-native-titlebar-menu" type="button" aria-haspopup="menu">
          ${logoSvg}
        </button>
        <div class="piping-desktop-native-titlebar-title" data-window-drag data-tauri-drag-region="deep">
          <span>Piping</span>
          <span class="piping-desktop-native-titlebar-context">${text("desktop")}</span>
        </div>
      </div>
      <div class="piping-desktop-native-titlebar-drag" data-window-drag data-tauri-drag-region="deep"></div>
      <div class="piping-desktop-native-titlebar-controls">
        <button class="piping-desktop-native-titlebar-control" type="button" data-window-action="minimize">-</button>
        <button class="piping-desktop-native-titlebar-control" type="button" data-window-action="toggle-maximize">□</button>
        <button class="piping-desktop-native-titlebar-control piping-desktop-native-titlebar-control-close" type="button" data-window-action="close">×</button>
      </div>
    `;

    const menu = bar.querySelector(".piping-desktop-native-titlebar-menu");
    menu?.addEventListener("click", (event) => {
      event.stopPropagation();
      void invoke("show_app_menu", { language: currentLanguage() })?.catch(() => undefined);
    });

    bar.querySelectorAll("[data-window-action]").forEach((button) => {
      button.addEventListener("click", () => {
        const action = button.getAttribute("data-window-action");
        if (action === "minimize") {
          void invoke("minimize_window")?.catch(() => undefined);
        } else if (action === "toggle-maximize") {
          void invoke("toggle_maximize_window")?.catch(() => undefined);
        } else if (action === "close") {
          void invoke("close_window")?.catch(() => undefined);
        }
      });
    });

    bar.querySelectorAll("[data-window-drag]").forEach((region) => {
      region.addEventListener("mousedown", (event) => {
        if (event.button !== 0 || event.detail > 1) return;
        event.preventDefault();
        void startWindowDrag();
      });

      region.addEventListener("dblclick", () => {
        void invoke("toggle_maximize_window")?.catch(() => undefined);
      });
    });

    return bar;
  }

  function ensureResizeHandles() {
    let handles = document.getElementById(RESIZE_HANDLES_ID);
    if (handles) return handles;

    handles = document.createElement("div");
    handles.id = RESIZE_HANDLES_ID;
    handles.setAttribute("aria-hidden", "true");
    handles.innerHTML = [
      "North",
      "South",
      "West",
      "East",
      "NorthWest",
      "NorthEast",
      "SouthWest",
      "SouthEast",
    ]
      .map((direction) => `<div data-window-resize="${direction}"></div>`)
      .join("");

    handles.querySelectorAll("[data-window-resize]").forEach((handle) => {
      handle.addEventListener("mousedown", (event) => {
        if (event.button !== 0) return;
        const direction = handle.getAttribute("data-window-resize");
        if (!direction) return;
        event.preventDefault();
        event.stopPropagation();
        void startWindowResize(direction);
      });
    });

    document.body.append(handles);
    return handles;
  }

  function ensureTitlebar() {
    return document.getElementById(BAR_ID) || createTitlebar();
  }

  function applyLabels(bar) {
    const menu = bar.querySelector(".piping-desktop-native-titlebar-menu");
    menu?.setAttribute("aria-label", text("menu"));
    menu?.setAttribute("title", text("menu"));

    const context = bar.querySelector(".piping-desktop-native-titlebar-context");
    if (context) context.textContent = text("desktop");

    const actions = {
      minimize: text("minimize"),
      "toggle-maximize": text("maximize"),
      close: text("close"),
    };
    bar.querySelectorAll("[data-window-action]").forEach((button) => {
      const label = actions[button.getAttribute("data-window-action") || ""];
      if (!label) return;
      button.setAttribute("aria-label", label);
      button.setAttribute("title", label);
    });
  }

  function embeddedHost() {
    const host = document.querySelector(".desktop-shell-titlebar");
    return host instanceof HTMLElement ? host : null;
  }

  function mountTitlebar() {
    if (!document.documentElement) return;
    markDesktopShell();
    installStyle();
    if (!document.body) return;

    const bar = ensureTitlebar();
    applyLabels(bar);
    ensureResizeHandles();

    const host = embeddedHost();
    if (host) {
      if (bar.parentElement !== host || host.children.length !== 1) {
        host.replaceChildren(bar);
      }
      host.classList.add("piping-desktop-native-titlebar-host");
      document.documentElement.dataset.pipingNativeTitlebarMode = "embedded";
      return;
    }

    if (bar.parentElement !== document.body || document.body.firstElementChild !== bar) {
      document.body.prepend(bar);
    }
    document.documentElement.dataset.pipingNativeTitlebarMode = "fixed";
  }

  markDesktopShell();

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", mountTitlebar, { once: true });
  } else {
    mountTitlebar();
  }

  for (const delay of [0, 50, 250, 1000, 2500]) {
    window.setTimeout(mountTitlebar, delay);
  }

  let mountScheduled = false;
  let observedMutations = 0;
  let observer = null;

  function scheduleMount() {
    if (mountScheduled) return;
    mountScheduled = true;
    window.requestAnimationFrame(() => {
      mountScheduled = false;
      mountTitlebar();
    });
  }

  observer = new MutationObserver(() => {
    observedMutations += 1;
    if (observedMutations > 40) {
      observer?.disconnect();
      return;
    }
    scheduleMount();
  });

  const observe = () => {
    if (!document.documentElement) {
      window.setTimeout(observe, 50);
      return;
    }
    observer.observe(document.documentElement, { childList: true, subtree: true });
    window.setTimeout(() => observer?.disconnect(), 5000);
  };
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", observe, { once: true });
  } else {
    observe();
  }
})();
