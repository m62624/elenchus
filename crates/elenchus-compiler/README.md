# elenchus-compiler

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

Part of [elenchus](https://github.com/m62624/elenchus): compiles the parsed DSL into a
canonical, solver-ready intermediate representation. **Preparation only — no solving.**

`no_std` (needs `alloc`); the optional `std` feature adds a filesystem-backed
import resolver.

## What it does

- **Atom interner** — `(subject, predicate, object?)` → dense `u32` ids,
  canonically sorted so ids (and any later enumeration) are deterministic.
- **Desugaring** to the single `Impossible` primitive: `EXCLUSIVE`/`FORBIDS`
  pairwise, `ONEOF` = pairwise + at-least-one, `ATLEAST` = one all-negated clause,
  `EXISTS <b> IN <set>` = at-least-one over the per-element instantiations,
  `WHEN … THEN` → `Impossible([A.., NOT C])` per consequent. `RULE` bodies are
  kept separate as forward-chaining rules.
- **Bounded quantification, compile-time** — `FOR EACH … IN <set>` / `… <rel> …`
  ground the body once per element/pair (linear); `CLOSE <rel>
  TRANSITIVE|SYMMETRIC|REFLEXIVE|EQUIVALENCE|SCC` closes a relation as a graph op
  (only `TRANSITIVE` requires a DAG). All of it desugars away — **the solver is
  never touched**, so every cost lives at compile time.
- **Source-agnostic `IMPORT`** via a `Resolver` (`MemoryResolver`, or the
  `std`-gated `FileResolver`): a flat merge into one shared atom universe, so an
  imported premise unifies with a local fact by identity.
- **Content-addressing** (sha256): identical clauses/premises are deduped
  (idempotent — `P ∧ P ≡ P`), import cycles are detected, and a name redefined
  with a different body is an error.

The actual reasoning (three-valued forward chaining, SAT, all-SAT, the WARNING
pool, the four results) lives in
[`elenchus-solver`](https://github.com/m62624/elenchus/tree/main/crates/elenchus-solver),
which consumes the `Compiled` IR produced here.

## Usage

Every program opens with `DOMAIN`; `compile_source` takes a source label and one
program string, `compile` resolves `IMPORT`s through a `Resolver`.

```rust
use elenchus_compiler::{compile, compile_source, MemoryResolver};

// Single source (one line or many — the parser is newline-oriented):
let ir = compile_source("creature.vrf", "DOMAIN creature\nFACT A has flying\n").unwrap();
assert_eq!(ir.facts.len(), 1);

// With imports (string-backed; a file is just one backing store). Atoms are
// qualified into the imported domain, so `physics.Motor` is the SAME atom the
// library's premise constrains:
let mut r = MemoryResolver::new();
r.add(
    "physics.vrf",
    "DOMAIN physics\nPREMISE speed_order:\n    WHEN Motor over_200\n    THEN Motor over_100\n",
);
r.add(
    "main.vrf",
    "DOMAIN demo\nIMPORT \"physics.vrf\"\nFACT physics.Motor over_200\nCHECK\n",
);
let ir = compile("main.vrf", &r).unwrap();
assert!(ir.pending_imports.is_empty()); // every import resolved
```

## License

MIT — see [LICENSE](LICENSE).
