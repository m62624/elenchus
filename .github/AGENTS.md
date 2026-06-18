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
- Installers are `shell` + `powershell` + `msi` + `homebrew`. `cargo binstall`
  works on top of the cargo-dist `dist-manifest.json` with no extra config, for
  both `elenchus-cli` and `elenchus-mcp`.
- Homebrew: `tap = "m62624/homebrew-elenchus"` + `publish-jobs = ["homebrew"]`
  push a formula per binary (`elenchus-cli.rb`, `elenchus-mcp.rb`) to the tap via
  the `publish-homebrew-formula` job. Both binary crates set `homepage.workspace
  = true` (Homebrew warns without a homepage).
- The Windows `.msi` needs per-package WiX config: `[package.metadata.wix]`
  (`upgrade-guid`/`path-guid`, both `license`/`eula = false`) plus a committed
  `crates/<bin>/wix/main.wxs`. The GUIDs are stable identities and must match the
  `.wxs` — never regenerate/change them. These were produced once via `dist init`
  in a throwaway clone (NOT in-tree, which would clobber `release.yml`). MSI
  builds inside `dist build` via WiX v3 (`candle`/`light`). GitHub's Windows
  runner images no longer ship WiX v3, so `bin-release.yml` has a manual
  "Install WiX v3" step (downloads the v3.14.1 binaries to PATH, Windows only)
  before `dist build`. WiX v4+ will NOT work — cargo-dist needs v3.
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
  `workflow_dispatch`. Jobs run **in parallel** (no cross-job `needs`): `check`
  (matrix Linux/Windows/macOS, fail-fast off: fmt + clippy + test on each OS),
  `no_std` (build the libs for `wasm32v1-none`), `dist-plan` (`dist plan`).
- `workflows/release.yml` (orchestrator) → triggered by pushing a `pin/v*` tag.
  **Flow:** `prepare` (parse the version, create `rc/vX.Y.Z`, `cargo set-version
  --workspace`, commit, push the RC, delete the pin tag) → `tests` (calls `ci.yml`
  against the RC) → `tag` (create the real `vX.Y.Z`) → `dist` (calls
  `bin-release.yml` — binaries + GitHub Release) → `publish-crates` (`cargo
  publish --workspace --locked` from the tag — publishes all 5 crates to
  crates.io in dependency order, including the two binary crates, only after the
  binary release succeeds) → `sync` (needs both; opens a PR from the RC to the
  repository's **default branch** — not hardcoded).
- `workflows/bin-release.yml` → the cargo-dist-generated workflow, kept intact
  except its trigger was changed to `on: workflow_call` (inputs: `tag`) and every
  checkout uses `ref: ${{ inputs.tag }}`. Builds binaries + shell/powershell/msi
  installers for the 6 configured targets, pushes the Homebrew formula after the
  global artifacts are built (`publish-homebrew-formula` → the tap, needs
  `HOMEBREW_TAP_TOKEN`), and only then publishes the GitHub Release. Regenerate
  the body (not the trigger) with `dist generate` only if the cargo-dist version
  in `Cargo.toml` changes.
- `workflows/labeler.yml` → on PR open/edit/sync, labels by Conventional-Commits
  prefix; the labels feed the changelog categories in the root `release.yml`.
- `release.yml` (repo root, not a workflow) → GitHub auto-generated-release-notes
  config (changelog categories by label).

### External Setup Required
- The shell/powershell/msi installers need no secrets. The `.msi` build needs WiX
  v3, which `bin-release.yml` installs itself (GitHub runners no longer ship it).
- **Homebrew** (`publish-homebrew-formula`) needs: a tap repo named
  `m62624/homebrew-elenchus` (must be created), and a `HOMEBREW_TAP_TOKEN` repo
  secret — a GitHub token with contents:write on that tap repo. Without it the
  job fails, but everything else still ships.
- **crates.io** (`publish-crates`) needs a `CARGO_REGISTRY_TOKEN` repo secret (a
  crates.io API token with publish scope). Without it that job fails, but the
  binary artifacts + GitHub Release still ship. Publishing to crates.io is what
  lets `cargo install <crate>` and the short `cargo binstall <crate>` resolve by
  name; for an unpublished/private repo use `cargo binstall --git <repo-url>
  elenchus-cli` (likewise `elenchus-mcp`).
<!-- elenchus:contracts:end -->
