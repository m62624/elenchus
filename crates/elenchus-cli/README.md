# elenchus-cli

> ⚠️ **Experimental.** elenchus is a reasoning-verification engine for AI coding
> models — used in equal measure by small local models and cloud models like
> Claude Code, both to drive it (write the DSL, read the verdict) and to build
> it. It is maintained with AI assistance and may contain non-professional design
> choices, rough edges, broken behavior, or mistakes. Use it at your own risk.

The `elenchus` command-line interface — check a `.vrf` program (file, inline
text, or stdin) and print the verdict. A thin `std` wrapper over the engine
crates (`elenchus-parser` → `elenchus-compiler` → `elenchus-solver`).

## Usage

```console
$ elenchus path/to/program.vrf          # check a file (IMPORTs resolve relative to it)
$ elenchus --text "FACT x a
CHECK x"                                  # inline program
$ cat program.vrf | elenchus            # stdin
$ elenchus program.vrf --format json    # machine-readable output
```

Exit code doubles as a CI gate:

| Code | Meaning |
|------|---------|
| 0 | CONSISTENT |
| 1 | UNDERDETERMINED or WARNING |
| 2 | CONFLICT, or a parse/compile error |

## Output

Human (default):

```text
RESULT: WARNING
  WARNING   wings_need_bone (AXIOM)  [creature.vrf:15]
      blocked by: Creature.A has wing
SUMMARY: 0 conflicts, 0 underdetermined, 1 warnings, 0 derived
EXIT_CODE: 1
```

JSON (`--format json`) — one line, for tooling and agents:

```json
{"status":"CONSISTENT","exit_code":0,"conflicts":[],"warnings":[],"derived":[],"underdetermined":null}
```

## License

MIT — see the [workspace LICENSE](../../LICENSE).
