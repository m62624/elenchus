# elenchus-compiler

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

Compiles the parsed [elenchus](https://github.com/m62624/elenchus) DSL into a canonical,
solver-ready intermediate representation. **Preparation only — no solving.**

`no_std` (needs `alloc`); the optional `std` feature adds a filesystem-backed
import resolver.

## What it does

- **Atom interner** — `(subject, predicate, object?)` → dense `u32` ids,
  canonically sorted so ids (and any later enumeration) are deterministic.
- **Desugaring** to the single `Impossible` primitive: `EXCLUSIVE`/`FORBIDS`
  pairwise, `ONEOF` = pairwise + at-least-one, `ATLEAST` = one all-negated clause,
  `WHEN … THEN` → `Impossible([A.., NOT C])` per consequent. `RULE` bodies are
  kept separate as forward-chaining rules.
- **Source-agnostic `IMPORT`** via a `Resolver` (`MemoryResolver`, or the
  `std`-gated `FileResolver`): a flat merge into one shared atom universe, so an
  imported axiom unifies with a local fact by identity.
- **Content-addressing** (sha256): identical clauses/axioms are deduped
  (idempotent — `P ∧ P ≡ P`), import cycles are detected, and a name redefined
  with a different body is an error.

The actual reasoning (three-valued forward chaining, SAT, all-SAT, the WARNING
pool, the four results) belongs to the planned `elenchus-solver`.

## Usage

```rust
use elenchus_compiler::{compile, compile_source, MemoryResolver};

// Single source:
let ir = compile_source("creature.vrf", "FACT Creature.A has flying\n").unwrap();
assert_eq!(ir.facts.len(), 1);

// With imports (string-backed; a file is just one backing store):
let mut r = MemoryResolver::new();
r.add("physics.vrf", "AXIOM speed_order:\n    WHEN Motor over_200\n    THEN Motor over_100\n");
r.add("main.vrf", "IMPORT \"physics.vrf\"\nFACT Motor over_200\nCHECK Motor\n");
let ir = compile("main.vrf", &r).unwrap();
assert!(ir.pending_imports.is_empty());
```

## License

MIT — see the [workspace LICENSE](../../LICENSE).
