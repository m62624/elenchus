# elenchus

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

A small **SAT checker with three-valued logic** (TRUE / FALSE / UNKNOWN) for LLM
reasoning. You write **facts** and **first principles** (premises) in a tiny
English-like DSL; a Rust engine does the boolean bookkeeping and flags
contradictions. The model can only get a premise wrong — never a step in a long
chain — and that is caught mechanically.

It's a *simplified* SAT checker, not SMT: just the boolean core, no arithmetic.
That simpler DSL — plus the bundled skill that drives it — keeps it within reach
of local LLMs, not only large hosted ones.

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
| **WARNING** | 1 | a premise couldn't be checked — a needed atom is UNKNOWN |
| **UNDERDETERMINED** | 1 | satisfiable, but more than one model fits |
| **CONFLICT** | 2 | a premise is violated, or the premises are jointly unsatisfiable |

The intended loop: run → if not `CONSISTENT`, add the missing facts or rethink the
premises → re-run until `CONSISTENT`.

## Example

A claim that looks isolated, but collides with a chain of ordinary first
principles three steps away — the kind of thing a model loses track of reading
top to bottom. ([`docs/examples/socrates.vrf`](docs/examples/socrates.vrf).)

```vrf
DOMAIN philosophy
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

PREMISE mortal_xor_immortal:        // can't be both
    EXCLUSIVE
        socrates is mortal
        socrates is immortal

CHECK socrates
```

The engine derives `mortal` through the chain, then catches that it can't coexist
with the asserted `immortal`:

```console
$ elenchus-cli socrates.vrf
RESULT: CONFLICT
  CONFLICT  mortal_xor_immortal (EXCLUSIVE)  [socrates.vrf:31]
      philosophy.socrates is mortal
      philosophy.socrates is immortal
      why:
        philosophy.socrates is human = TRUE   [FACT socrates.vrf:15]
        philosophy.socrates is animal = TRUE   from humans_are_animals (RULE)  [socrates.vrf:19]  <= philosophy.socrates is human
        philosophy.socrates is living = TRUE   from animals_are_living (RULE)  [socrates.vrf:23]  <= philosophy.socrates is animal
        philosophy.socrates is mortal = TRUE   from living_things_are_mortal (RULE)  [socrates.vrf:27]  <= philosophy.socrates is living
        philosophy.socrates is immortal = TRUE   [FACT socrates.vrf:16]
  DERIVED   philosophy.socrates is animal = TRUE   from humans_are_animals (RULE)  [socrates.vrf:19]
  DERIVED   philosophy.socrates is living = TRUE   from animals_are_living (RULE)  [socrates.vrf:23]
  DERIVED   philosophy.socrates is mortal = TRUE   from living_things_are_mortal (RULE)  [socrates.vrf:27]
SUMMARY: 1 conflicts, 0 underdetermined, 0 warnings, 3 derived
EXIT_CODE: 2
```

The DSL: every file opens with `DOMAIN <name>` (the identity namespace of its
atoms); `FACT`/`NOT` assert TRUE/FALSE (anything unstated is UNKNOWN, not false);
`ASSUME` adds a soft, retractable hypothesis (on a clash the engine says which to
drop, never blaming a fact); `PREMISE` states a checked first principle
(`EXCLUSIVE`/`FORBIDS`/`ONEOF`/`ATLEAST`, or `WHEN … THEN`); `RULE` derives facts;
`IMPORT` reuses another domain (its atoms are `<domain>.<atom>`); `CHECK`
(optionally `BIDIRECTIONAL`) runs it. See SPEC.md for the grammar.

### Multi-step example — iterate to CONSISTENT

The real workflow: start with a broken program, read the conflict, fix it, re-run.
The model believes auth is optional, but a rule says an external service *must*
authenticate — a contradiction the engine catches and explains.

```vrf
// service.vrf
DOMAIN net
FACT service_api is external
FACT api_auth is optional

RULE auth_rule:                       // external ⇒ auth is required
    WHEN service_api is external
    THEN api_auth is required

PREMISE auth_state:                   // auth can't be both optional and required
    EXCLUSIVE
        api_auth is required
        api_auth is optional

CHECK service_api
```

**Step 1 — first run, conflict detected.** The `why:` trace gives the exact chain:
`external` forces `required` through the rule, which collides with the asserted
`optional`. (Atoms print with their domain, `net.…`.)

```console
$ elenchus-cli service.vrf
RESULT: CONFLICT
  CONFLICT  auth_state (EXCLUSIVE)  [service.vrf:10]
      net.api_auth is required
      net.api_auth is optional
      why:
        net.service_api is external = TRUE   [FACT service.vrf:3]
        net.api_auth is required = TRUE   from auth_rule (RULE)  [service.vrf:6]  <= net.service_api is external
        net.api_auth is optional = TRUE   [FACT service.vrf:4]
  DERIVED   net.api_auth is required = TRUE   from auth_rule (RULE)  [service.vrf:6]
SUMMARY: 1 conflicts, 0 underdetermined, 0 warnings, 1 derived
EXIT_CODE: 2
```

