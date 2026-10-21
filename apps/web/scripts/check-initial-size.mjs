#!/usr/bin/env node
/**
 * Initial-payload size gate.
 *
 * `size-limit` in package.json measures individual chunk FILES by glob. That is
 * blind to what the browser actually downloads before it can render: the entry
 * script plus every `<link rel="modulepreload">` Vite injects into
 * `dist/index.html` for the entry's static import graph. Those preloads are
 * fetched immediately and in parallel with the entry, so they are part of the
 * initial payload even though no single one of them trips a per-file limit.
 *
 * This script reconstructs that set from the built HTML and gzips it, so the
 * number it prints is the number a cold visitor pays. Stylesheets are NOT
 * counted (the budget below is JS-only, matching the size-limit convention);
 * they are render-blocking too and remain ungated — see the audit notes.
 *
 * Usage: node scripts/check-initial-size.mjs [--html dist/index.html] [--json]
 */
import { gzipSync } from "node:zlib";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { argv, exit } from "node:process";

/**
 * Budget in gzipped bytes for the whole initial JS payload.
 *
 * Derived (2026-07-31): after moving both i18n dictionaries and all of Sentry
 * out of the entry graph into lazy chunks, the measured initial payload is
 * 223.0 KB gz. Budget = 223.0 KB x 1.15 headroom, rounded to a flat 256 KB.
 * The headroom absorbs ordinary feature growth; a jump past it means something
 * large re-entered the static graph of `main.tsx` (a locale dictionary, a
 * vendor SDK, a route component that stopped being `lazy()`), which is exactly
 * the regression this gate exists to catch. Raise it only with a measurement
 * and a reason, never to make a red pipeline green.
 */
const BUDGET_BYTES = 256 * 1024;

// gzip level is pinned: node's default (6) and level 9 differ by a few KB on
// these chunks, so an unpinned level would make the budget irreproducible
// across environments. 9 matches what a CDN with max compression serves.
const GZIP_LEVEL = 9;

const args = argv.slice(2);
const htmlArgIndex = args.indexOf("--html");
const htmlPath = resolve(htmlArgIndex === -1 ? "dist/index.html" : args[htmlArgIndex + 1]);
const asJson = args.includes("--json");

let html;
try {
  html = readFileSync(htmlPath, "utf-8");
} catch {
  console.error(`check-initial-size: cannot read ${htmlPath} — run \`pnpm run build\` first.`);
  exit(2);
}

const distDir = dirname(htmlPath);

/** Collect every attribute value matched by `pattern` (capture group 1). */
function collect(pattern) {
  return [...html.matchAll(pattern)].map((match) => match[1]);
}

const entryScripts = collect(/<script\b[^>]*\bsrc=["']([^"']+)["'][^>]*>/g);
const preloads = collect(
  /<link\b[^>]*\brel=["']modulepreload["'][^>]*\bhref=["']([^"']+)["'][^>]*>/g,
).concat(collect(/<link\b[^>]*\bhref=["']([^"']+)["'][^>]*\brel=["']modulepreload["'][^>]*>/g));

const assets = [...new Set([...entryScripts, ...preloads])].filter((href) =>
  href.endsWith(".js"),
);

if (assets.length === 0) {
  console.error(`check-initial-size: no entry script or modulepreload found in ${htmlPath}.`);
  exit(2);
}

const measured = assets
  .map((href) => {
    // Hrefs are root-absolute (`/assets/…`) or base-prefixed; resolve them
    // against the dist directory the HTML itself lives in.
    const file = resolve(distDir, href.replace(/^.*\/assets\//, "assets/"));
    const raw = readFileSync(file);
    return {
      href,
      bytes: raw.byteLength,
      gzip: gzipSync(raw, { level: GZIP_LEVEL }).byteLength,
    };
  })
  .sort((a, b) => b.gzip - a.gzip);

const totalGzip = measured.reduce((sum, asset) => sum + asset.gzip, 0);
const kb = (bytes) => `${(bytes / 1024).toFixed(1)} KB`;

if (asJson) {
  console.log(JSON.stringify({ budget: BUDGET_BYTES, totalGzip, assets: measured }, null, 2));
} else {
  console.log(`Initial JS payload (${measured.length} chunks, gzip level ${GZIP_LEVEL}):`);
  for (const asset of measured) {
    console.log(`  ${kb(asset.gzip).padStart(9)}  ${asset.href}`);
  }
  console.log(`  ${"—".repeat(9)}`);
  console.log(`  ${kb(totalGzip).padStart(9)}  total   (budget ${kb(BUDGET_BYTES)})`);
}

if (totalGzip > BUDGET_BYTES) {
  console.error(
    `\ncheck-initial-size: FAIL — initial payload ${kb(totalGzip)} exceeds the ${kb(
      BUDGET_BYTES,
    )} budget by ${kb(totalGzip - BUDGET_BYTES)}.\n` +
      "Largest contributors are listed above. Something re-entered the entry's\n" +
      "static import graph — make it `lazy()`/dynamic `import()` rather than\n" +
      "raising the budget.",
  );
  exit(1);
}
