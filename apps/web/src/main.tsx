import { createRoot } from "react-dom/client";
import App from "./app/App.tsx";
import { initObservability, loadObservability } from "./shared/lib/observability";
import { initI18n, preloadFallbackLanguage } from "./shared/i18n/index";
import "./index.css";

// Installs buffering error listeners synchronously; the Sentry SDK itself is a
// lazy chunk fetched on idle, after the first render.
initObservability();

// The active translation dictionary is a lazy chunk. Mounting before it lands
// would paint raw translation keys, so the first render waits for it — then the
// other dictionary warms in the background as the `fallbackLng` source.
await initI18n();
createRoot(document.getElementById("root")!).render(<App />);
// Both lazy fetches are scheduled only after the first render is queued.
preloadFallbackLanguage();
loadObservability();
