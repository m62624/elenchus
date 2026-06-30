<!-- pi-code-planner:contracts:start -->
## Planner Contracts

### Purpose
Documents the elenchus-compiler crate — the AST-to-IR compilation pipeline. Covers atom interning, desugaring, content-addressing, import resolution, and the Compiler struct. This is the bridge between parsing (elenchus-parser) and solving (elenchus-solver).

### Parent
- `(root)`

### Child Index
- (none)

### Stable Contracts
- The compiler takes parsed AST from elenchus-parser and produces a deterministic solver-ready IR.
- Atom interning maps (subject, predicate, object?) to dense u32 ids, canonically sorted for determinism.
- Desugaring converts surface CAPS sugar (EXCLUSIVE, WHEN…THEN, etc.) into Impossible clauses.
- Content-addressing via SHA-256 deduplicates identical clauses; premise redefinition with different body is an error.
- IMPORT resolution is source-agnostic via the Resolver trait; compile() resolves imports, compile_source() leaves them pending.
- Resolution is iterative (work stack, not native recursion) to handle arbitrarily deep import chains.
- Each source is keyed by DOMAIN; atoms unify only within a domain.
- A source reached by several paths is compiled once (diamond imports stay linear).

### Read First
- crates/elenchus-compiler/src/lib.rs
- crates/elenchus-compiler/src/compiler.rs
- crates/elenchus-compiler/src/ir.rs
- crates/elenchus-compiler/src/resolver.rs
- crates/elenchus-compiler/src/error.rs

### Do Not Touch Unless
- Compiler pipeline logic — changes affect all downstream crates
- IR data structures — consumers in solver depend on exact field layouts
- Resolver trait — used by solver and wasm

### Domain Details
- Module structure: lib.rs (entry + public API), compiler.rs (Compiler struct), ir.rs (IR types: AtomId, Clause, Fact, Lit, Origin, etc.), resolver.rs (Resolver trait + MemoryResolver + FileResolver), error.rs (CompileError + UnknownValue), data.rs (read_data_bindings/read_data_source), domain.rs, closure.rs, ports.rs, sig.rs, subst.rs
- Public API: compile(), compile_with(), compile_source(), compile_source_with(), hash_hex(), Compiler struct, Resolver trait, FileResolver (std feature), all IR types re-exported
- Feature flags: std (enables FileResolver), default-features = false for no_std builds
- The crate is the middle layer: parser → compiler → solver. Changes here ripple to both neighbors.
<!-- pi-code-planner:contracts:end -->
