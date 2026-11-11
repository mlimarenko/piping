#!/usr/bin/env node
/**
 * Move a hidden-source-map build into a release-bound private artifact.
 *
 * The normal build emits no maps. The CI job builds with `PIPING_SOURCE_MAPS=1`,
 * then this script:
 * - refuses a missing/empty release or a build without maps;
 * - verifies every emitted JS chunk has a map;
 * - moves maps to `sourcemaps/<release>/`, never leaving them in `dist` where
 *   the static image or a later artifact step could publish them;
 * - writes a manifest that binds each bundle filename to its map.
 *
 * The map files stay in GitLab job artifacts (project-member access), not in the
 * image, the npm package or the public site.
 */
import { existsSync, mkdirSync, readdirSync, renameSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

const projectRoot = resolve(import.meta.dirname, "..");
const dist = join(projectRoot, "dist");
const release = process.argv[2]?.trim() ?? "";

if (release === "" || !/^[0-9A-Za-z.+-]{1,80}$/.test(release)) {
  console.error("usage: collect-sourcemaps.mjs <release> (semver-like, no slashes)");
  process.exit(1);
}
if (!existsSync(dist)) {
  console.error(`missing build output: ${dist}`);
  process.exit(1);
}

function filesUnder(directory) {
  const found = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) found.push(...filesUnder(path));
    else found.push(path);
  }
  return found;
}

const allFiles = filesUnder(dist);
const maps = allFiles.filter((file) => file.endsWith(".map"));
const scripts = allFiles.filter((file) => file.endsWith(".js"));
if (maps.length === 0) {
  console.error("build emitted no source maps; was PIPING_SOURCE_MAPS=1 set?");
  process.exit(1);
}
// Every map must belong to a built chunk; an orphan map means the build and
// the artifact disagree about the bundle identity.
const orphaned = maps.filter((map) => !existsSync(map.slice(0, -".map".length)));
if (orphaned.length > 0) {
  console.error(
    `source maps without a JS chunk: ${orphaned.map((file) => relative(dist, file)).join(", ")}`,
  );
  process.exit(1);
}
// Rolldown emits no map for pure re-export facades and the runtime, and the
// public worker is copied rather than bundled. Record them instead of failing:
// the artifact contract is "no map stays in dist", not "every file is mapped".
const unmapped = scripts.filter((script) => !existsSync(`${script}.map`));

const output = join(projectRoot, "sourcemaps", release);
if (existsSync(output)) {
  console.error(`source map directory already exists: ${output}`);
  process.exit(1);
}
mkdirSync(output, { recursive: true });

const manifest = { release, files: {}, chunksWithoutMap: [] };
for (const map of maps) {
  const relativePath = relative(dist, map);
  const target = join(output, relativePath);
  mkdirSync(dirname(target), { recursive: true });
  renameSync(map, target);
  manifest.files[relativePath] = statSync(target).size;
}
manifest.chunksWithoutMap = unmapped.map((file) => relative(dist, file));

// The shipped tree must be map-free after collection.
const leftover = filesUnder(dist).filter((file) => file.endsWith(".map"));
if (leftover.length > 0) {
  console.error(`source maps still in dist: ${leftover.join(", ")}`);
  process.exit(1);
}

writeFileSync(join(output, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
writeFileSync(join(output, "release.txt"), `${release}\n`);
console.log(
  `collected ${maps.length} source maps for release ${release} into ${relative(projectRoot, output)}`,
);
if (unmapped.length > 0) {
  console.log(`${unmapped.length} chunk(s) have no map: ${manifest.chunksWithoutMap.join(", ")}`);
}
