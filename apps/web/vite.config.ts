import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import fs from "fs";
import path from "path";
import { visualizer } from "rollup-plugin-visualizer";
import checker from "vite-plugin-checker";

const BACKEND_TARGET = process.env.PIPING_DEV_BACKEND_URL ?? "http://127.0.0.1:7000";
const appVersion = process.env.VITE_APP_VERSION?.trim() ?? "";
const analyzeBundle = process.env.PIPING_BUNDLE_ANALYZE === "1" || process.env.ANALYZE === "1";

// Demo mode (`--mode demo`): the SAME single-page app, but it boots against MSW
// mocks (no backend) from its own html entry (`index.demo.html` →
// `src/demo-main.tsx`) under a `/demo-app/` base so the public portal can embed
// the static build at that path. Demo mode is strictly additive — none of this
// affects the normal `pnpm build` / `pnpm dev` paths.

// In dev, Vite serves the root `index.html`. Demo dev (`dev:demo`) should serve
// the demo entry instead, so it boots the MSW + demo shell.
function serveDemoEntry(): Plugin {
  return {
    name: "piping-demo-entry",
    apply: "serve",
    // `order: "pre"` so this runs BEFORE other plugins' HTML transforms — in
    // particular @vitejs/plugin-react, which injects the Fast Refresh
    // preamble. Returning the demo HTML here lets that preamble still be added
    // afterwards; replacing the HTML in a later/default-order hook would wipe
    // the preamble and React would never mount.
    // Running `order: "pre"` lets Vite's own base-prefixing run AFTER us, so the
    // root-absolute `/src/demo-main.tsx` in the entry HTML is rewritten to
    // `${base}src/demo-main.tsx` automatically — no manual rewrite needed (a
    // manual one would double the base, e.g. `/demo-app/demo-app/src/…`).
    transformIndexHtml: {
      order: "pre",
      handler: () => fs.readFileSync(path.resolve(__dirname, "index.demo.html"), "utf-8"),
    },
  };
}

export default defineConfig(({ command, mode }) => {
  const isDemo = mode === "demo";

  return {
  base: isDemo ? "/demo-app/" : "/",
  define: {
    "import.meta.env.VITE_PIPING_DEMO": JSON.stringify(isDemo ? "1" : ""),
    __APP_VERSION__: JSON.stringify(appVersion),
  },
  server: {
    host: "127.0.0.1",
    port: 5173,
    hmr: {
      overlay: false,
    },
    // In demo mode MSW intercepts /api/v1 in the browser; no backend proxy.
    proxy: isDemo
      ? undefined
      : {
          "/api/v1": {
            target: BACKEND_TARGET,
            changeOrigin: true,
            ws: true,
          },
        },
  },
  plugins: isDemo
    ? [react(), serveDemoEntry()]
    : [
        react(),
        ...(command === "serve"
          ? [
              checker({
                typescript: { tsconfigPath: "tsconfig.app.json" },
                overlay: { initialIsOpen: false },
              }),
            ]
          : []),
        ...(analyzeBundle
          ? [
              visualizer({
                filename: "dist/bundle-stats.html",
                gzipSize: true,
                brotliSize: true,
                template: "treemap",
              }),
            ]
          : []),
      ],
  build: {
    // noVNC's browser.js uses top-level await (WebCodecs H264 capability check).
    // es2022 is required to emit TLA; all supported browsers ship it.
    target: "es2022",
    outDir: isDemo ? "dist-demo" : "dist",
    rollupOptions: {
      input: isDemo
        ? path.resolve(__dirname, "index.demo.html")
        : path.resolve(__dirname, "index.html"),
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) {
            return undefined;
          }
          if (id.includes("@novnc")) {
            return "vendor-novnc";
          }
          if (id.includes("guacamole-common-js")) {
            return "vendor-guacamole";
          }
          if (id.includes("@xterm")) {
            return "vendor-xterm";
          }
          if (id.includes("maplibre-gl") || id.includes("pmtiles")) {
            return "vendor-maplibre";
          }
          // Isolate large UI/state vendors that were landing in the main chunk.
          // react-dom must stay in the main chunk (required for hydration),
          // but tanstack query, radix, and lucide can be deferred.
          if (id.includes("@tanstack")) {
            return "vendor-tanstack";
          }
          if (id.includes("@radix-ui")) {
            return "vendor-radix";
          }
          if (id.includes("lucide-react")) {
            return "vendor-lucide";
          }
          if (
            id.includes("swagger-ui-react") ||
            id.includes("swagger-ui") ||
            id.includes("swagger-client")
          ) {
            return "vendor-swagger";
          }
          return undefined;
        },
      },
    },
    // Swagger UI, MapLibre, and the connection clients are intentionally split
    // into lazy vendor chunks. Keep the warning budget above those known assets
    // so new unexpected chunk growth still stands out in build logs.
    chunkSizeWarningLimit: 1400,
  },
  optimizeDeps: {
    // noVNC is CommonJS; pre-bundle it so Vite can serve it as ESM in dev
    // and correctly include it in the production build. In demo mode the VNC
    // viewer is lazy (off the default route) and never imported on load, so
    // skip pre-bundling its top-level-await module.
    include: isDemo ? [] : ["@novnc/novnc"],
    exclude: isDemo ? ["@novnc/novnc"] : [],
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
    dedupe: [
      "react",
      "react-dom",
      "react/jsx-runtime",
      "react/jsx-dev-runtime",
      "@tanstack/react-query",
      "@tanstack/query-core",
    ],
  },
  };
});
