# elenchus-cli

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

The `elenchus-cli` command-line interface — check a `.vrf` program (file, inline
text, or stdin) and print the verdict. A thin `std` wrapper over the engine
crates (`elenchus-parser` → `elenchus-compiler` → `elenchus-solver`).

The **skill** ([`skill/SKILL.md`](../../skill/SKILL.md)) teaches an LLM agent how
to use elenchus end-to-end — when to reach for it, the DSL, worked examples, and
the iterate-to-CONSISTENT workflow. It's adapted for the CLI and works in any
harness that supports shell tools.

## Usage

```console
$ elenchus-cli path/to/program.vrf          # check a file (IMPORTs resolve relative to it)
$ elenchus-cli --text "FACT x a
CHECK x"                                      # inline program
$ cat program.vrf | elenchus-cli -          # stdin
$ elenchus-cli program.vrf --format json    # machine-readable output
$ elenchus-cli broken.vrf --max-errors 5    # cap a flood of syntax errors
```

One input, three ways: a positional `<file>`, inline `--text`, or explicit stdin
with `-`. Running `elenchus-cli` with no input prints help instead of waiting on
stdin. `--text` and a file are mutually exclusive. **`IMPORT` resolves only for
the file form** — `--text` and stdin are treated as a single source, so a program
using `IMPORT` must be passed as a file.

Exit code doubles as a CI gate:

| Code | Meaning |
|------|---------|
| 0 | CONSISTENT |
| 1 | UNDERDETERMINED or WARNING |
| 2 | CONFLICT, or a parse/compile error |

## Output

Human (default) — e.g. a gate whose consequent has not been stated yet:

```text
$ elenchus-cli ready.vrf
RESULT: WARNING
  WARNING   ready (PREMISE)  [ready.vrf:2]
      blocked by: svc tested
SUMMARY: 0 conflicts, 0 underdetermined, 1 warnings, 0 derived
EXIT_CODE: 1
```

JSON (`--format json`) — one line, for tooling and agents:

```json
{"status":"CONSISTENT","exit_code":0,"conflicts":[],"warnings":[],"derived":[],"underdetermined":null,"unsat_core":[],"retract":[],"hints":[]}
```

### Syntax errors

A malformed program exits `2` and prints one block per error — the line number,
the offending line, a caret, the problem, and that keyword's correct syntax with
a real example. **Every** error is collected in one pass (the parser recovers and
keeps going); `--max-errors N` shows the first `N` with a `(showing N of TOTAL)`
footer so a large broken file does not flood the output.

```text
$ elenchus-cli broken.vrf
RESULT: 2 syntax errors in broken.vrf

[1/2] line 1, col 6
   | FACT lonely
   |      ^^^^^^
   problem : FACT expects an atom: <Subject> <predicate> [<object>]
   syntax  : FACT <Subject> <predicate> [<object>]
   example : FACT socrates is human

[2/2] line 4, col 9
   |     THEN
   |         ^
   problem : THEN expects a literal: [NOT] <Subject> <predicate> [<object>]
   syntax  : THEN <literal>
   example : THEN motor uses fast_path
```

## License

MIT — see [LICENSE](LICENSE).
