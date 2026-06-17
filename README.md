# elenchus

A formal reasoning-verification engine for small language models.

A small model writes only **facts** and **first principles** (axioms) in a simple,
English-like DSL. A Rust engine does all the logical inference and catches
contradictions mathematically. The model cannot lie at the inference level — only
at the axiom level — and a mistake in an axiom is caught early and mechanically.

The name *elenchus* (ἔλεγχος) is the Socratic method of cross-examining claims
against first principles until a contradiction surfaces — exactly what the engine
does to a set of facts.

See [`docs/SPEC.md`](docs/SPEC.md) for the full specification (epistemic basis,
the single `Impossible` primitive, three-valued Kleene logic, the grammar, and
the invariants).

## Workspace

| Crate | Role |
|-------|------|
| [`elenchus-parser`](crates/elenchus-parser) | English-like DSL text → AST. `nom` + `nom_locate`, `no_std`, precise `^--- here` errors. |
| [`elenchus-compiler`](crates/elenchus-compiler) | AST → canonical `Impossible`/CNF clause IR: import resolution, desugaring, atom interning, sha256 content-addressed dedup. `no_std`. |
| `elenchus-solver` *(planned)* | A `no_std` SAT engine (varisat port) + the three-valued layer, WARNING pool, and the four results. |

## Example

```vrf
IMPORT "physics.vrf"

FACT Motor over_200
NOT  Motor over_100        // contradicts the imported speed_order axiom

CHECK Motor
```

The compiler merges the imported library and the local facts into one shared atom
universe; the solver (planned) reports the `CONFLICT`.

## Status

Parser and compiler are implemented and tested (`no_std`, warning-free). The
solver and a thin CLI are the next milestones.

## License

MIT — see [LICENSE](LICENSE).
