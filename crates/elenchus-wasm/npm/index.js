"use strict";

// Hand-written Node entry layered over the wasm-pack output (`./elenchus.js`).
// It re-exports the engine functions, adds camelCase names, and adds two
// filesystem helpers (checkFile / checkFileWithImports) backed by Node `fs`.
// `scripts/build-npm.mjs` copies this next to `elenchus.js` inside `pkg/`.

const fs = require("node:fs");
const wasm = require("./elenchus.js");

/** Read each path in `dataFiles` and fold it into the `data` record, keyed by the
 * path (its origin label). Each file is a PROVIDE-only `.vrf`, parsed exactly like
 * a CLI `--data` file. This is the Node counterpart of CLI `--data <file>`: the
 * wasm core has no filesystem, so the wrapper reads the bytes here. Returns the
 * merged record (or the original `data` when there are no `dataFiles`). */
function mergeDataFiles(data, dataFiles) {
  if (!dataFiles || dataFiles.length === 0) return data;
  const merged = { ...(data || {}) };
  for (const path of dataFiles) {
    merged[path] = fs.readFileSync(path, "utf8");
  }
  return merged;
}

/** Check an inline `.vrf` program (no IMPORT resolution). `values` supplies VAR
 * port values as a `{ [name]: boolean }` record; `data` supplies them from
 * data-file text as a `{ [name]: string }` record (PROVIDE-only `.vrf`);
 * `dataFiles` supplies the same from PROVIDE-only files read off disk. */
function check(program, format, maxClasses, maxPerClass, values, data, dataFiles) {
  return wasm.check(program, format, maxClasses, maxPerClass, values, mergeDataFiles(data, dataFiles));
}

/** Check a `.vrf` program, resolving IMPORTs via a synchronous read callback. */
function checkWithResolver(root, read, format, maxClasses, maxPerClass, values, data, dataFiles) {
  return wasm.check_with_resolver(root, read, format, maxClasses, maxPerClass, values, mergeDataFiles(data, dataFiles));
}

/** Read a single `.vrf` file and check it (no IMPORT resolution). */
function checkFile(file, format, maxClasses, maxPerClass, values, data, dataFiles) {
  return wasm.check(fs.readFileSync(file, "utf8"), format, maxClasses, maxPerClass, values, mergeDataFiles(data, dataFiles));
}

/**
 * Check a `.vrf` file, resolving IMPORTs through the filesystem. The engine
 * normalizes each relative import against the importing file, then asks the
 * resolver to load the resulting path — so a plain `readFileSync` is enough.
 */
function checkFileWithImports(entry, format, maxClasses, maxPerClass, values, data, dataFiles) {
  const read = (path) => fs.readFileSync(path, "utf8");
  return wasm.check_with_resolver(entry, read, format, maxClasses, maxPerClass, values, mergeDataFiles(data, dataFiles));
}

module.exports = {
  check,
  checkWithResolver,
  checkFile,
  checkFileWithImports,
  version: wasm.version,
  about: wasm.about,
  skill: wasm.skill,
  skillVersion: wasm.skill_version,
};
