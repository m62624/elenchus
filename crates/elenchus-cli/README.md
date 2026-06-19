# elenchus-cli

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

The `elenchus` command-line interface — check a `.vrf` program (file, inline
text, or stdin) and print the verdict. A thin `std` wrapper over the engine
crates (`elenchus-parser` → `elenchus-compiler` → `elenchus-solver`).

## Usage

```console
$ elenchus path/to/program.vrf          # check a file (IMPORTs resolve relative to it)
$ elenchus --text "FACT x a
CHECK x"                                  # inline program
$ cat program.vrf | elenchus -          # stdin
$ elenchus program.vrf --format json    # machine-readable output
```

One input, three ways: a positional `<file>`, inline `--text`, or explicit stdin
with `-`. Running `elenchus` with no input prints help instead of waiting on
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

Human (default):

```text
RESULT: WARNING
  WARNING   wings_need_bone (PREMISE)  [creature.vrf:15]
      blocked by: Creature.A has wing
SUMMARY: 0 conflicts, 0 underdetermined, 1 warnings, 0 derived
EXIT_CODE: 1
```

JSON (`--format json`) — one line, for tooling and agents:

```json
{"status":"CONSISTENT","exit_code":0,"conflicts":[],"warnings":[],"derived":[],"underdetermined":null}
```

## License

MIT — see [LICENSE](LICENSE).
