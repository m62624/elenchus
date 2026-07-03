# elenchus-cli

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

The command-line interface of [elenchus](https://github.com/m62624/elenchus), part of
that project. It reads a `.vrf` program (a file, inline text, or stdin), runs the
consistency check, and prints the verdict. A thin `std` wrapper over the engine crates
(`elenchus-parser` → `elenchus-compiler` → `elenchus-solver`).

The companion **skill** ([`skill/SKILL.md`](https://github.com/m62624/elenchus/blob/main/skill/SKILL.md)) teaches an LLM agent
how to drive it end to end — the DSL, worked examples, and the iterate-to-CONSISTENT
loop. It works in any harness that can run shell tools.

## Usage

One input, three ways — a positional file, inline `--text`, or stdin `-`:

```console
$ elenchus-cli path/to/program.vrf              # a file (IMPORTs resolve relative to it)
$ elenchus-cli --text "DOMAIN d
FACT x a
CHECK x"                                          # inline, multi-line
$ printf 'DOMAIN d\nFACT x a\nNOT x a\nCHECK x\n' | elenchus-cli -   # stdin, one line
$ elenchus-cli program.vrf --format json          # machine-readable, one line out
$ elenchus-cli broken.vrf --max-per-class 3       # cap places shown per error class
$ elenchus-cli slow.vrf --max-conflicts 100000    # safety valve (exit 3 if exceeded)
```

`--text` and a file are mutually exclusive; with no input at all the CLI prints help
instead of blocking on stdin. **`IMPORT` resolves only for the file form** — `--text`
and stdin are treated as a single source.

Exit code doubles as a CI gate:

| Code | Meaning |
|------|---------|
| 0 | CONSISTENT |
| 1 | UNDERDETERMINED or WARNING |
| 2 | CONFLICT, or a parse/compile error |
| 3 | `--max-conflicts` budget exceeded — the check did **not** finish, no verdict |

**`--max-conflicts`** — safety valve, only set if a check hangs. Aborts (exit 3, no verdict) if the solver needs more SAT conflicts than this (`0` or omitted = unlimited). Recommended: **100000**.

## Output

**Human (default).** Here a gate whose consequent has not been established yet — the
premise cannot be checked, so the verdict is `WARNING` and the report says exactly
which atom is missing and how to supply it:

```console
$ elenchus-cli ready.vrf
RESULT: WARNING
  WARNING   ready (PREMISE)  [ready.vrf:3]
      blocked by: web.svc tested
      fix: nothing determines `web.svc tested` — add `FACT web.svc tested` (or `NOT …`), or if a PREMISE's THEN is meant to establish it, make that PREMISE a RULE so it derives the value
SUMMARY: 0 conflicts, 0 underdetermined, 1 warnings, 0 derived
EXIT_CODE: 1
```

**JSON (`--format json`)** — a single line, for tooling and agents. Here a CONSISTENT
run that forward-chained one derived fact:

```json
{"status":"CONSISTENT","exit_code":0,"conflicts":[],"warnings":[],"derived":[{"premise":"r","kind":"RULE","source":"<text>","line":3,"atom":"d.a ready","value":true}],"defeated":[],"underdetermined":null,"unsat_core":[],"retract":[],"hints":[],"orphans":[],"unused_imports":[],"placeholders":[],"tried":[],"beliefs":[]}
```

### Syntax errors

A malformed program exits `2` and prints every error found in one pass, **grouped by
class** (one class per keyword): the correct syntax and a real example are shown *once
per class*, with each offending place listed beneath — line, caret, and the specific
problem.

```console
$ elenchus-cli broken.vrf
RESULT: 1 syntax error in broken.vrf

THEN  (1 problem)
  syntax  : THEN <literal>
  example : THEN motor uses fast_path
    line 4, col 9 - THEN expects a literal: [NOT] <Subject> <predicate> [<object>]
      |     THEN
      |         ^
```

Two independent caps control the volume (both default to "all"):

| Flag | Caps | Footer when it hides something |
|------|------|--------------------------------|
| `--max-classes N` | number of classes shown | `… and N more classes` |
| `--max-per-class N` | places shown within each class | `… and N more <keyword> problems` |

Set just one to cap that dimension and leave the other full; set both for full control.

## License

MIT — see [LICENSE](LICENSE).
