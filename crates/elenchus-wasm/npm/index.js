"use strict";

// Hand-written Node entry layered over the wasm-pack output (`./elenchus.js`).
// It re-exports the engine functions, adds camelCase names, and adds two
// filesystem helpers (checkFile / checkFileWithImports) backed by Node `fs`.
// `scripts/build-npm.mjs` copies this next to `elenchus.js` inside `pkg/`.

const fs = require("node:fs");
const wasm = require("./elenchus.js");

/** Check an inline `.vrf` program (no IMPORT resolution). `values` supplies VAR
 * port values as a `{ [name]: boolean }` record; `data` supplies them from
 * data-file text as a `{ [name]: string }` record (PROVIDE-only `.vrf`). */
function check(program, format, maxClasses, maxPerClass, values, data) {
  return wasm.check(program, format, maxClasses, maxPerClass, values, data);
}

/** Check a `.vrf` program, resolving IMPORTs via a synchronous read callback. */
function checkWithResolver(root, read, format, maxClasses, maxPerClass, values, data) {
  return wasm.check_with_resolver(root, read, format, maxClasses, maxPerClass, values, data);
}

/** Read a single `.vrf` file and check it (no IMPORT resolution). */
function checkFile(file, format, maxClasses, maxPerClass, values, data) {
  return wasm.check(fs.readFileSync(file, "utf8"), format, maxClasses, maxPerClass, values, data);
}

/**
 * Check a `.vrf` file, resolving IMPORTs through the filesystem. The engine
 * normalizes each relative import against the importing file, then asks the
 * resolver to load the resulting path — so a plain `readFileSync` is enough.
 */
function checkFileWithImports(entry, format, maxClasses, maxPerClass, values, data) {
  const read = (path) => fs.readFileSync(path, "utf8");
  return wasm.check_with_resolver(entry, read, format, maxClasses, maxPerClass, values, data);
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
