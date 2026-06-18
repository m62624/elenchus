# elenchus-solver

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

The reasoning interpreter for [elenchus](https://github.com/m62624/elenchus) — the forward pass.

`no_std` (needs `alloc`). Consumes the [`elenchus-compiler`] `Compiled` IR and
evaluates it under three-valued Kleene logic (TRUE / FALSE / UNKNOWN, where
UNKNOWN ≠ FALSE).

## What it does

1. Seeds a model from confident `FACT`/`NOT` facts; `FACT X` + `NOT X` is a CONFLICT.
2. Forward-chains `RULE`s to a fixpoint, deriving facts (a derived value that
   contradicts a known one is a CONFLICT).
3. Evaluates every `Impossible` clause (the desugared axioms):
   - all literals forced TRUE → **CONFLICT** (constraint violated);
   - some literal FALSE → satisfied → **CONSISTENT**;
   - otherwise an UNKNOWN blocks the check → **WARNING** for implication axioms
     (missing data), CONSISTENT for list axioms (`EXCLUSIVE`/`FORBIDS`/`ONEOF`/
     `ATLEAST` — UNKNOWN means "no conflict yet").

On `CHECK ... BIDIRECTIONAL` a **backward pass** runs too: the axioms, rules and
confident facts are encoded as CNF and solved by a small in-crate CDCL SAT core
(`sat`, a `no_std` replication of [varisat](https://github.com/jix/varisat)'s
algorithm — trail + decision levels, two-watched-literal propagation, 1-UIP
clause learning, non-chronological backjumping, VSIDS). It counts models: 0 →
jointly unsatisfiable (a CONFLICT the forward pass may miss), ≥2 → an alternative
model exists (`UNDERDETERMINED`). varisat's infra (proof logging, GC, restarts,
multithreading) is intentionally omitted.

## Usage

```rust
use elenchus_solver::{verify_source, Status};

let report = verify_source(
    "demo.vrf",
    "FACT A has flying\nAXIOM w:\n    WHEN A has flying\n    THEN A has wing\n",
)
.unwrap();
assert_eq!(report.status, Status::Warning); // `A has wing` is UNKNOWN
println!("{report}");
```

```text
RESULT: WARNING
  WARNING   w (AXIOM)  [demo.vrf:2]
      blocked by: A has wing
SUMMARY: 0 conflicts, 1 warnings, 0 derived
```

## License

MIT — see [LICENSE](LICENSE).
