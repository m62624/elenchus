<!-- pi-code-planner:contracts:start -->
## Planner Contracts

### Purpose
Root AGENTS.md for the elenchus project — documents project architecture, CI/CD workflows, release process, versioning discipline, and SKILL synchronization rules.

### Parent
- `(root)`

### Child Index
- `.github/AGENTS.md`: GitHub Automation Contracts — CI, PR labeling, release-candidate flow, and tagged binary releases via cargo-dist.
- `crates/elenchus-compiler/AGENTS.md`: Compiler crate contracts — AST-to-IR pipeline, atom interning, desugaring, content-addressing, import resolution.
- `crates/elenchus-solver/AGENTS.md`: Solver crate contracts — three-valued Kleene logic forward chaining, backward CNF/SAT pass, report generation, CDCL core.
- `crates/elenchus-mcp/AGENTS.md`: MCP server contracts — JSON-RPC 2.0 over stdio, three tools (elenchus_check, elenchus_version, elenchus_about).

### Stable Contracts
- CI must run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` on Linux/Windows/macOS; plus `no_std` build of three library crates for `wasm32v1-none`, plus `dist plan`.
- CI is not tied to a base branch: pushes to any branch and PRs against any base run it, except release-candidate branches `rc/v*` which are invoked via `workflow_call`.
- Binary releases ship only two binaries — `elenchus` CLI and `elenchus-mcp` server. Three libraries set `dist = false`.
- Release process: push `pin/v*` tag → `prepare` (bump version, create RC branch) → `tests` (CI on RC) → `skill-check` (verify SKILL.md marker matches workspace version) → `tag` (create vX.Y.Z) → `dist` (build binaries + installers) → `publish-crates` (crates.io) → `publish-npm` (npm) → `sync` (PR RC→main).
- SKILL.md `skill-version` marker MUST equal the workspace version at release time. This is a manual checkpoint — humans must review and update SKILL.md instructions for the new version. Auto-bump is forbidden by design.
- SKILL.md frontmatter: name ≤ 64 chars, description ≤ 1024 chars (Anthropic Agent Skills limits).

### Read First
- .github/AGENTS.md
- crates/elenchus-compiler/AGENTS.md
- crates/elenchus-solver/AGENTS.md
- crates/elenchus-mcp/AGENTS.md
- skill/SKILL.md
- Cargo.toml

### Do Not Touch Unless
- Do not run `cargo bump` or `cargo set-version` manually — CI does this during release.
- Do not auto-bump the SKILL.md `skill-version` marker — it must be manually reviewed for each release.
- Do not create redundant SKILL content. Each SKILL must be unique and non-redundant. Avoid repeating the same information across multiple SKILL files.

### Domain Details
- Version bumping discipline: When a SKILL requests a version bump (recorded in `domainDetails`), do NOT perform `cargo bump`. CI automatically bumps versions during tagging via `cargo set-version --workspace` in the `prepare` job. The SKILL spec does not auto-bump — this ensures humans always manually review and synchronize the SKILL with the actual technical functionality before any version change.
- SKILL content deduplication: When writing AGENTS.md, include a rule that SKILL creation must avoid repeating the same information across multiple SKILL files. Each SKILL should be unique and non-redundant. This prevents future duplication where the same content appears many times across different SKILL files.
- Project structure: Cargo workspace with 6 crates — `elenchus-parser`, `elenchus-compiler`, `elenchus-solver`, `elenchus-cli`, `elenchus-mcp`, `elenchus-wasm`. Libraries publish to crates.io; only CLI and MCP ship as binaries via cargo-dist.
- Release workflow: `pin/v*` tag triggers orchestrator. `skill-check` job verifies SKILL.md marker == workspace version. `skill-asset` uploads SKILL.md as a release asset for agent consumption.
- CI jobs run in parallel (no cross-job `needs`): `check` (lint+test matrix Linux/Windows/macOS), `no_std` (wasm32v1-none build), `wasm-pack` (npm build smoke-test), `dist-plan`. Gate job `ci-pass` aggregates all for branch protection.
- WiX v3 required for Windows MSI builds. GitHub runners no longer ship WiX v3 — `bin-release.yml` installs v3.14.1 manually. WiX v4+ will NOT work.
- crates.io publish needs `CARGO_REGISTRY_TOKEN` secret. Homebrew publish needs `HOMEBREW_TAP_TOKEN` secret + `m62624/homebrew-elenchus` tap repo.
- npm publish uses OIDC trusted publishing (no NPM_TOKEN needed). First publish of a new package name must be done manually with npm token.
- Data flow: parser → compiler → solver → report. CLI and MCP are consumer-facing binaries that wrap the solver pipeline. WASM is a JavaScript bridge to the solver.
<!-- pi-code-planner:contracts:end -->
