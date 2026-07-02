# elenchus-solver

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

The inference interpreter of [elenchus](https://github.com/m62624/elenchus), part of
that project — the forward pass plus a backward SAT pass.

`no_std` (needs `alloc`). Consumes the
[`elenchus-compiler`](https://github.com/m62624/elenchus/tree/main/crates/elenchus-compiler)
`Compiled` IR and
evaluates it under three-valued Kleene logic (TRUE / FALSE / UNKNOWN, where
UNKNOWN ≠ FALSE).

## What it does

1. Seeds a model from confident `FACT`/`NOT` facts; `FACT X` + `NOT X` is a CONFLICT.
2. Forward-chains `RULE`s to a fixpoint, deriving facts (a derived value that
   contradicts a known one is a CONFLICT).
3. Evaluates every `Impossible` clause (the desugared premises):
   - all literals forced TRUE → **CONFLICT** (constraint violated);
   - some literal FALSE → satisfied → **CONSISTENT**;
   - otherwise an UNKNOWN blocks the check → **WARNING** for implication premises
     (missing data), CONSISTENT for list premises (`EXCLUSIVE`/`FORBIDS`/`ONEOF`/
     `ATLEAST` — UNKNOWN means "no conflict yet").

On `CHECK ... BIDIRECTIONAL` a **backward pass** runs too: the premises, rules and
confident facts are encoded as CNF and solved by a small in-crate CDCL SAT core
(`sat`, a `no_std` replication of [varisat](https://github.com/jix/varisat)'s
algorithm). It counts models: 0 → jointly unsatisfiable (a CONFLICT the forward
pass may miss), ≥2 → an alternative model exists (`UNDERDETERMINED`).

## Algorithms

What actually runs, piece by piece — no ML, just classic algorithms:

| Piece | Algorithm |
|-------|-----------|
| forward pass | forward chaining to a fixpoint over three-valued Kleene logic (TRUE / FALSE / UNKNOWN) |
| SAT search | CDCL: assignment trail + decision levels, two-watched-literal propagation with blocking literals, 1-UIP conflict analysis with clause learning, non-chronological backjumping |
| decision order | VSIDS activity with decay + phase saving; the next variable comes off an indexed max-heap (O(log n) per decision) |
| learned clauses | MiniSat-style minimization (ccmin) — enabled only where the caller consumes verdicts/counts, so reported witnesses never shift |
| assumptions | MiniSat's `analyzeFinal`: a contradicted assumption yields a sufficient unsat core |
| incremental solving | one clause database answers a whole sequence of assumption queries; learned clauses persist between them |
| model counting | enumeration with blocking clauses, counted up to two; per-query blocking clauses are disarmed by guard variables |
| unsat core | assumption-selector core, then deletion minimization → an irreducible blamed set |
| fix checking (drop / flip) | each candidate fix is re-solved on the shared incremental solver; only verified fixes are reported |
| `TRY` (abduction) | one bounded side-solve per supplied hypothesis |
| `BECAUSE` / `UNLESS` / `WITNESS` / `KNOWS` | direct reads of the settled model, constant work per line |
| performance gating | deterministic work counters (decisions / propagations / conflicts / learned literals) — bit-identical on any hardware, asserted in tests; wall-clock benchmarks are informational only |

Intentionally omitted from the SAT core: proof/DRAT logging, clause-database GC,
multithreading. Luby restarts were implemented, measured on the work counters, and
rejected — they only added conflicts on this engine's workloads.

## Usage

`verify_source` takes a source label and one program string (every program opens with
`DOMAIN`; atoms print namespaced as `<domain>.<atom>`). The program can be written
multi-line or squeezed onto one line with `\n` separators — the parser is
newline-oriented, not indentation-sensitive. Here a premise whose antecedent holds but
whose consequent is never established, so the check is blocked (`WARNING`):

```rust
use elenchus_solver::{verify_source, Status};

let report = verify_source(
    "demo.vrf",
    "DOMAIN demo\nFACT A has flying\nPREMISE w:\n    WHEN A has flying\n    THEN A has wing\nCHECK A\n",
)
.unwrap();
assert_eq!(report.status, Status::Warning); // `demo.A has wing` is UNKNOWN
println!("{report}");
```

The `Report`'s `Display` is the full human report (the same text the CLI prints):

```text
RESULT: WARNING
  WARNING   w (PREMISE)  [demo.vrf:3]
      blocked by: demo.A has wing
      fix: nothing determines `demo.A has wing` — add `FACT demo.A has wing` (or `NOT …`), or if a PREMISE's THEN is meant to establish it, make that PREMISE a RULE so it derives the value
SUMMARY: 0 conflicts, 0 underdetermined, 1 warnings, 0 derived
EXIT_CODE: 1
```

Sibling entry points cover the other inputs: `verify` resolves `IMPORT`s through a
`Resolver`, and `verify_source_with` / `verify_with` bind `VAR` ports. Each returns the
same `Report`.

## License

MIT — see [LICENSE](LICENSE).
