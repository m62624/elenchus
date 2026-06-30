<!-- pi-code-planner:contracts:start -->
## Planner Contracts

### Purpose
Documents the elenchus-solver crate — three-valued Kleene logic, forward chaining, backward CNF/SAT pass, CDCL core, report generation.

### Parent
- `(root)`

### Child Index
- (none)

### Stable Contracts
- Three-valued Kleene logic: TRUE, FALSE, UNKNOWN. Forward chaining fixes UNKNOWN to TRUE when all antecedents are TRUE.
- Bidirectional solving: forward chaining to fixpoint, then CNF conversion, then CDCL SAT pass for assumption analysis.
- CDCL core: clause learning, conflict analysis, non-chronological backjumping, decision heuristics, unit propagation.
- Report generation: consistent/warning/underdetermined/conflict verdicts, derivation traces, unsat core extraction, assumption retraction analysis.
- 7+ source modules: solve.rs, sat.rs, report.rs, ports.rs, prop.rs, forward.rs, cnf.rs, puzzles.rs test module.

### Read First
- crates/elenchus-solver/src/lib.rs
- crates/elenchus-solver/src/solve.rs
- crates/elenchus-solver/src/sat.rs
- crates/elenchus-solver/src/report.rs
- crates/elenchus-solver/src/ports.rs
- crates/elenchus-solver/src/prop.rs
- crates/elenchus-solver/src/forward.rs
- crates/elenchus-solver/src/cnf.rs

### Do Not Touch Unless
- CDCL SAT core in sat.rs — clause learning and conflict analysis are critical for correct unsat cores
- Three-valued Kleene logic in prop.rs — the foundation of all reasoning
- Report generation in report.rs — consumer-facing output format

### Domain Details
- Module structure: solve.rs (main solve_entry + bidirectional pipeline), sat.rs (CDCL SAT solver), report.rs (human + JSON report generation), ports.rs (port resolution), prop.rs (propositional logic + Kleene truth table), forward.rs (forward chaining), cnf.rs (CNF conversion), puzzles.rs (test-only puzzle solving)
- Public API: solve() → Result<SolveResult, SolveError>, SolveResult enum (Consistent/Warning/Underdetermined/Conflict)
- Feature flag: default=["std"], std enables file I/O for data files. no_std builds use in-memory data.
- The solver is the reasoning engine. Its output feeds both elenchus-cli (exit codes) and elenchus-mcp (JSON responses).
<!-- pi-code-planner:contracts:end -->
