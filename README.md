# elenchus

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

elenchus is a small **SAT checker** with a thin **English-like DSL** on top, meant to
be driven by an LLM. You write **facts** and **first principles** (premises); a Rust
engine does the boolean bookkeeping under three-valued logic (TRUE / FALSE / UNKNOWN,
where UNKNOWN ≠ FALSE) and reports whether they are consistent. The split is the
point: the model supplies premises, the engine performs every inference step — so the
model can get a premise wrong, but never a step in a long chain, and a wrong premise
is caught mechanically.

It is a *simplified* SAT checker, not an SMT solver: the boolean core only, no
arithmetic. The engine and DSL can be used directly, but the project is aimed at
**small local models** — the reduced surface is what keeps it within their reach. It
also runs against large hosted models; whether that is worth doing depends on the
task. This is a consistency checker for problems that reduce to boolean constraints,
not a general-purpose tool for every task.

The name comes from *elenchus* (ἔλεγχος) — Socratic refutation by finding
contradictions; that is the spirit, not the mechanism. Mechanically it is a small
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

Three ordinary facts, each fine in isolation, that collide two inference steps apart —
the kind of thing a model loses track of reading a file top to bottom. `app` pulls in
`orm_v2` (which forces an async runtime, hence `tokio`) and `legacy_io` (which forces
a `blocking` runtime), while a premise forbids running both at once. The engine chains
the rules forward and reports the exact path to the contradiction.

```vrf
// runtime.vrf
DOMAIN build
FACT app uses orm_v2
FACT app uses legacy_io

RULE orm_needs_async:                 // orm_v2 => an async runtime
    WHEN app uses orm_v2
    THEN app needs async_runtime
RULE async_is_tokio:                  // async runtime => tokio
    WHEN app needs async_runtime
    THEN app runtime_is tokio

RULE io_needs_blocking:               // legacy_io => a blocking runtime, by default
    WHEN app uses legacy_io
    THEN app runtime_is blocking
    UNLESS app has compat_shim        // ...unless a compat shim is present

PREMISE one_runtime:                  // the two runtimes are mutually exclusive
    FORBIDS
        app runtime_is tokio
        app runtime_is blocking

CHECK app
```

```console
$ elenchus-cli runtime.vrf
RESULT: CONFLICT
  CONFLICT  one_runtime (FORBIDS)  [runtime.vrf:18]
      build.app runtime_is tokio
      build.app runtime_is blocking
      why:
        build.app uses orm_v2 = TRUE   [FACT runtime.vrf:3]
        build.app needs async_runtime = TRUE   from orm_needs_async (RULE)  [runtime.vrf:6]  <= build.app uses orm_v2
        build.app runtime_is tokio = TRUE   from async_is_tokio (RULE)  [runtime.vrf:9]  <= build.app needs async_runtime
        build.app uses legacy_io = TRUE   [FACT runtime.vrf:4]
        build.app runtime_is blocking = TRUE   from io_needs_blocking (RULE)  [runtime.vrf:13]  <= build.app uses legacy_io
  DERIVED   build.app needs async_runtime = TRUE   from orm_needs_async (RULE)  [runtime.vrf:6]
  DERIVED   build.app runtime_is tokio = TRUE   from async_is_tokio (RULE)  [runtime.vrf:9]
  DERIVED   build.app runtime_is blocking = TRUE   from io_needs_blocking (RULE)  [runtime.vrf:13]
SUMMARY: 1 conflicts, 0 underdetermined, 0 warnings, 3 derived
EXIT_CODE: 2
```

The `why:` trace is the point: it names every step from the two root facts down to the
clash, so the contradiction is located mechanically rather than by eye.

**The fix — establish the exception.** `io_needs_blocking` is a *default* (`UNLESS`):
it derives `blocking` only while `compat_shim` is not established. Adding
`FACT app has compat_shim` (below the two facts) defeats that default, the `blocking`
derivation is retracted, and the conflict is gone — this is the one non-monotonic
construct in the DSL, so the same rules now settle instead of clashing:

```console
$ elenchus-cli runtime.vrf         # after adding `FACT app has compat_shim`
RESULT: CONSISTENT
  DERIVED   build.app needs async_runtime = TRUE   from orm_needs_async (RULE)  [runtime.vrf:7]
  DERIVED   build.app runtime_is tokio = TRUE   from async_is_tokio (RULE)  [runtime.vrf:10]
  DEFEATED  io_needs_blocking (RULE)  [runtime.vrf:14]   default build.app runtime_is blocking suppressed by build.app has compat_shim
SUMMARY: 0 conflicts, 0 underdetermined, 0 warnings, 2 derived, 1 defeated
EXIT_CODE: 0
```

