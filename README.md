# elenchus

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

A small **SAT checker with three-valued logic** (TRUE / FALSE / UNKNOWN), aimed at
small local LLMs. You write **facts** and **first principles** (axioms) in a tiny
English-like DSL; a Rust engine does the boolean bookkeeping and flags
contradictions. The model can only get an axiom wrong — never a step in a long
chain — and that is caught mechanically.

The name comes from *elenchus* (ἔλεγχος) — Socratic refutation by finding
contradictions; that's just the spirit of it. Mechanically it's a small
consistency/SAT checker, not a dialogue.

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

A claim that looks isolated, but collides with a chain of ordinary first
principles three steps away — the kind of thing a model loses track of reading
top to bottom. ([`docs/examples/socrates.vrf`](docs/examples/socrates.vrf).)

```vrf
FACT socrates is human
FACT socrates is immortal        // the claim being cross-examined

RULE humans_are_animals:
    WHEN socrates is human
    THEN socrates is animal
RULE animals_are_living:
    WHEN socrates is animal
    THEN socrates is living
RULE living_things_are_mortal:
    WHEN socrates is living
    THEN socrates is mortal

AXIOM mortal_xor_immortal:        // can't be both
    EXCLUSIVE
        socrates is mortal
        socrates is immortal

CHECK socrates
```

The engine derives `mortal` through the chain, then catches that it can't coexist
with the asserted `immortal`:

```console
$ elenchus socrates.vrf
RESULT: CONFLICT
  CONFLICT  mortal_xor_immortal (EXCLUSIVE)  [socrates.vrf:29]
      socrates is mortal
      socrates is immortal
  DERIVED   socrates is animal = TRUE   from humans_are_animals (RULE)  [socrates.vrf:17]
  DERIVED   socrates is living = TRUE   from animals_are_living (RULE)  [socrates.vrf:21]
  DERIVED   socrates is mortal = TRUE   from living_things_are_mortal (RULE)  [socrates.vrf:25]
SUMMARY: 1 conflicts, 0 underdetermined, 0 warnings, 3 derived
EXIT_CODE: 2
```

The DSL: `FACT`/`NOT` assert TRUE/FALSE (anything unstated is UNKNOWN, not false);
`AXIOM` states a checked first principle (`EXCLUSIVE`/`FORBIDS`/`ONEOF`/`ATLEAST`,
or `WHEN … THEN`); `RULE` derives facts; `IMPORT` reuses a library; `CHECK`
(optionally `BIDIRECTIONAL`) runs it. See SPEC.md for the grammar.

## Install

Two binaries — the `elenchus` CLI (crate `elenchus-cli`) and the `elenchus-mcp`
server (crate `elenchus-mcp`) — built for **Linux, Windows and macOS (x64 &
arm64)** on every tagged release.

**Pick the one method that's convenient — you don't need more than one.** They all
install the *same* binaries. Quick guide:

| If you… | Use |
|---|---|
| have `cargo` and want the simplest cross-platform install | **`cargo binstall`** |
| are on macOS / Linux with Homebrew | **Homebrew** |
| don't want a Rust toolchain | **installer script**, or the Windows **`.msi`** |
| want managed install/uninstall on Windows | **`.msi`** |
| want to compile it yourself | **from source** |

### `cargo binstall` (recommended)

[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) downloads the
prebuilt binary instead of compiling. It reads the release's cargo-dist
manifest, so it just works on every OS/arch above — no extra config:

```console
$ cargo binstall elenchus-cli     # the `elenchus` CLI
$ cargo binstall elenchus-mcp     # the `elenchus-mcp` server
```

### Homebrew (macOS / Linux)

