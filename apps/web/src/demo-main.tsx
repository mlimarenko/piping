/**
 * Demo-mode entry point.
 *
 * Identical to `main.tsx` except it (1) starts the MSW service worker so every
 * `/api/v1/*` request is answered from `src/demo/handlers.ts` (no backend), and
 * (2) seeds a demo-only initial UI state so the connections tree opens with one
 * site already expanded. None of this runs in the normal app — only the
 * `index.demo.html` entry imports this module, and it is only emitted under
 * `vite build --mode demo`.
 */
import { createRoot } from "react-dom/client";
import App from "./app/App.tsx";
import { worker } from "./demo/browser";
import { demoLanguageFromLocation } from "./demo/language";
import { installDemoSshSocket } from "./demo/ssh-socket-shim";
import { changeAppLanguage, initI18n, preloadFallbackLanguage } from "./shared/i18n/index";
import "./index.css";

/**
 * Demo-only: seed the legacy tree collapse key so exactly ONE site branch is
 * expanded on first load. Runtime persistence now uses user preferences, but
 * TreeView still reads this localStorage key as a first-load fallback for demo
 * and existing browsers. In auto mode hosts group by site -> tenant -> role, so
 * collapsing the second site ("Frankfurt DC") leaves "Amsterdam Edge" open.
 */
function seedDemoTreeState(): void {
  try {
    const KEY = "piping.connections.tree.collapsed";
    if (window.localStorage.getItem(KEY) === null) {
      window.localStorage.setItem(KEY, JSON.stringify(["site=Frankfurt DC|"]));
    }
  } catch {
    // Storage may be unavailable (privacy mode); the tree still renders, just
    // with every site expanded — harmless for the demo.
  }
}

async function bootstrap(): Promise<void> {
  seedDemoTreeState();
  // Demo-only: route the SSH relay WebSocket to a fake that streams a canned
  // shell transcript into xterm (MSW's browser WS interception isn't reliable
  // here). Every non-demo WebSocket URL still uses the native socket.
  installDemoSshSocket();
  // `/demo-app/` base: the worker script and its scope live under the embed path.
  await worker.start({
    serviceWorker: { url: `${import.meta.env.BASE_URL}mockServiceWorker.js` },
    onUnhandledRequest: "bypass",
    quiet: true,
  });
  // The active dictionary is a lazy chunk; mounting first would flash raw keys.
  await initI18n();
  // The embedding portal names the demo language in `?lang=`; that query is the
  // only demo language source (the sites share no storage). Apply it after the
  // native i18n init and before the first render so the embedded surface paints
  // in the requested language instead of the detector's. The mocked settings API
  // reports the same value, so the later preference load cannot override it.
  const demoLanguage = demoLanguageFromLocation();
  if (demoLanguage) {
    await changeAppLanguage(demoLanguage);
  }
  createRoot(document.getElementById("root")!).render(<App />);
  preloadFallbackLanguage();
}

void bootstrap();