The rest of the vocabulary follows the same shape — one keyword per line, CAPS keyword
first. Every file opens with `DOMAIN <name>` (the namespace of its atoms); `FACT`/`NOT`
assert TRUE/FALSE (anything unstated is UNKNOWN, not false), optionally
`FACT … BECAUSE …` to name a ground; `ASSUME` adds a soft, retractable hypothesis (on a
clash the engine says which to drop, never blaming a `FACT`); `PREMISE` states a checked
first principle (`EXCLUSIVE`/`FORBIDS`/`ONEOF`/`ATLEAST`, `EXISTS … IN` a set or
`EXISTS … WITNESS` a named element, or `WHEN … THEN`); `RULE` derives facts, and
`RULE … UNLESS` makes it a default with exceptions; `SET` + `FOR EACH` quantify a body
over a set or relation, and `CLOSE <rel> TRANSITIVE|SYMMETRIC|REFLEXIVE|EQUIVALENCE|SCC`
closes a relation at compile time; `TRY` reports whether a candidate would pin an open
model without committing it; `KNOWS`/`BELIEVES` attribute a claim to a named agent
(knowledge is factive, belief is not); `IMPORT` reuses another domain (its atoms are
`<domain>.<atom>`); `CHECK` (optionally `BIDIRECTIONAL`) runs it. See
[`docs/SPEC.md`](docs/SPEC.md) for the grammar and every construct in full — including
**Performance**, which explains why the grammar makes a blow-up unrepresentable, and
[`docs/examples/`](docs/examples) for runnable programs.

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

Both run the **same engine**, return the **same verdicts**, and expose the **same
capabilities** — `IMPORT` / multi-domain, `VAR` ports, and data files all work on
every surface (CLI, MCP, and the wasm/npm build). **All three can read files from
the filesystem;** they differ only in transport and *how a resolver gets a file's
text* — so **pick whichever your harness is already wired for**:

- **CLI** — reads the **filesystem** by path (`FileResolver`). The simplest: point
  it at a file and imports just resolve.
- **MCP** — *two* entry modes. Inline (`program` + an in-memory `files: { path:
  text }` map) works on a local **or** remote server. Or `path`: a filesystem path
  the server reads and resolves imports from disk, exactly like the CLI — this needs
  the server running **locally with filesystem access** (the one bit of extra setup;
  a remote server can't see your files).
- **wasm/npm** — calls a host-supplied **`read(path) => string` callback**. In Node
  that reads the real filesystem (`checkFileWithImports` wires up `fs.readFileSync`);
  a browser host can back it with any virtual store.

| | CLI (`elenchus-cli`) | MCP (`elenchus-mcp`) |
|---|---|---|
| Transport | shell command | stdio JSON-RPC |
| Entry input | a file, `--text`, or stdin | inline `program`, or `path` to a file |
| **`IMPORT` / multi-domain** | ✅ from the filesystem | ✅ from disk (`path`) **or** inline (`files` map) |
| **`VAR` ports** | `--set "k:true"` and/or `--data file.vrf` | `values: { k: true }` and/or `data: { name: text }` |
| Hide the PLACEHOLDERS section | `--hide-params` | n/a (JSON always carries it) |
| Filesystem setup | none | only for `path` mode (local server) |

**Prefer the CLI** when your harness can run a shell (Claude Code, CI, a terminal):
no configuration, files come straight from disk. Reach for **MCP** when your harness
speaks MCP natively and you'd rather not (or can't) run a shell — same capabilities,
just supply files inline (`files`) or, on a local server, via `path`.

The one shared exception is **pure inline text** (CLI `--text`/stdin, MCP `program`
without `files`, wasm's `check`): a single source resolves no `IMPORT`. The moment
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
AI agents (plus `elenchus_version` / `elenchus_about`). The entry is **either**
inline `program` **or** a filesystem `path` (exactly one). With `program`, imports
resolve against an in-memory `files` (`{ "path": "<.vrf text>" }`) map — portable,
no filesystem needed; with `path`, the server reads the file and its imports from
disk like the CLI (local server only). `values` (`{ "port": true|false }`) and
`data` (`{ "name": "<PROVIDE text>" }`) bind `VAR` ports in either mode. See
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
