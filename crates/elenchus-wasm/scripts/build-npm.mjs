#!/usr/bin/env node
// Assemble the publishable npm package from the wasm-pack output:
//   1. wasm-pack build (nodejs target) -> pkg/           (skip with --no-build)
//   2. drop in the hand-written Node entry (index.js/.d.ts) and the skill
//   3. rewrite pkg/package.json (scoped name, entry points, file list)
//
// Run from the crate dir:  node scripts/build-npm.mjs
// Override the published name with NPM_PACKAGE_NAME (default "@m62624/elenchus").

import { execFileSync } from "node:child_process";
import { copyFileSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const crateDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = join(crateDir, "..", "..");
const pkg = join(crateDir, "pkg");

if (!process.argv.includes("--no-build")) {
  execFileSync(
    "wasm-pack",
    ["build", crateDir, "--target", "nodejs", "--out-dir", "pkg", "--out-name", "elenchus"],
    { stdio: "inherit" },
  );
}

// Node entry + companion skill, copied next to the wasm-pack artifacts.
copyFileSync(join(crateDir, "npm", "index.js"), join(pkg, "index.js"));
copyFileSync(join(crateDir, "npm", "index.d.ts"), join(pkg, "index.d.ts"));
copyFileSync(join(repoRoot, "skill", "SKILL.md"), join(pkg, "SKILL.md"));
copyFileSync(join(crateDir, "README.md"), join(pkg, "README.md"));

// Point the package at the curated Node entry and list everything we ship.
const pkgJsonPath = join(pkg, "package.json");
const pkgJson = JSON.parse(readFileSync(pkgJsonPath, "utf8"));
pkgJson.name = process.env.NPM_PACKAGE_NAME ?? "@m62624/elenchus";
pkgJson.main = "index.js";
pkgJson.types = "index.d.ts";
pkgJson.files = [
  "elenchus_bg.wasm",
  "elenchus.js",
  "elenchus.d.ts",
  "index.js",
  "index.d.ts",
  "SKILL.md",
  "README.md",
];
writeFileSync(pkgJsonPath, `${JSON.stringify(pkgJson, null, 2)}\n`);

console.log(`Assembled ${pkgJson.name}@${pkgJson.version} in ${pkg}`);