From the [`m62624/homebrew-elenchus`](https://github.com/m62624/homebrew-elenchus)
tap; `brew upgrade` / `brew uninstall` then manage it like any formula:

```console
$ brew install m62624/elenchus/elenchus-cli     # the `elenchus` CLI
$ brew install m62624/elenchus/elenchus-mcp     # the `elenchus-mcp` server
```

### Installer scripts (no Rust toolchain)

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

### Windows `.msi` (managed install/uninstall)

Each binary also ships a `.msi` (`elenchus-cli-*.msi`, `elenchus-mcp-*.msi`) on the
Releases page. Double-click to install; it registers the app in **"Add or remove
programs"**, so upgrades and uninstalls go through the normal Windows UI — the
friendlier choice over the PowerShell script.

### From source

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

### Uninstall

**Installed with `cargo binstall` / `cargo install`** (either resolves to cargo's
own install tracking, so plain `cargo uninstall` works):

```console
$ cargo uninstall elenchus-cli      # removes the `elenchus` binary
$ cargo uninstall elenchus-mcp
```

**Installed with Homebrew:** `brew uninstall elenchus-cli elenchus-mcp`.

**Installed from a Windows `.msi`:** uninstall from **"Add or remove programs"**
(or Settings → Apps), exactly like any other Windows app.

**Installed with the shell/PowerShell scripts:** cargo-dist does not ship an
uninstaller, so remove the binaries and their install receipts by hand. By default
the binaries land in `~/.cargo/bin` (note: `cargo uninstall` won't touch these —
cargo didn't track them), and a receipt is written per app.

```console
# Linux / macOS
$ rm -f  ~/.cargo/bin/elenchus ~/.cargo/bin/elenchus-mcp
$ rm -rf ~/.config/elenchus-cli ~/.config/elenchus-mcp     # install receipts
```

```powershell
# Windows (PowerShell)
> Remove-Item "$env:USERPROFILE\.cargo\bin\elenchus.exe","$env:USERPROFILE\.cargo\bin\elenchus-mcp.exe" -ErrorAction SilentlyContinue
> Remove-Item "$env:LOCALAPPDATA\elenchus-cli","$env:LOCALAPPDATA\elenchus-mcp" -Recurse -ErrorAction SilentlyContinue
```

If you pointed the installer somewhere else (`ELENCHUS_CLI_INSTALL_DIR` /
`ELENCHUS_MCP_INSTALL_DIR`, or `CARGO_DIST_FORCE_INSTALL_DIR`), delete from that
directory instead. The installer may also have added the bin dir to your `PATH` —
prune that line from your shell profile if nothing else uses it.

## Use it

- **CLI** — one input three ways: a positional `elenchus <file.vrf>`, inline
  `--text "<program>"`, or explicit stdin with `-`; `--text` and a file are
  mutually exclusive. Running `elenchus` with no input prints help instead of
  waiting on stdin. `--format json` for tooling; exit code is the verdict (CI gate).
  Note: **`IMPORT` resolves only for the file form** — `--text`/stdin are a single
  source. See [`crates/elenchus-cli`](crates/elenchus-cli).
- **MCP server** — `elenchus-mcp` speaks stdio JSON-RPC and exposes one tool,
  `elenchus_check`, for AI agents. See [`crates/elenchus-mcp`](crates/elenchus-mcp).
- **Skill** — [`skill/SKILL.md`](skill/SKILL.md): when to reach for elenchus, the
  DSL, worked examples, a self-check, and the iterate-to-CONSISTENT workflow.

### Using the skill

The skill is a single self-contained file, [`skill/SKILL.md`](skill/SKILL.md).
**Copy it verbatim (one-to-one) into wherever your agent loads skills from** — the
location depends on the host, and most LLM harnesses already know how to pick a
skill file up:

- **Claude Code** — put it at `~/.claude/skills/elenchus/SKILL.md` (user-wide) or
  `.claude/skills/elenchus/SKILL.md` (per-project); it loads on the next session.
- **Other harnesses** — drop the file in whatever directory that host scans for
  skills/tools; the YAML frontmatter (`name`, `description`) is what they import.

Don't edit the contents when copying — the frontmatter and the worked examples are
load-bearing. After copying, **verify it imported**: ask the agent to run the
skill's own Step 0 smoke-test (`FACT x a` / `NOT x a` / `CHECK x` → `CONFLICT`); if
the skill is loaded and elenchus is installed, it will know to do exactly that.

## Workspace

| Crate | std? | Role |
|-------|:----:|------|
| [`elenchus-parser`](crates/elenchus-parser) | `no_std` | English-like DSL text → AST (`nom` + `nom_locate`, precise `^--- here` errors). |
| [`elenchus-compiler`](crates/elenchus-compiler) | `no_std` | AST → canonical `Impossible`/CNF clause IR: import resolution, desugaring, atom interning, sha256 content-addressed dedup. |
| [`elenchus-solver`](crates/elenchus-solver) | `no_std` | The interpreter: three-valued Kleene forward pass + a compact CDCL SAT core (varisat algorithm) for the backward pass. |
| [`elenchus-cli`](crates/elenchus-cli) | std | The `elenchus` command-line interface. |
| [`elenchus-mcp`](crates/elenchus-mcp) | std | The Model Context Protocol server. |

The three library crates build for a `no_std` target (`wasm32v1-none`), verified
in CI.

## Status

**Verified** (in CI and locally): all five crates implemented and tested
(parser → compiler → solver, plus CLI and MCP); the SAT core is property-tested
against a brute-force oracle; `clippy`-clean with `-D warnings`; the three library
crates build for the `no_std` target `wasm32v1-none`. CI runs fmt/clippy/test on
Linux/Windows/macOS.

**Not yet proven — treat as experimental.** The release pipeline and the
installers (shell/PowerShell/`.msi`/Homebrew, `cargo binstall`, crates.io publish)
are *configured* but have **not** been exercised by a real tagged release. Until
the first release runs green, assume some installer or publish step may fail —
verify whichever path you use, and expect fixes. `dist plan` passes, but that only
validates the plan, not an actual build/upload.

## License

MIT — see [LICENSE](LICENSE).
