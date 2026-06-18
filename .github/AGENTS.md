<!-- elenchus:contracts:start -->
## GitHub Automation Contracts

### Purpose
CI, PR labeling, the release-candidate flow, and tagged binary releases via
cargo-dist. Pure Rust — there is no npm/Node anywhere here.

### Stable Contracts
- CI must run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  and `cargo test --workspace` on Linux/Windows/macOS; plus a `no_std` build of the
  three library crates for the bare `wasm32v1-none` target, plus `dist plan`.
- CI is **not tied to a base branch**: pushes to any branch and PRs against any
  base run it (an integration branch collecting many PRs is supported).
- Binary releases ship only the two binaries — the `elenchus` CLI and the
  `elenchus-mcp` server. The three libraries set `dist = false`.
- The cargo-dist workflow is customized (`workflow_call`), so `dist generate`
  is **not** run to keep it in sync; `allow-dirty = ["ci"]` is set so `dist plan`
  does not fail on it. Do not blindly run `dist init`/`dist generate` — it would
  overwrite `release.yml` with the un-orchestrated default.

### Read First
- `workflows/ci.yml`
- `workflows/release.yml` (orchestrator)
- `workflows/bin-release.yml` (cargo-dist, `workflow_call`)
- `release.yml` (repo-root: release-notes config)

### Domain Details
- `workflows/ci.yml` → `push`/`pull_request` (path-filtered, any branch),
  `workflow_call` (with a `ref` input, used by `release.yml`'s `tests` job), and
  `workflow_dispatch`. Jobs: `test` (matrix Linux/Windows/macOS: fmt + clippy +
  test), `no_std` (build the libs for `wasm32v1-none`), `dist-plan` (`dist plan`).
- `workflows/release.yml` (orchestrator) → triggered by pushing a `pin/v*` tag.
  **Flow:** `prepare` (parse the version, create `rc/vX.Y.Z`, `cargo set-version
  --workspace`, commit, push the RC, delete the pin tag) → `tests` (calls `ci.yml`
  against the RC) → `tag` (create the real `vX.Y.Z`) → `dist` (calls
  `bin-release.yml` with that tag) → `sync` (open a PR from the RC to the
  repository's **default branch** — not hardcoded).
- `workflows/bin-release.yml` → the cargo-dist-generated workflow, kept intact
  except its trigger was changed to `on: workflow_call` (inputs: `tag`) and every
  checkout uses `ref: ${{ inputs.tag }}`. Builds binaries + shell/powershell/
  homebrew installers for the 6 configured targets and publishes the GitHub
  Release. Regenerate the body (not the trigger) with `dist generate` only if the
  cargo-dist version in `Cargo.toml` changes.
- `workflows/labeler.yml` → on PR open/edit/sync, labels by Conventional-Commits
  prefix; the labels feed the changelog categories in the root `release.yml`.
- `release.yml` (repo root, not a workflow) → GitHub auto-generated-release-notes
  config (changelog categories by label).

### External Setup Required
- Homebrew publishing needs a tap repo `m62624/homebrew-elenchus` and a
  `HOMEBREW_TAP_TOKEN` secret with write access to it. Until that exists, drop
  `"homebrew"` from `installers` (and `publish-jobs`) in `Cargo.toml`, or expect
  the `publish-homebrew-formula` job to fail (binaries + shell/powershell still release).
<!-- elenchus:contracts:end -->