**Step 2 — fix: drop the wrong `FACT api_auth is optional`, re-run.** The rule
still derives `required`, and now nothing contradicts it (line numbers shift up
because the fact was removed):

```console
$ elenchus-cli service.vrf
RESULT: CONSISTENT
  DERIVED   net.api_auth is required = TRUE   from auth_rule (RULE)  [service.vrf:5]
SUMMARY: 0 conflicts, 0 underdetermined, 0 warnings, 1 derived
EXIT_CODE: 0
```

**Step 3 — add an undecided cache choice (an `EXCLUSIVE` over `cached`/`uncached`)
and ask `CHECK … BIDIRECTIONAL`.** It is satisfiable but no longer unique — the
backward pass says so and suggests how to pin it:

```console
$ elenchus-cli service.vrf
RESULT: UNDERDETERMINED
  UNDERDETERMINED  an alternative model exists
      pin it down: add  FACT net.service_api is uncached  or  NOT net.service_api is uncached
  DERIVED   net.api_auth is required = TRUE   from auth_rule (RULE)  [service.vrf:5]
SUMMARY: 0 conflicts, 1 underdetermined, 0 warnings, 1 derived
EXIT_CODE: 1
```

UNDERDETERMINED means satisfiable but not fully pinned — add the missing fact and
re-run until CONSISTENT.

### Trying a hypothesis — `ASSUME`

Sometimes you want to *test* a guess without committing to it. `ASSUME` is a soft
fact: it takes part in the check like a `FACT`, but if the guesses can't all hold,
the engine tells you which to drop — and never blames a real `FACT`/`PREMISE`.

```vrf
// service.vrf
DOMAIN net
FACT service_api is external
RULE auth_rule:
    WHEN service_api is external
    THEN api_auth is required
PREMISE auth_state:
    EXCLUSIVE
        api_auth is required
        api_auth is optional
ASSUME api_auth is optional           // what if we made auth optional here?
CHECK service_api
```

```console
$ elenchus-cli service.vrf
RESULT: CONFLICT
  RETRACT  your FACTs and PREMISEs are fine.
      But these ASSUME guesses cannot all be true together.
      Remove or flip ONE of them, then check again:
      ASSUME net.api_auth is optional   [service.vrf:11]
  DERIVED   net.api_auth is required = TRUE   from auth_rule (RULE)  [service.vrf:4]
SUMMARY: 1 conflicts, 0 underdetermined, 0 warnings, 1 derived
EXIT_CODE: 2
```

The verdict stays CONFLICT, but the fix is "drop the hypothesis", not "a fact is
wrong" — the engine did the backtracking for you.

## Install

Two binaries — the `elenchus` CLI (crate `elenchus-cli`) and the `elenchus-mcp`
server (crate `elenchus-mcp`) — built for **Linux, Windows and macOS (x64 &
arm64)** on every tagged release.

**Pick the one method that's convenient — you don't need more than one.** They all
install the *same* binaries. Quick guide:

| If you… | Use |
|---|---|
| are on macOS / Linux with Homebrew | **Homebrew** |
| don't want a Rust toolchain | **installer script**, or the Windows **`.msi`** |
| want managed install/uninstall on Windows | **`.msi`** |
| have `cargo` and want a cross-platform install | **`cargo binstall`** |
| want to compile it yourself | **from source** |

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

### Windows `.msi`

