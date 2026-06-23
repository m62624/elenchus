# elenchus (wasm / npm)

A **WebAssembly build of the [elenchus](https://github.com/m62624/elenchus) engine** —
a small three-valued SAT checker for logical consistency. You feed it a `.vrf`
program as a string and get the verdict back as JSON (or a human report): program
text in, JSON out. The engine (`parse → compile → solve`) is reused verbatim from
the Rust core; nothing is reimplemented here.

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
} from "@m62624/elenchus"; // package name TBD

// Inline program (no IMPORT resolution):
check("DOMAIN d\nFACT x a\nNOT x a\nCHECK x");
// -> '{"status":"CONFLICT","exit_code":2, ...}'

// Options mirror the MCP surface:
check(program, "human");                 // human-readable report
check(program, "json", maxClasses, maxPerClass); // cap grouped syntax errors

// Files (Node): read by the JS layer; multi-file IMPORT resolves via Node fs.
checkFile("program.vrf");
checkFileWithImports("entry.vrf");

version();      // "elenchus 0.9.1"  (the ENGINE version, not the npm version)
skill();        // the full companion SKILL.md text
skillVersion(); // "0.9.1" — the version the bundled skill targets
about();         // short pointer to the skill
```

`version()` / `skillVersion()` report the **engine** version (e.g. `0.9.1`),
which is independent of this npm package's own version line.

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

MIT © Mansur Azatbek. See the [repository](https://github.com/m62624/elenchus).
