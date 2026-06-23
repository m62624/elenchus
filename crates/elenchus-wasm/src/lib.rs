//! `elenchus-wasm` — a thin wasm-bindgen wrapper exposing the elenchus engine
//! (parse → compile → solve) to JavaScript as "program text in, JSON verdict
//! out".
//!
//! No engine logic is reimplemented here: it reuses the core `elenchus-solver`
//! pipeline verbatim and mirrors the `elenchus-mcp` tool surface
//! (`check` / `version` / `about`), adding `skill` accessors (so a consumer can
//! persist the companion skill next to the engine) and an `IMPORT` resolver
//! bridged to a JavaScript `read(path) -> string` callback.

use elenchus_solver::{CompileError, Report, Resolver, verify, verify_source};
use wasm_bindgen::prelude::*;

/// The companion skill (the whole DSL how-to + the verdict loop), embedded so a
/// consumer can persist it next to the engine without a second download. Single
/// source of truth: the repo-root `skill/SKILL.md`.
const SKILL_MD: &str = include_str!("../../../skill/SKILL.md");

/// Mirror of `elenchus-mcp`'s `ABOUT_TOOL` — a short, version-free pointer to the
/// skill (the version lives in [`version`]).
const ABOUT: &str = "elenchus checks whether a set of facts and first principles is \
logically consistent (a small three-valued SAT checker). You'll get markedly better results \
with the matching `elenchus` skill loaded — it carries the verdict loop (iterate until \
CONSISTENT), the DSL, and worked examples. Call `version()` for the engine version and load \
the skill that matches it (see `skill()`): https://github.com/m62624/elenchus";

/// Turn an optional cap into the `Option<usize>` the diagnostics renderer takes:
/// `0` or absent means "no limit", matching `elenchus-mcp`.
fn limit(n: Option<u32>) -> Option<usize> {
    match n {
        Some(v) if v > 0 => Some(v as usize),
        _ => None,
    }
}

/// Render a result the way `elenchus-mcp` does: a `Report` becomes JSON (or the
/// human report when `format == "human"`); a parse error becomes the grouped
/// diagnostic block (capped by the two limits); any other compile error becomes
/// its message.
fn render(
    result: Result<Report, CompileError>,
    format: Option<String>,
    max_classes: Option<u32>,
    max_per_class: Option<u32>,
) -> String {
    match result {
        Ok(report) => {
            if format.as_deref() == Some("human") {
                format!("{report}")
            } else {
                report.to_json()
            }
        }
        Err(CompileError::Parse(diag)) => diag.render(limit(max_classes), limit(max_per_class)),
        Err(other) => other.to_string(),
    }
}

/// Check a single `.vrf` program (inline text; `IMPORT` is not resolved — use
/// [`check_with_resolver`] for multi-file programs). Mirrors `elenchus_check`.
#[wasm_bindgen]
pub fn check(
    program: &str,
    format: Option<String>,
    max_classes: Option<u32>,
    max_per_class: Option<u32>,
) -> String {
    render(
        verify_source("<wasm>", program),
        format,
        max_classes,
        max_per_class,
    )
}

/// Check a `.vrf` program that may `IMPORT` other sources, resolving every import
/// through the JavaScript `read` callback: `read(path: string) -> string`
/// (synchronous; throw to signal "not found"). `root` is the entry path, passed
/// to `read` first. This is how Node `fs` (or any virtual store) backs `IMPORT`
/// inside wasm, where there is no filesystem.
#[wasm_bindgen]
pub fn check_with_resolver(
    root: &str,
    read: &js_sys::Function,
    format: Option<String>,
    max_classes: Option<u32>,
    max_per_class: Option<u32>,
) -> String {
    let resolver = JsResolver { read: read.clone() };
    render(verify(root, &resolver), format, max_classes, max_per_class)
}

/// Bridges the engine's [`Resolver`] to a JS `read(path) -> string` callback.
/// Path normalization mirrors the host `FileResolver` (relative imports resolve
/// against the importing file's directory, with manual `..` handling, forward
/// slashes) so resolved paths — and the provenance recorded in the IR — match
/// the CLI's.
struct JsResolver {
    read: js_sys::Function,
}

impl Resolver for JsResolver {
    fn load(&self, path: &str) -> Result<String, CompileError> {
        match self.read.call1(&JsValue::NULL, &JsValue::from_str(path)) {
            Ok(value) => value.as_string().ok_or_else(|| {
                CompileError::ImportNotFound(format!("{path}: read() did not return a string"))
            }),
            Err(err) => {
                let detail = err
                    .as_string()
                    .unwrap_or_else(|| "read() threw".to_string());
                Err(CompileError::ImportNotFound(format!("{path}: {detail}")))
            }
        }
    }

    fn resolve(&self, base: &str, relative: &str) -> String {
        use std::path::{Component, Path, PathBuf};
        let parent = Path::new(base).parent().unwrap_or_else(|| Path::new("."));
        let joined = parent.join(relative);
        let mut out = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                c => out.push(c),
            }
        }
        out.to_string_lossy().replace('\\', "/")
    }
}

/// The running engine version, e.g. `"elenchus 0.9.1"` — the *engine*, not this
/// npm package's version. Mirrors `elenchus_version`; compare it to the skill's
/// `<!-- skill-version -->` marker (see [`skill_version`]).
#[wasm_bindgen]
pub fn version() -> String {
    format!("elenchus {}", elenchus_solver::VERSION)
}

/// A short pointer to the companion skill. Mirrors `elenchus_about`.
#[wasm_bindgen]
pub fn about() -> String {
    ABOUT.to_string()
}

/// The full companion skill text (`SKILL.md`), so a consumer can persist it next
/// to the engine (e.g. into an agent's skills directory) without a second fetch.
#[wasm_bindgen]
pub fn skill() -> String {
    SKILL_MD.to_string()
}

/// The skill's `<!-- skill-version: X -->` marker (the engine version the skill
/// targets), parsed from the embedded skill. Empty if the marker is absent.
#[wasm_bindgen]
pub fn skill_version() -> String {
    skill_version_of(SKILL_MD).unwrap_or_default()
}

/// Extract the `<!-- skill-version: X -->` marker value from skill text.
fn skill_version_of(skill: &str) -> Option<String> {
    let marker = "<!-- skill-version:";
    let start = skill.find(marker)? + marker.len();
    let rest = &skill[start..];
    let end = rest.find("-->")?;
    Some(rest[..end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_reports_conflict_as_json() {
        let out = check("DOMAIN d\nFACT x a\nNOT x a\nCHECK x", None, None, None);
        assert!(
            out.contains("CONFLICT"),
            "expected a CONFLICT verdict, got: {out}"
        );
        assert!(
            out.contains("exit_code"),
            "expected machine JSON, got: {out}"
        );
    }

    #[test]
    fn check_human_format_differs_from_json() {
        let json = check("DOMAIN d\nFACT x a\nCHECK x", None, None, None);
        let human = check(
            "DOMAIN d\nFACT x a\nCHECK x",
            Some("human".to_string()),
            None,
            None,
        );
        assert!(json.contains("status"));
        assert_ne!(json, human);
    }

    #[test]
    fn version_reports_engine_not_package() {
        assert_eq!(version(), format!("elenchus {}", elenchus_solver::VERSION));
    }

    #[test]
    fn skill_text_and_marker_are_present() {
        assert!(skill().contains("name: elenchus"));
        // The bundled skill targets the engine it ships with.
        assert_eq!(skill_version(), elenchus_solver::VERSION);
    }
}
