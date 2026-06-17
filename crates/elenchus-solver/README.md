# elenchus-solver

The reasoning interpreter for [elenchus](https://github.com/) — the forward pass.

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

This is **phase 1** — the forward pass needs no SAT backend. The backward pass
(`UNDERDETERMINED` via all-SAT model finding) and a `no_std` varisat port are
future work.

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

MIT — see the [workspace LICENSE](../../LICENSE).
