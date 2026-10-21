const LANGUAGE_STORAGE_KEY = "piping.desktop.language";

const translations = {
  en: {
    "app.desktop": "Desktop",
    "app.language": "Language",
    "app.menu": "Piping menu",
    "actions.checkUpdates": "Check updates",
    "actions.about": "About",
    "actions.changeAddress": "Change address",
    "window.minimize": "Minimize",
    "window.maximize": "Maximize",
    "window.close": "Close",
    "setup.eyebrow": "Piping Desktop",
    "setup.title": "Connect to your Piping service",
    "setup.lede":
      "Enter the deployed web address once. The desktop app will open that service directly on future launches.",
    "setup.serviceAddress": "Service address",
    "setup.placeholder": "piping.example.com",
    "setup.open": "Open",
    "setup.opening": "Opening...",
    "setup.hint": "Hostnames without a scheme use HTTPS automatically.",
    "status.runtimeUnavailable": "Tauri runtime is unavailable.",
    "status.opening": "Opening service...",
  },
  ru: {
    "app.desktop": "Desktop",
    "app.language": "Язык",
    "app.menu": "Меню Piping",
    "actions.checkUpdates": "Обновления",
    "actions.about": "О приложении",
    "actions.changeAddress": "Адрес сервиса",
    "window.minimize": "Свернуть",
    "window.maximize": "Развернуть",
    "window.close": "Закрыть",
    "setup.eyebrow": "Piping Desktop",
    "setup.title": "Подключение к Piping-сервису",
    "setup.lede":
      "Введите адрес развернутого web-приложения один раз. При следующих запусках desktop откроет этот сервис сразу.",
    "setup.serviceAddress": "Адрес сервиса",
    "setup.placeholder": "piping.example.com",
    "setup.open": "Открыть",
    "setup.opening": "Открываем...",
    "setup.hint": "Если схема не указана, будет автоматически использован HTTPS.",
    "status.runtimeUnavailable": "Среда Tauri недоступна.",
    "status.opening": "Открываем сервис...",
  },
};

const invoke = window.__TAURI__?.core?.invoke;
let currentLanguage = detectNavigatorLanguage();

function startWindowDrag() {
  const currentWindow = window.__TAURI__?.window?.getCurrentWindow?.();
  if (currentWindow?.startDragging) {
    return currentWindow.startDragging().catch(() => invoke?.("start_window_drag").catch(() => {}));
  }
  return invoke?.("start_window_drag").catch(() => {});
}

function startWindowResize(direction) {
  return window.__TAURI__?.window?.getCurrentWindow?.()?.startResizeDragging?.(direction)?.catch(() => {});
}

export function getInvoke() {
  return invoke;
}

export function getLanguage() {
  return currentLanguage;
}

export function getSavedLanguage() {
  const savedLanguage = window.localStorage.getItem(LANGUAGE_STORAGE_KEY);
  if (savedLanguage === "ru" || savedLanguage === "en") {
    return savedLanguage;
  }
  return null;
}

function normalizeLanguage(value) {
  if (!value) {
    return null;
  }
  const normalized = value.toLowerCase().replace("_", "-");
  if (normalized.startsWith("ru")) {
    return "ru";
  }
  if (normalized.startsWith("en")) {
    return "en";
  }
  return null;
}

function detectNavigatorLanguage() {
  const candidates = [...(window.navigator.languages || []), window.navigator.language];
  return candidates.map(normalizeLanguage).find(Boolean) || "en";
}

async function detectInitialLanguage() {
  const savedLanguage = getSavedLanguage();
  if (savedLanguage) {
    return savedLanguage;
  }

  const systemLanguage = invoke ? await invoke("get_system_language").catch(() => null) : null;
  const systemCandidates = String(systemLanguage || "")
    .split(":")
    .filter(Boolean);
  return systemCandidates.map(normalizeLanguage).find(Boolean) || detectNavigatorLanguage();
}

export function t(key) {
  const language = getLanguage();
  return translations[language]?.[key] || translations.en[key] || key;
}

