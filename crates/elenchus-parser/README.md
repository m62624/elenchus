# elenchus-parser

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

Parser for the English-like [elenchus](https://github.com/m62624/elenchus) consistency-checking DSL.

`no_std` (needs `alloc`), built on `nom` + `nom_locate`. Zero-copy over `&str`,
line/column tracking, and human-friendly errors with a `^--- here` caret.

The syntax is line- and keyword-oriented (not S-expressions): keywords are always
CAPS, content is lowercase, and **indentation is cosmetic** — block boundaries are
found by keywords, never by indent depth. This is deliberately easy for a small
model to emit without tripping on parentheses or whitespace.

## Surface

```vrf
IMPORT "physics.vrf"

FACT Creature.A has flying
NOT  Creature.A has cold_blood

PREMISE fly_xor_swim:
    EXCLUSIVE
        Creature.A has flying
        Creature.A has swimming

RULE needs_oxygen:
    WHEN Creature.A has flying
    THEN Creature.A needs oxygen

CHECK Creature.A BIDIRECTIONAL
```

Beyond the basics, bodies also take `ONEOF`/`ATLEAST` and `EXISTS <b> IN <set>`
(at least one element of a set), and `SET` + `FOR EACH` quantify a premise over a
set or relation. `CLOSE <rel> TRANSITIVE|SYMMETRIC|REFLEXIVE|EQUIVALENCE|SCC` closes
a relation. A syntax error groups every mistake **by class** and prints the correct
form + example once per class, so the output stays readable even on messy input.

## Usage

```rust
use elenchus_parser::{parse, Statement};

let program = parse("FACT Creature.A has flying\n").unwrap();
assert!(matches!(program.statements[0], Statement::Fact(_)));
```

On a malformed input, `parse` returns a `ParseError` whose `Display` points at the
real problem:

```text
Syntax Error at line 3, col 1: expected THEN to complete the WHEN ... THEN implication
  | CHECK Creature.A
  | ^--- here
```

## License

MIT — see [LICENSE](LICENSE).