Download `elenchus-cli-*.msi` or `elenchus-mcp-*.msi` from the
[Releases page](https://github.com/m62624/elenchus/releases). Double-click to
install; it registers the app in **"Add or remove programs"**, so upgrades and
uninstalls go through the normal Windows UI.

### Windows PowerShell script (alternative to `.msi`)

If you prefer a script over a GUI installer:

```powershell
> powershell -ExecutionPolicy Bypass -c "irm https://github.com/m62624/elenchus/releases/latest/download/elenchus-cli-installer.ps1 | iex"
> powershell -ExecutionPolicy Bypass -c "irm https://github.com/m62624/elenchus/releases/latest/download/elenchus-mcp-installer.ps1 | iex"
```

### `cargo binstall`

[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) downloads the
prebuilt binary instead of compiling. It reads the release's cargo-dist
manifest, so it just works on every OS/arch above — no extra config:

```console
$ cargo binstall elenchus-cli     # the `elenchus-cli` binary
$ cargo binstall elenchus-mcp     # the `elenchus-mcp` binary
```

### From source

Needs a Rust toolchain; compiles locally and works on any platform Rust targets.
Both crates are published to crates.io, so you can build straight from there:

```console
$ cargo install elenchus-cli     # the `elenchus-cli` binary
$ cargo install elenchus-mcp     # the `elenchus-mcp` binary
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
$ cargo uninstall elenchus-cli      # removes the `elenchus-cli` binary
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
$ rm -f  ~/.cargo/bin/elenchus-cli ~/.cargo/bin/elenchus-mcp
$ rm -rf ~/.config/elenchus-cli ~/.config/elenchus-mcp     # install receipts
```

```powershell
# Windows (PowerShell)
> Remove-Item "$env:USERPROFILE\.cargo\bin\elenchus-cli.exe","$env:USERPROFILE\.cargo\bin\elenchus-mcp.exe" -ErrorAction SilentlyContinue
> Remove-Item "$env:LOCALAPPDATA\elenchus-cli","$env:LOCALAPPDATA\elenchus-mcp" -Recurse -ErrorAction SilentlyContinue
```

If you pointed the installer somewhere else (`ELENCHUS_CLI_INSTALL_DIR` /
`ELENCHUS_MCP_INSTALL_DIR`, or `CARGO_DIST_FORCE_INSTALL_DIR`), delete from that
directory instead. The installer may also have added the bin dir to your `PATH` —
prune that line from your shell profile if nothing else uses it.

## Use it

### CLI or MCP — which one?

Both run the **same engine**, return the **same verdicts**, and now expose the
**same capabilities** — `IMPORT` / multi-domain, `VAR` ports, and data files all
work on every surface (CLI, MCP, and the wasm/npm build). All three **resolve
imports**; they differ only in transport and *where a resolver gets a file's text*:

- **CLI** — reads the real **filesystem** by path (`FileResolver`).
- **MCP** — looks the path up in an **in-memory map** (`files: { path: text }`) sent
  inline in the request; the server itself touches no filesystem.
- **wasm/npm** — calls a host-supplied **`read(path) => string` callback**. In Node
  that callback reads the real filesystem (`checkFileWithImports` wires up
  `fs.readFileSync`); a browser host can back it with any virtual store.

| | CLI (`elenchus-cli`) | MCP (`elenchus-mcp`) |
|---|---|---|
| Transport | shell command | stdio JSON-RPC |
| Entry input | a file, `--text`, or stdin | one `program` string |
| **`IMPORT` / multi-domain** | ✅ from the **filesystem** (file mode) | ✅ via a `files: { path: text }` map |
| **`VAR` ports** | `--set "k:true"` and/or `--data file.vrf` | the `values: { k: true }` object and/or `data: { name: text }` |
| Hide the PLACEHOLDERS section | `--hide-params` | n/a (JSON always carries it) |
| Setup | none | configure the MCP server |

- **Use the CLI** when your harness can run a shell (Claude Code, CI, a terminal).
  Files come straight from disk, so multi-file templates and `--data` files need no
  inlining. **Recommended whenever a shell is available.**
- **Use the MCP server** when your harness speaks MCP natively and you'd rather not
  (or can't) run a shell. Same reach — for a multi-file template, send the imported
  sources inline in `files`; for data, pass `values` and/or `data`.

The one shared exception is **pure inline text** (CLI `--text`/stdin, MCP without
`files`, wasm's `check`): a single source resolves no `IMPORT`. The moment
files/imports are in play, all three resolve them identically — the path normalizer
is shared, so Windows- and Unix-style import paths behave the same everywhere.

The skill ([`skill/SKILL.md`](skill/SKILL.md)) is written for both and tells the
agent how to drive whichever transport it has.

### CLI

One input three ways: a positional `elenchus-cli <file.vrf>`, inline
`--text "<program>"`, or explicit stdin with `-`; `--text` and a file are
mutually exclusive. Running `elenchus-cli` with no input prints help instead of
waiting on stdin. `--format json` for tooling; exit code is the verdict (CI gate).
`VAR` ports take values via `--set "k:true"` and/or `--data file.vrf`;
`--hide-params` drops the PLACEHOLDERS section. Note: **`IMPORT` resolves only for
the file form** — `--text`/stdin are a single source. See
[`crates/elenchus-cli`](crates/elenchus-cli).

### MCP server

`elenchus-mcp` speaks stdio JSON-RPC and exposes one tool, `elenchus_check`, for
AI agents (plus `elenchus_version` / `elenchus_about`). `program` is the entry
source; optional arguments give it the CLI's full reach without a filesystem:
`files` (`{ "path": "<.vrf text>" }`) supplies the sources its `IMPORT`s resolve
against (multi-domain templates), while `values` (`{ "port": true|false }`) and
`data` (`{ "name": "<PROVIDE text>" }`) bind `VAR` ports. See
[`crates/elenchus-mcp`](crates/elenchus-mcp).

### Using the skill

The skill is a single self-contained file, [`skill/SKILL.md`](skill/SKILL.md).
It is **version-matched to the engine** and attached to every release, so grab the
one that matches your installed binary (`elenchus-cli --version`) from the
[Releases page](https://github.com/m62624/elenchus/releases).

(The repo copy tracks `main`; each release asset is pinned to that tag, and the
pipeline enforces that the file's `skill-version` marker equals the release.)
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
| [`elenchus-wasm`](crates/elenchus-wasm) | std | WebAssembly/JS build (`wasm-pack` → the `elenchus-wasm` npm package): `check(program, …, values)` in, JSON verdict out — for embedding the engine in a Node/browser host. |

The three library crates build for a `no_std` target (`wasm32v1-none`), verified
in CI. The same engine is also shipped to JavaScript as a WebAssembly module
(`elenchus-wasm`), so a JS/TS host can embed it directly — same verdicts, same
`VAR`-port `values` API.

## License

MIT — see [LICENSE](LICENSE).
