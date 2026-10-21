// Static file server for the demo build, used as the Playwright `webServer`.
//
// Why not `vite preview`: the demo build's rollup input is `index.demo.html`, so
// `dist-demo` contains no `index.html` and every SPA fallback implementation
// (preview's included) 404s on deep links like `/connections`. Serving the demo
// entry as the fallback document here keeps `vite.config.ts` untouched — the
// public portal still embeds the very same artifact under `/demo-app/`.
//
// The e2e build is produced with `--base=/` so the suite's absolute
// `page.goto("/connections")` calls resolve without rewriting 20 spec files.
import { createServer } from "node:http";
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "dist-demo");
const ENTRY = path.join(ROOT, "index.demo.html");
const PORT = Number(process.env.PIPING_DEMO_PORT ?? 4173);

const CONTENT_TYPES = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".ico", "image/x-icon"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".pbf", "application/x-protobuf"],
  [".png", "image/png"],
  [".pmtiles", "application/octet-stream"],
  [".svg", "image/svg+xml"],
  [".txt", "text/plain; charset=utf-8"],
  [".woff", "font/woff"],
  [".woff2", "font/woff2"],
  [".yaml", "text/yaml; charset=utf-8"],
]);

/** Resolve a URL path to a file inside ROOT, or null when it escapes/misses. */
async function resolveFile(urlPath) {
  const decoded = decodeURIComponent(urlPath.split("?")[0]);
  const candidate = path.join(ROOT, path.normalize(decoded));
  // path.join collapses `..`; reject anything that still lands outside ROOT.
  if (candidate !== ROOT && !candidate.startsWith(ROOT + path.sep)) {
    return null;
  }
  try {
    const info = await stat(candidate);
    return info.isFile() ? candidate : null;
  } catch {
    return null;
  }
}

const server = createServer((req, res) => {
  void (async () => {
    const url = req.url ?? "/";
    // The SPA fallback must never answer `/api/v1/*`. Only MSW, inside the page,
    // serves the API here — a Node-side `page.request.get("/api/v1/...")` has no
    // service worker in front of it, and answering it with 200 index.html would
    // make `response.ok()` true and turn such a call into a silent fake pass.
    if (url.startsWith("/api/")) {
      res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      res.end("the demo lane has no backend: /api/* is served by MSW in-page only\n");
      return;
    }
    const file = (await resolveFile(url)) ?? ENTRY;
    res.writeHead(200, {
      "content-type": CONTENT_TYPES.get(path.extname(file)) ?? "application/octet-stream",
      // MSW registers its worker at the server root; without this a nested
      // script path would be refused a root scope by the browser.
      "service-worker-allowed": "/",
      "cache-control": "no-store",
    });
    createReadStream(file).pipe(res);
  })();
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`demo build served on http://127.0.0.1:${PORT}`);
});
