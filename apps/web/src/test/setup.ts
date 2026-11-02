import "@testing-library/jest-dom";
import { act, cleanup, configure } from "@testing-library/react";
import { afterEach, beforeAll, vi } from "vitest";

afterEach(async () => {
  await act(async () => {
    cleanup();
    // Radix schedules unmount autofocus on a zero-delay timer. Finish that turn
    // while its container and global Event constructors still share a realm.
    if (vi.isFakeTimers()) await vi.runOnlyPendingTimersAsync();
    else await new Promise<void>(resolve => setTimeout(resolve, 0));
  });
});

// Production loads exactly ONE translation dictionary, lazily, so the entry
// chunk does not carry ~144 KB of translations (see `src/shared/i18n/index.ts`).
// Tests render synchronously and a few switch language mid-test, so make both
// dictionaries present before any test body runs.
//
// This has to be a `beforeAll` with a dynamic import, not a top-level static
// import: importing `@/shared/i18n` (and through it `@/shared/settings`) at
// setup time would instantiate the real settings modules before a test file's
// `vi.mock("./api")` is registered, and those mocks would then never apply.
// `beforeAll` runs after the test file's own graph is loaded, so mocks win.
beforeAll(async () => {
  const { ensureLanguageBundle, initI18n } = await import("@/shared/i18n");
  await initI18n();
  await Promise.all([ensureLanguageBundle("en"), ensureLanguageBundle("ru")]);
});

// The default 1s findBy*/waitFor timeout flakes on the shared CI runner once
// v8 coverage instrumentation slows renders down (seen first on the
// PlaybooksPage delete-confirmation alertdialog). Wall-clock cost only applies
// on failure; green tests still resolve as soon as the element appears.
configure({ asyncUtilTimeout: 5000 });

// Node 26 + jsdom 20 ship with `window.localStorage` undefined unless vitest is
// invoked with `--localstorage-file`. Tests that touch localStorage straight on
// `window` (e.g. `fileBrowserState`) crash with "Cannot read properties of
// undefined (reading 'removeItem')". A tiny in-memory shim restores the
// expected Storage surface without needing a per-suite override.
if (!window.localStorage) {
  const store = new Map<string, string>();
  const storage: Storage = {
    get length() {
      return store.size;
    },
    clear: () => store.clear(),
    getItem: (key) => (store.has(key) ? (store.get(key) as string) : null),
    key: (index) => Array.from(store.keys())[index] ?? null,
    removeItem: (key) => {
      store.delete(key);
    },
    setItem: (key, value) => {
      store.set(key, String(value));
    },
  };
  Object.defineProperty(window, "localStorage", { writable: true, value: storage });
  Object.defineProperty(globalThis, "localStorage", { writable: true, value: storage });
}

Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => {},
  }),
});

Object.defineProperty(HTMLCanvasElement.prototype, "getContext", {
  writable: true,
  value: () => null,
});

if (!window.ResizeObserver) {
  const ResizeObserverMock = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };

  Object.defineProperty(window, "ResizeObserver", {
    writable: true,
    value: ResizeObserverMock,
  });
  Object.defineProperty(globalThis, "ResizeObserver", {
    writable: true,
    value: ResizeObserverMock,
  });
}

// jsdom lacks these DOM methods that Radix primitives (Select, etc.) rely on to
// open and position floating content. No-op shims keep them testable.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}
if (!Element.prototype.hasPointerCapture) {
  Element.prototype.hasPointerCapture = () => false;
}
if (!Element.prototype.releasePointerCapture) {
  Element.prototype.releasePointerCapture = () => {};
}