export async function initChrome({ onLanguageChange } = {}) {
  currentLanguage = await detectInitialLanguage();
  applyLanguage(currentLanguage);
  window.__setDesktopLanguage = (language) => {
    if (language === "en" || language === "ru") {
      setLanguage(language, onLanguageChange);
    }
  };
  wireLanguageSelects(onLanguageChange);
  wireLogoMenu();
  wireTopbarActions();
  wireWindowControls();
  wireDragRegions();
  wireResizeHandles();
}

function setLanguage(language, onLanguageChange) {
  currentLanguage = language;
  window.localStorage.setItem(LANGUAGE_STORAGE_KEY, language);
  applyLanguage(language);
  onLanguageChange?.(language);
}

function applyLanguage(language) {
  document.documentElement.lang = language;

  document.querySelectorAll("[data-i18n]").forEach((node) => {
    node.textContent = translations[language]?.[node.dataset.i18n] || translations.en[node.dataset.i18n] || "";
  });

  document.querySelectorAll("[data-i18n-placeholder]").forEach((node) => {
    node.setAttribute(
      "placeholder",
      translations[language]?.[node.dataset.i18nPlaceholder] ||
        translations.en[node.dataset.i18nPlaceholder] ||
        "",
    );
  });

  document.querySelectorAll("[data-i18n-title]").forEach((node) => {
    const value = translations[language]?.[node.dataset.i18nTitle] || translations.en[node.dataset.i18nTitle] || "";
    node.setAttribute("title", value);
    node.setAttribute("aria-label", value);
  });
}

function wireLanguageSelects(onLanguageChange) {
  document.querySelectorAll("[data-language-select]").forEach((select) => {
    select.innerHTML = "";
    [
      ["en", "EN"],
      ["ru", "RU"],
    ].forEach(([value, label]) => {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      select.append(option);
    });
    select.value = getLanguage();
    select.addEventListener("change", () => setLanguage(select.value, onLanguageChange));
  });
}

function wireTopbarActions() {
  document.querySelectorAll("[data-action]").forEach((button) => {
    button.addEventListener("click", async () => {
      if (!invoke) {
        return;
      }

      button.disabled = true;
      try {
        if (button.dataset.action === "check-updates") {
          await invoke("check_for_updates", { language: getLanguage() });
        } else if (button.dataset.action === "about") {
          await invoke("show_about", { language: getLanguage() });
        } else if (button.dataset.action === "change-service") {
          await invoke("change_service_address");
        }
      } finally {
        button.disabled = false;
      }
    });
  });
}

function wireLogoMenu() {
  const trigger = document.querySelector("[data-menu-trigger]");
  if (!trigger) {
    return;
  }
  trigger.setAttribute("aria-expanded", "false");

  trigger.addEventListener("click", async (event) => {
    event.stopPropagation();
    if (!invoke) {
      return;
    }
    await invoke("show_app_menu", { language: getLanguage() }).catch(() => {});
  });
}

function wireWindowControls() {
  document.querySelectorAll("[data-window-action]").forEach((button) => {
    button.addEventListener("click", async () => {
      if (!invoke) {
        return;
      }

      if (button.dataset.windowAction === "minimize") {
        await invoke("minimize_window");
      } else if (button.dataset.windowAction === "toggle-maximize") {
        await invoke("toggle_maximize_window");
      } else if (button.dataset.windowAction === "close") {
        await invoke("close_window");
      }
    });
  });
}

function wireDragRegions() {
  document.querySelectorAll("[data-window-drag]").forEach((region) => {
    region.addEventListener("mousedown", (event) => {
      if (event.button !== 0 || event.detail > 1) {
        return;
      }
      event.preventDefault();
      void startWindowDrag();
    });

    region.addEventListener("dblclick", async () => {
      if (!invoke) {
        return;
      }
      await invoke("toggle_maximize_window").catch(() => {});
    });
  });
}

function wireResizeHandles() {
  document.querySelectorAll("[data-window-resize]").forEach((handle) => {
    handle.addEventListener("mousedown", (event) => {
      if (event.button !== 0) {
        return;
      }
      const direction = handle.dataset.windowResize;
      if (!direction) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      void startWindowResize(direction);
    });
  });
}
