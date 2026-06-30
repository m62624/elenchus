<!-- pi-code-planner:contracts:start -->
## Planner Contracts

### Purpose
Documents the elenchus-solver crate — the inference engine consuming compiled IR from elenchus-compiler. Covers three-valued Kleene logic forward chaining, backward CNF/SAT pass, report generation, and the SAT core (CDCL). This is the reasoning heart of the project.

### Parent
- `(root)`

### Child Index
- (none)

### Stable Contracts
- The solver consumes Compiled IR from elenchus-compiler and evaluates under three-valued Kleene logic (TRUE/FALSE/UNKNOWN, where UNKNOWN ≠ FALSE).
- Forward pass: seed model from confident FACT/NOT facts, forward-chain RULEs to fixpoint, evaluate all Impossible clauses (desugared PREMISEs).
- Verdict logic: all literals TRUE → CONFLICT; some FALSE → CONSISTENT; otherwise UNKNOWN → WARNING for implication premises, CONSISTENT for list premises (EXCLUSIVE/FORBIDS/ONEOF/ATLEAST).
- BIDIRECTIONAL mode triggers backward pass: premises/rules/facts encoded as CNF, passed to in-crate CDCL SAT core (sat module, replicating varisat's algorithm) — 0 models = CONFLICT, ≥2 models = UNDERDETERMINED.
- Conflict analysis: when facts/premises are consistent but ASSUME hypotheses break it, the solver names which assumptions to retract (retract_assumptions).
- Advisory-only signals (never influence verdict): similar_atom_pairs (typos), orphan_facts (logically-inert assertions), unused_imports, placeholders.
- The sat module contains a CDCL SAT solver core; cnf module handles CNF encoding; v3 module contains V3 protocol support; unsat module handles assumption retraction.
- The solver is the final layer: compiler → solver → report. It must never modify compiled IR.

### Read First
- crates/elenchus-solver/src/lib.rs
- crates/elenchus-solver/src/eval.rs
- crates/elenchus-solver/src/report/mod.rs
- crates/elenchus-solver/src/sat/mod.rs
- crates/elenchus-solver/src/sat/solver.rs
- crates/elenchus-solver/src/unsat.rs

### Do Not Touch Unless
- Forward chaining logic (eval.rs) — correctness of verdict depends on exact fixpoint behavior
- CNF encoding (cnf.rs) — backward pass correctness
- SAT core (sat/) — CDCL algorithm must not be altered without full regression testing
- Report types (report/) — consumers (CLI, MCP, wasm) depend on exact field shapes

### Domain Details
- Module structure: lib.rs (entry + solve/verify APIs), eval.rs (forward chaining + premise checking), report/mod.rs + report/human.rs + report/json.rs (Report type + rendering), sat/mod.rs + sat/solver.rs + sat/models.rs (CDCL SAT core), cnf.rs (CNF encoding), unsat.rs (assumption retraction), v3.rs (V3 protocol), analysis.rs (orphan_facts, similar_atom_pairs)
- Public API: solve(), verify(), verify_with(), verify_source(), verify_source_with(), Report, Status, Retract, Conflict, Warning, TraceStep, TraceReason, CoreItem, Derived, OrphanFact, SimilarAtoms, sat module (CDCL), V3, VERSION constant
- Feature flags: std (enables FileResolver re-export), default-features = false for no_std builds
- The solver is the final processing layer. It reads Compiled IR (immutable) and produces Report. It re-exports compiler types for convenience so downstream users need only one import.
<!-- pi-code-planner:contracts:end -->
