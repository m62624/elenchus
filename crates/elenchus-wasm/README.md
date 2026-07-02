# elenchus (wasm / npm)

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

The **WebAssembly build** of [elenchus](https://github.com/m62624/elenchus), part of
that project — a small three-valued SAT checker for logical consistency. You feed it a
`.vrf` program as a string and get the verdict back as JSON (or a human report):
program text in, JSON out. The engine (`parse → compile → solve`) is reused verbatim
from the Rust core; nothing is reimplemented here.

It runs anywhere Node runs — no native binary, no PATH, no install step beyond
`npm install`. **TypeScript types are included** (`.d.ts` generated from the
engine signatures plus the Node helpers).

## API

```ts
import {
  check,
  checkFile,
  checkFileWithImports,
  version,
  about,
  skill,
  skillVersion,
} from "elenchus-wasm";

// Inline program (no IMPORT resolution):
check("DOMAIN d\nFACT x a\nNOT x a\nCHECK x");
// -> '{"status":"CONFLICT","exit_code":2, ...}'

// Options mirror the MCP surface:
check(program, "human");                 // human-readable report
check(program, "json", maxClasses, maxPerClass); // cap grouped syntax errors

// Files (Node): read by the JS layer; multi-file IMPORT resolves via Node fs.
checkFile("program.vrf");
checkFileWithImports("entry.vrf");

version();      // "elenchus x.y.z"  (the ENGINE version, not the npm version)
skill();        // the full companion SKILL.md text
skillVersion(); // "x.y.z" — the engine version the bundled skill targets
about();         // short pointer to the skill
```

`version()` reports the **engine** version and `skillVersion()` the skill's
target-version marker; both are independent of this npm package's own version line.

## What this package is — and isn't

This is a **library** with a small API. It is *not* the CLI or the MCP server.

- Want the **full Rust library API** (all of `elenchus-parser` /
  `elenchus-compiler` / `elenchus-solver`)? Use the crates in the
  [elenchus repository](https://github.com/m62624/elenchus).
- Want a **native binary** — the `elenchus` CLI or the `elenchus-mcp` server —
  for Linux, macOS, or Windows (as a shell tool or over MCP)? Grab a prebuilt
  release (cargo-dist: shell / PowerShell / `.msi` / Homebrew) from the same
  repository's [releases](https://github.com/m62624/elenchus/releases).

## The skill

`skill()` returns the companion **agent skill** (the DSL how-to, the verdict loop
"iterate until CONSISTENT", and worked examples), version-locked to the engine in
this package. A consumer can persist it next to the engine (e.g. into an agent's
skills directory) without a second download. The same text ships as `SKILL.md` in
this package.

## License

MIT — see [LICENSE](LICENSE).
