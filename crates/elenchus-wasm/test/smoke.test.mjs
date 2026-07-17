// npm-level tests for the assembled package (run after `node scripts/build-npm.mjs`).
// These exercise the published Node surface — the wasm `check`, the fs-backed
// helpers, and the IMPORT resolver bridged to Node `fs` — which the crate's Rust
// unit tests cannot reach (they have no JS host).

import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
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

test("skill: the CLI/MCP 'Run it' transport+version appendix is stripped for wasm", () => {
  const s = e.skill();
  assert.doesNotMatch(s, /## Run it/);
  assert.doesNotMatch(s, /Step 0c/);
  assert.doesNotMatch(s, /pick your transport/);
  assert.doesNotMatch(s, /wasm-strip:(begin|end)/);
  // The DSL how-to a wasm consumer needs still ships…
  assert.match(s, /Reading the report/);
  // …and the shipped SKILL.md file matches skill() (both stripped identically).
  const shipped = readFileSync(join(here, "..", "pkg", "SKILL.md"), "utf8");
  assert.doesNotMatch(shipped, /## Run it/);
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

test("maxConflicts: abort is an error string, omitted stays a verdict", () => {
  // A tiny pigeonhole — UNSAT that needs real search.
  let php = "DOMAIN php\n";
  for (let i = 0; i < 3; i++)
    php += `PREMISE pigeon${i}:\n    ATLEAST\n        p${i} in h0\n        p${i} in h1\n`;
  for (let j = 0; j < 2; j++)
    php += `PREMISE hole${j}:\n    EXCLUSIVE\n        p0 in h${j}\n        p1 in h${j}\n        p2 in h${j}\n`;
  php += "CHECK p0 BIDIRECTIONAL\n";
  const aborted = e.check(php, "json", 0, 0, undefined, undefined, undefined, 0);
  assert.match(aborted, /conflict budget exceeded/);
  assert.doesNotMatch(aborted, /"exit_code"/);
  assert.equal(
    e.check(php, "json", 0, 0, undefined, undefined, undefined, 1000000),
    e.check(php)
  );
});
