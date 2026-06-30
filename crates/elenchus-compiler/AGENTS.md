<!-- pi-code-planner:contracts:start -->
## Planner Contracts

### Purpose
Documents the elenchus-compiler crate — AST-to-IR pipeline, atom interning, desugaring, content-addressing, import resolution.

### Parent
- `(root)`

### Child Index
- (none)

### Stable Contracts
- Atom interner produces deterministic u32 ids from canonically sorted (subject, predicate, object?) triples.
- Desugaring rules: CAPS → Impossible clauses, EXCLUSIVE → pairwise, WHEN…THEN → Impossible([A, …, NOT C]).
- SHA-256 content-addressing: identical clauses deduped (idempotent), redefinition with different body → PremiseRedefinition error.
- Import resolution is iterative, depth-first, with circular-import detection and per-import error recording.
- 11 source modules: compiler.rs, data.rs, domain.rs, error.rs, ir.rs, ports.rs, resolver.rs, sig.rs, subst.rs, closure.rs.

### Read First
- crates/elenchus-compiler/src/lib.rs
- crates/elenchus-compiler/src/compiler.rs
- crates/elenchus-compiler/src/resolver.rs
- crates/elenchus-compiler/src/ir.rs
- crates/elenchus-compiler/src/data.rs
- crates/elenchus-compiler/src/domain.rs
- crates/elenchus-compiler/src/error.rs
- crates/elenchus-compiler/src/ports.rs
- crates/elenchus-compiler/src/sig.rs
- crates/elenchus-compiler/src/subst.rs
- crates/elenchus-compiler/src/closure.rs

### Do Not Touch Unless
- Atom interner logic in compiler.rs — deterministic u32 ids are critical for IR stability
- Desugaring rules — consumers depend on exact transformation semantics
- Import resolution in resolver.rs — circular import detection, iterative depth-first merge

### Domain Details
- Module structure: compiler.rs (main compilation loop), data.rs (provide/set parsing), domain.rs (domain declarations), error.rs (diagnostics + nearest-atom suggestions), ir.rs (IR types), ports.rs (port resolution), resolver.rs (import resolution trait + FileResolver), sig.rs (signature-based dedup), subst.rs (substitution engine), closure.rs (transitive/symmetric/equivalence closures)
- Public API: compile_source() → Result<Ir, Diagnostics>, compile() → Result<Ir, Diagnostics>, read_data_bindings(), read_data_source()
- Feature flag: default=["std"], std enables FileResolver. no_std builds use InMemoryResolver.
- The compiler is the middle layer: parser AST → compiler IR → solver. Its output is consumed by both elenchus-solver and elenchus-wasm.
<!-- pi-code-planner:contracts:end -->
