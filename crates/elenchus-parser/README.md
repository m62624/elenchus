# elenchus-parser

Parser for the English-like [elenchus](https://github.com/) reasoning DSL.

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

AXIOM fly_xor_swim:
    EXCLUSIVE
        Creature.A has flying
        Creature.A has swimming

RULE needs_oxygen:
    WHEN Creature.A has flying
    THEN Creature.A needs oxygen

CHECK Creature.A BIDIRECTIONAL
```

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

MIT — see the [workspace LICENSE](../../LICENSE).
