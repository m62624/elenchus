# elenchus

A formal reasoning-verification engine. You write **facts** and **first
principles** (axioms) in a tiny English-like DSL; a Rust engine does all the
logical inference and catches contradictions mathematically. The model can only
err at the axiom level — never in the middle of a long reasoning chain — and that
error is caught early and mechanically.

The name *elenchus* (ἔλεγχος) is the Socratic method of cross-examining claims
against first principles until a contradiction surfaces — exactly what the engine
does to a set of facts.

> **Full specification:** [`docs/SPEC.md`](docs/SPEC.md) — the epistemic basis
> (three-valued Kleene logic), the single `Impossible` primitive and its sugar,
> the grammar (EBNF), `IMPORT` semantics, and every invariant. This README is the
> overview; SPEC.md is the source of truth.

## What it does

Given a `.vrf` program it returns one of four verdicts (and a matching exit code):

| Result | exit | Meaning |
|--------|:----:|---------|
| **CONSISTENT** | 0 | no contradictions, and the answer is pinned down |
| **WARNING** | 1 | an axiom couldn't be checked — a needed atom is UNKNOWN |
| **UNDERDETERMINED** | 1 | satisfiable, but more than one model fits |
| **CONFLICT** | 2 | an axiom is violated, or the axioms are jointly unsatisfiable |

The intended loop: run → if not `CONSISTENT`, add the missing facts or rethink the
axioms → re-run until `CONSISTENT`.

## Example

```vrf
IMPORT "physics.vrf"          // a vetted axiom library

FACT Motor over_200
NOT  Motor over_100           // contradicts the imported speed_order axiom

CHECK Motor
```

```console
$ elenchus motor.vrf
RESULT: CONFLICT
  CONFLICT  speed_order (AXIOM)  [physics.vrf:9]
      Motor over_200
      Motor over_100
SUMMARY: 1 conflicts, 0 underdetermined, 0 warnings, 0 derived
EXIT_CODE: 2
```

The DSL: `FACT`/`NOT` assert TRUE/FALSE (anything unstated is UNKNOWN, not false);
`AXIOM` states a checked first principle (`EXCLUSIVE`/`FORBIDS`/`ONEOF`/`ATLEAST`,
or `WHEN … THEN`); `RULE` derives facts; `IMPORT` reuses a library; `CHECK`
(optionally `BIDIRECTIONAL`) runs it. See SPEC.md for the grammar.

## Install

Two binaries ship as prebuilt downloads on every tagged release (built by
cargo-dist for **Linux, Windows and macOS, x64 & arm64**): the `elenchus` CLI
(crate `elenchus-cli`) and the `elenchus-mcp` server (crate `elenchus-mcp`).
Pick whichever method you like — all of them pull the *same* prebuilt artifacts.

### 1. `cargo binstall` (recommended)

[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) downloads the
prebuilt binary instead of compiling. It reads the release's cargo-dist
manifest, so it just works on every OS/arch above — no extra config:

```console
$ cargo binstall elenchus-cli     # the `elenchus` CLI
$ cargo binstall elenchus-mcp     # the `elenchus-mcp` server
```

### 2. Installer scripts (no Rust toolchain needed)

Each binary has its own script on the
[Releases page](https://github.com/m62624/elenchus/releases); `latest` always
points at the newest tag.

```console
# Linux / macOS  (POSIX sh)
$ curl --proto '=https' --tlsv1.2 -LsSf https://github.com/m62624/elenchus/releases/latest/download/elenchus-cli-installer.sh | sh
$ curl --proto '=https' --tlsv1.2 -LsSf https://github.com/m62624/elenchus/releases/latest/download/elenchus-mcp-installer.sh | sh
```

```powershell
# Windows  (PowerShell)
> powershell -ExecutionPolicy Bypass -c "irm https://github.com/m62624/elenchus/releases/latest/download/elenchus-cli-installer.ps1 | iex"
> powershell -ExecutionPolicy Bypass -c "irm https://github.com/m62624/elenchus/releases/latest/download/elenchus-mcp-installer.ps1 | iex"
```

### 3. From source

Needs a Rust toolchain; compiles locally and works on any platform Rust targets.
Both crates are published to crates.io, so you can build straight from there:

```console
$ cargo install elenchus-cli     # the `elenchus` CLI
$ cargo install elenchus-mcp     # the `elenchus-mcp` server
```

…or from a local checkout of this repo:

```console
$ cargo install --path crates/elenchus-cli
$ cargo install --path crates/elenchus-mcp
```

## Use it

- **CLI** — `elenchus <file.vrf>` / `--text "<program>"` / stdin; `--format json`
  for tooling; exit code is the verdict (CI gate). See
  [`crates/elenchus-cli`](crates/elenchus-cli).
- **MCP server** — `elenchus-mcp` speaks stdio JSON-RPC and exposes one tool,
  `elenchus_check`, for AI agents. See [`crates/elenchus-mcp`](crates/elenchus-mcp).
- **Skill** — [`skill/SKILL.md`](skill/SKILL.md): when to reach for elenchus, the
  DSL, worked examples, and the iterate-to-CONSISTENT workflow.

## Workspace

| Crate | std? | Role |
|-------|:----:|------|
| [`elenchus-parser`](crates/elenchus-parser) | `no_std` | English-like DSL text → AST (`nom` + `nom_locate`, precise `^--- here` errors). |
| [`elenchus-compiler`](crates/elenchus-compiler) | `no_std` | AST → canonical `Impossible`/CNF clause IR: import resolution, desugaring, atom interning, sha256 content-addressed dedup. |
| [`elenchus-solver`](crates/elenchus-solver) | `no_std` | The interpreter: three-valued Kleene forward pass + a compact CDCL SAT core (varisat algorithm) for the backward pass. |
| [`elenchus-cli`](crates/elenchus-cli) | std | The `elenchus` command-line interface. |
| [`elenchus-mcp`](crates/elenchus-mcp) | std | The Model Context Protocol server. |

The three library crates build for a bare-metal `no_std` target
(`wasm32v1-none`), verified in CI.

## Status

All five crates implemented and tested (parser → compiler → solver, plus CLI and
MCP), property-tested against a brute-force SAT oracle, `clippy`-clean, and
`no_std`-verified on bare metal. CI runs fmt/clippy/test on Linux/Windows/macOS;
tagged binary releases are built by cargo-dist.

## License

MIT — see [LICENSE](LICENSE).
