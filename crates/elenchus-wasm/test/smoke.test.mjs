// npm-level tests for the assembled package (run after `node scripts/build-npm.mjs`).
// These exercise the published Node surface — the wasm `check`, the fs-backed
// helpers, and the IMPORT resolver bridged to Node `fs` — which the crate's Rust
// unit tests cannot reach (they have no JS host).

import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
// The assembled package (built into ../pkg by scripts/build-npm.mjs).
const e = require(join(here, "..", "pkg"));
const fx = (name) => join(here, "fixtures", name);

test("check: inline CONFLICT as JSON", () => {
  const out = e.check("DOMAIN d\nFACT x a\nNOT x a\nCHECK x");
  assert.match(out, /"status":"CONFLICT"/);
  assert.match(out, /"exit_code":2/);
});

test("check: human format differs from JSON", () => {
  const program = "DOMAIN d\nFACT x a\nCHECK x";
  assert.notEqual(e.check(program, "json"), e.check(program, "human"));
});

test("version reports the engine; skill marker is version-shaped", () => {
  assert.match(e.version(), /^elenchus \d+\.\d+\.\d+/);
  // Not asserted equal to the engine version — the release-only CI `skill-check`
  // owns that. The marker and the crate version move at different moments during
  // a release, so they are legitimately out of sync between releases.
  assert.match(e.skillVersion(), /^\d+\.\d+\.\d+/);
});

test("skill/about: skill is the SKILL.md text, about points to it", () => {
  assert.match(e.skill(), /name: elenchus/);
  assert.match(e.about(), /elenchus/);
});

test("checkFile: reads and checks a standalone file", () => {
  assert.match(e.checkFile(fx("consistent.vrf")), /"status":"CONSISTENT"/);
});

test("checkFileWithImports: resolves multi-file IMPORT (conflict)", () => {
  assert.match(e.checkFileWithImports(fx("entry-conflict.vrf")), /"status":"CONFLICT"/);
});

test("checkFileWithImports: resolves multi-file IMPORT (consistent)", () => {
  assert.match(e.checkFileWithImports(fx("entry-ok.vrf")), /"status":"CONSISTENT"/);
});

test("checkFileWithImports: a missing import surfaces as an error, not a crash", () => {
  const out = e.checkFileWithImports(fx("entry-missing.vrf"));
  assert.match(out, /not found/i);
});

test("values: an inline VAR template is driven by a values record", () => {
  // The template's RULE only fires when both ports are true.
  const out = e.checkFile(fx("template.vrf"), "json", 0, 0, {
    feature_flag: true,
    tests_pass: true,
  });
  assert.match(out, /"status":"CONSISTENT"/);
  assert.match(out, /deploy is_auto/);
});

test("dataFiles: a PROVIDE-only file path drives the template (CLI --data parity)", () => {
  const out = e.checkFile(fx("template.vrf"), "json", 0, 0, undefined, undefined, [
    fx("provide.vrf"),
  ]);
  assert.match(out, /"status":"CONSISTENT"/);
  assert.match(out, /deploy is_auto/);
});

test("dataFiles: disagreeing with a values record is a hard PortConflict", () => {
  // provide.vrf sets feature_flag true; values sets it false → conflict (exit 2).
  const out = e.checkFile(fx("template.vrf"), "json", 0, 0, { feature_flag: false }, undefined, [
    fx("provide.vrf"),
  ]);
  assert.match(out, /two different values/);
});
