(() => {
  if (window.__PIPING_DESKTOP_MENU__) return;

  let active = null;

  function close({ restoreFocus = false } = {}) {
    if (!active) return;
    const { panel, trigger, listeners, observer, title } = active;
    active = null;
    listeners.abort();
    observer.disconnect();
    panel.remove();
    trigger.setAttribute("aria-expanded", "false");
    trigger.setAttribute("data-open", "false");
    trigger.removeAttribute("aria-controls");
    if (title === null) trigger.removeAttribute("title");
    else trigger.setAttribute("title", title);
    if (restoreFocus && trigger.isConnected) trigger.focus({ preventScroll: true });
  }

  function isOpen(trigger) {
    return Boolean(active && active.trigger === trigger && active.panel.isConnected);
  }

  function item(label, icon, action) {
    const button = document.createElement("button");
    button.type = "button";
    button.setAttribute("role", "menuitem");
    button.className = "piping-desktop-menu-item";
    const symbol = document.createElement("span");
    symbol.className = "piping-desktop-menu-icon";
    symbol.setAttribute("aria-hidden", "true");
    symbol.textContent = icon;
    const text = document.createElement("span");
    text.textContent = label;
    button.append(symbol, text);
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      close({ restoreFocus: true });
      action();
    });
    return button;
  }

  function installStyle() {
    if (document.getElementById("piping-desktop-menu-style")) return;
    const style = document.createElement("style");
    style.id = "piping-desktop-menu-style";
    style.textContent = `
      .piping-desktop-menu-panel {
        position: fixed; z-index: 2147483000; box-sizing: border-box;
        width: 216px; max-width: calc(100vw - 12px); overflow-y: auto;
        padding: 5px; border: 1px solid rgba(148,163,184,.24); border-radius: 8px;
        background: #15181e; color: #e5e7eb; box-shadow: 0 14px 34px rgba(0,0,0,.45);
        font-family: Inter,ui-sans-serif,system-ui,-apple-system,'Segoe UI',sans-serif;
      }
      .piping-desktop-menu-item {
        display: flex; width: 100%; align-items: center; gap: 8px;
        border: 0; border-radius: 6px; background: transparent; color: inherit;
        font: inherit; font-size: 12px; padding: 7px 9px; text-align: left; cursor: pointer;
      }
      .piping-desktop-menu-item:hover { background: rgba(148,163,184,.14); }
      .piping-desktop-menu-item:focus-visible,
      .piping-desktop-menu-language button:focus-visible {
        outline: 2px solid #78b4f0; outline-offset: -2px;
      }
      .piping-desktop-menu-icon { width: 14px; display: inline-flex; justify-content: center; opacity: .75; }
      .piping-desktop-menu-divider { height: 1px; margin: 4px 2px; background: rgba(148,163,184,.18); }
      .piping-desktop-menu-language { display: flex; gap: 4px; padding: 4px 2px 2px; }
      .piping-desktop-menu-language button {
        flex: 1; border: 0; border-radius: 5px; padding: 5px 6px;
        font: inherit; font-size: 11px; cursor: pointer; background: transparent; color: inherit;
      }
      .piping-desktop-menu-language button[aria-checked=true] { background: rgba(47,125,225,.28); }
    `;
    (document.head || document.documentElement).append(style);
  }

  function toggle(trigger, { items, language, onLanguageChange }) {
    if (isOpen(trigger)) {
      close({ restoreFocus: true });
      return;
    }
    close();
    installStyle();
    const panel = document.createElement("div");
    panel.id = "piping-desktop-menu-panel";
    panel.className = "piping-desktop-menu-panel";
    panel.setAttribute("role", "menu");
    panel.setAttribute("aria-label", trigger.getAttribute("aria-label") || trigger.title);
    const divider = () => {
      const node = document.createElement("div");
      node.className = "piping-desktop-menu-divider";
      node.setAttribute("role", "separator");
      panel.append(node);
    };
    for (const entry of items) {
      if (entry === null) divider();
      else panel.append(item(entry.label, entry.icon, entry.action));
    }
    divider();
    const languages = document.createElement("div");
    languages.className = "piping-desktop-menu-language";
    for (const [code, label] of [["ru", "Русский"], ["en", "English"]]) {
      const button = document.createElement("button");
      button.type = "button";
      button.setAttribute("role", "menuitemradio");
      button.setAttribute("aria-checked", String(language === code));
      button.textContent = label;
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        close({ restoreFocus: true });
        onLanguageChange(code);
      });
      languages.append(button);
    }
    panel.append(languages);
    const rect = trigger.getBoundingClientRect();
    const top = rect.bottom + 4;
    panel.style.left = `${Math.max(6, Math.min(rect.left, window.innerWidth - 222))}px`;
    panel.style.top = `${top}px`;
    panel.style.maxHeight = `${Math.max(0, window.innerHeight - top - 6)}px`;
    const listeners = new AbortController();
    const observer = new MutationObserver(() => {
      if (!trigger.isConnected || !panel.isConnected) close();
    });
    active = { panel, trigger, listeners, observer, title: trigger.getAttribute("title") };
    trigger.removeAttribute("title");
    trigger.setAttribute("aria-expanded", "true");
    trigger.setAttribute("data-open", "true");
    trigger.setAttribute("aria-controls", panel.id);
    document.body.append(panel);
    observer.observe(document.body, { childList: true, subtree: true });
    const outside = (event) => {
      if (!panel.contains(event.target) && !trigger.contains(event.target)) close();
    };
    const capture = { capture: true, signal: listeners.signal };
    window.addEventListener("pointerdown", outside, capture);
    document.addEventListener("focusin", outside, capture);
    window.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        close({ restoreFocus: true });
      } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        event.preventDefault();
        const choices = [...panel.querySelectorAll('[role="menuitem"], [role="menuitemradio"]')];
        const current = choices.indexOf(document.activeElement);
        const index = event.key === "Home" ? 0 : event.key === "End" ? choices.length - 1
          : (current + (event.key === "ArrowDown" ? 1 : -1) + choices.length) % choices.length;
        choices[index]?.focus({ preventScroll: true });
      }
    }, capture);
    window.addEventListener("blur", () => close(), { signal: listeners.signal });
    window.addEventListener("pagehide", () => close(), { signal: listeners.signal });
    panel.querySelector('[role="menuitem"]')?.focus({ preventScroll: true });
  }

  Object.defineProperty(window, "__PIPING_DESKTOP_MENU__", {
    value: Object.freeze({ toggle, close, isOpen }),
    configurable: false,
    writable: false,
  });
})();
