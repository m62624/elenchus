# elenchus-parser

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

Part of [elenchus](https://github.com/m62624/elenchus): the parser for its
English-like consistency-checking DSL, turning program text into an AST.

`no_std` (needs `alloc`), built on `nom` + `nom_locate`. Zero-copy over `&str`,
line/column tracking, and human-friendly errors with a caret under the offending
token.

The syntax is line- and keyword-oriented (not S-expressions): keywords are always
CAPS, content is lowercase, and **indentation is cosmetic** — block boundaries are
found by keywords, never by indent depth. This is deliberately easy for a small
model to emit without tripping on parentheses or whitespace.

## Surface

```vrf
DOMAIN zoo
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

`parse` returns a flat `Program` — one `Statement` per line:

```rust
use elenchus_parser::{parse, Statement};

let program = parse("FACT Creature.A has flying\nCHECK Creature.A\n").unwrap();
assert_eq!(program.statements.len(), 2);
assert!(matches!(program.statements[0], Statement::Fact { .. }));
```

On malformed input `parse` returns [`Diagnostics`] instead — *every* syntax error
from one pass (the parser recovers and keeps going), each rendered as a caret block
and grouped by class, with the keyword's correct form and an example shown once per
class. This is the same rendering the CLI prints:

```text
RESULT: 1 syntax error in creature.vrf

THEN  (1 problem)
  syntax  : THEN <literal>
  example : THEN motor uses fast_path
    line 4, col 1 - expected THEN to complete the WHEN ... THEN implication
      | CHECK Creature.A
      | ^^^^^^^^^^^^^^^^
```

[`Diagnostics`]: https://docs.rs/elenchus-parser

## License

MIT — see [LICENSE](LICENSE).
